//! The real Rust-side streaming HTTP download (spec FR-001/FR-002/FR-005/FR-009 — see this
//! module's own docs for how progress/concurrency/rate-limit are all threaded through one call).

use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use futures_util::StreamExt;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::domain_rules::DomainRule;
use super::filename::resolve_filename;
use super::DownloadManager;

/// Re-resolves the effective rate limit (`max_bytes_per_sec`) for `hostname` — invoked on every
/// chunk read so a user clearing/changing a rate cap takes effect mid-transfer, not only on
/// downloads started after the change (see `download_one`'s own docs for the concurrency/rate
/// asymmetry this deliberately introduces). Implementations must be cheap in the steady state:
/// the canonical [`super::live_rate::LiveRateResolver`] does one atomic generation-counter load
/// per call and only touches Redis when the counter actually moved.
pub trait RateResolver: Send + Sync {
    fn resolve(&self, hostname: &str) -> BoxFuture<'static, Option<u64>>;
}

/// A resolver that never rate-limits — used by unit tests and callers with no live plugin-options
/// source (the real-H@H integration test in `plugins.rs`), where `rules` is `&[]` anyway.
pub struct NoopRateResolver;

impl RateResolver for NoopRateResolver {
    fn resolve(&self, _hostname: &str) -> BoxFuture<'static, Option<u64>> {
        Box::pin(async { None })
    }
}

/// One resource to fetch — mirrors `contracts/plugin-download-protocol.md`'s `downloads[]`
/// element (deserialized straight from a plugin's `execDownload` JSON result upstream).
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub method: Option<String>,
    pub headers: Vec<(String, String)>,
    pub filename_hint: Option<String>,
}

/// Sidecar metadata persisted alongside a resumable download's partial staging file (issue #88) —
/// stored as `{resume_path}.meta.json`, next to the `.part` file itself. Written once a `200`
/// response is first seen for a `resume_key`'d download, read back on the next attempt (a retry
/// after a Stop, a crash, or a container restart) to decide whether the partial bytes already on
/// disk are still safe to build on. The staging file itself carries no metadata of its own (it's
/// raw response bytes, byte-for-byte what ends up in the final archive) — this sidecar is the only
/// place that record lives.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeMetadata {
    /// The exact URL this partial file was downloaded from — a resume attempt that somehow ended
    /// up with a different URL for the same `resume_key` (shouldn't happen in practice, since
    /// `resume_key` is derived from a fixed queue-item/resource-index pair, but cheap to check) is
    /// treated the same as any other mismatch: discard and restart.
    url: String,
    /// The response's own strong validator, when it sent one — compared byte-for-byte against the
    /// resume attempt's own `If-Range` response to decide whether the server still considers this
    /// the same underlying content (see `download_one_inner`'s `If-Range` handling).
    etag: Option<String>,
    last_modified: Option<String>,
    /// The filename resolved from the *original* `200` response's own headers/hint (see
    /// `resolve_filename`) — a resumed `206` response is not guaranteed to repeat
    /// `Content-Disposition` (some servers only send it on the initial full response), so this is
    /// the authoritative filename for the whole download, not re-derived on resume.
    filename: String,
}

/// Where a resumable download's partial bytes and sidecar metadata live on disk, derived once from
/// `resume_key` — kept as a single struct (rather than recomputing both paths separately at each
/// call site) so the `.part`/`.meta.json` naming convention only needs to be written in one place.
struct ResumePaths {
    part_path: PathBuf,
    meta_path: PathBuf,
}

impl ResumePaths {
    fn new(staging_dir: &Path, resume_key: &str) -> Self {
        // `resume_key` is caller-controlled (`plugins.rs` derives it from a queue-item UUID plus a
        // small integer index — see `run_managed_downloads`), never raw user/plugin input, so no
        // path-traversal sanitization is needed here the way `sanitize_filename` needs for a
        // server-supplied name.
        let base = format!("download-resume-{resume_key}");
        Self {
            part_path: staging_dir.join(format!("{base}.part")),
            meta_path: staging_dir.join(format!("{base}.meta.json")),
        }
    }

    async fn remove_both(&self) {
        let _ = tokio::fs::remove_file(&self.part_path).await;
        let _ = tokio::fs::remove_file(&self.meta_path).await;
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("invalid URL {0:?}: {1}")]
    InvalidUrl(String, #[source] url::ParseError),
    #[error("invalid HTTP method {0:?}")]
    InvalidMethod(String),
    #[error("request failed: {0}")]
    Request(#[source] reqwest::Error),
    #[error("server responded with status {1}")]
    HttpStatus(String, reqwest::StatusCode),
    #[error("failed to write to {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
    /// The caller's `CancellationToken` fired (a user pressed Stop) — deliberately not converted
    /// to a `QueueError` variant the way every other member of this enum is: a user-requested stop
    /// isn't a failure to surface as an error state, it's handled by the caller reverting the
    /// queue item back to `Queued` instead (see `download_queue::stop_one`).
    #[error("cancelled")]
    Cancelled,
}

/// Converts to the structured, translatable [`lanrurugi_core::queue_error::QueueError`] the
/// frontend actually renders — logs the original `Display` text via `tracing::warn!` here (the
/// one place it's available before being discarded) rather than serializing it anywhere
/// user-facing.
impl From<&DownloadError> for lanrurugi_core::queue_error::QueueError {
    fn from(e: &DownloadError) -> Self {
        use lanrurugi_core::queue_error::QueueError;
        tracing::warn!(error = %e, "download failed");
        match e {
            DownloadError::InvalidUrl(url, _) => QueueError::InvalidUrl { url: url.clone() },
            DownloadError::InvalidMethod(method) => QueueError::InvalidHttpMethod {
                method: method.clone(),
            },
            DownloadError::Request(err) => QueueError::HttpRequestFailed {
                url: err.url().map(|u| u.to_string()).unwrap_or_default(),
            },
            DownloadError::HttpStatus(url, status) => QueueError::HttpStatus {
                url: url.clone(),
                status: status.as_u16(),
            },
            DownloadError::Io(..) => QueueError::WriteFailed,
            // Every real caller intercepts `Cancelled` before it ever reaches this conversion
            // (`run_managed_downloads`/`start_download` check for it explicitly and revert the
            // queue item to `Queued` instead of recording an error) — this arm only exists so the
            // match stays exhaustive; `Internal` is a harmless placeholder that should never
            // actually surface.
            DownloadError::Cancelled => QueueError::Internal,
        }
    }
}

/// Result of a single successful streaming download.
#[derive(Debug)]
pub struct DownloadedFile {
    /// The staged file's path (caller is responsible for `ingest_file`-ing it, per
    /// `upload.rs::upload_archive`'s own established staging pattern).
    pub path: PathBuf,
    /// The real filename to use for the final cataloged archive (see [`resolve_filename`]'s own
    /// docs on how this is determined).
    pub filename: String,
    pub bytes_downloaded: u64,
}

/// One `(bytes_downloaded_so_far_for_this_resource, total_bytes_for_this_resource)` progress
/// update from [`download_one`] — see that function's own docs on why this is delivered via
/// channel rather than a direct `JobRegistry` write or an async callback.
pub type ProgressUpdate = (u64, Option<u64>);

/// Performs one real streaming HTTP download of `req`, respecting `manager`'s per-domain
/// concurrency/rate-limit rules (resolved against `req.url`'s hostname), sending a
/// [`ProgressUpdate`] down `progress_tx` as it proceeds (spec FR-001/FR-002), and writing the
/// result to a fresh staging file inside `staging_dir` (mirroring `upload.rs::upload_archive`'s
/// own staging-path convention — the caller still owns moving it into `archive_dir`/calling
/// `ingest_file`, exactly as that handler does for an uploaded file).
///
/// **Why a channel instead of writing straight to `JobRegistry` (or an async callback)**: a single
/// [`DownloadRequest`] is only ever *one* of possibly several resources making up one download job
/// (spec FR-003 — Pixiv-style multi-page downloads). If this function wrote directly to a fixed
/// `job_id`, the second resource's `downloaded`/`total` would overwrite the first's, making the
/// job's reported progress jump backwards every time a new resource starts — a real bug caught
/// during this feature's own implementation (multi-resource combined-progress verification,
/// US1/T019) before it ever shipped. An `async`/`AsyncFnMut` callback parameter was tried first
/// instead of a channel, but hit a real, current rustc limitation: an `AsyncFnMut` closure
/// capturing any borrowed reference, called from inside a function whose caller wraps the whole
/// thing in `tokio::spawn` (which requires the spawned future to be `'static`+`Send` for *every*
/// possible lifetime, not just the one the closure was actually created with), fails to compile
/// with "implementation of `Send` is not general enough" regardless of which reference is
/// captured — confirmed directly by trying it (`AppState`, `PluginInfo`, `PluginPool`, `&str` all
/// individually triggered the identical error). An unbounded mpsc channel sidesteps this
/// entirely: the receiver end is a plain owned value the caller can consume in its own
/// independent `.await` loop, with no closure/higher-ranked-lifetime interaction with
/// `tokio::spawn` at all. `plugins.rs::run_managed_downloads` owns combining multiple resources'
/// byte counts (received from each resource's own channel) into one job-wide total; this function
/// only ever reports *this one resource's* own progress.
///
/// The concurrency permit is captured once, at the top of this function, before the real request
/// is sent — FR-016 snapshot-at-start-time: whatever `rules` resolves `max_concurrent` to at this
/// moment governs this download for its entire duration. The *rate* limit (`max_bytes_per_sec`),
/// by contrast, is re-resolved on every chunk read via `resolver` — a user clearing or changing a
/// rate cap takes effect mid-transfer at the next chunk boundary, not only on downloads started
/// after the change.
///
/// `resume_key`, when `Some` (issue #88 — `plugins.rs::run_managed_downloads` derives it from a
/// `"{queue_item_id}-{resource_index}"` pair, stable across a retry/restart for the same queue
/// item and resource, so a second attempt can find the first attempt's own partial bytes), makes
/// this call **resumable**: if a partial file from a previous attempt at the same `resume_key`
/// already exists on disk, this sends `Range: bytes=<len>-` (plus `If-Range` with whatever strong
/// validator the original response supplied) instead of a fresh unconditional request, and — only
/// if the server actually honors it with `206 Partial Content` — appends onto the existing bytes
/// rather than re-downloading them. `None` (a one-off, non-queue-tracked download, or a test) skips
/// all of this and behaves exactly as before: a fresh randomly-named staging file every call, no
/// disk state consulted or left behind. See [`ResumeMetadata`]/[`ResumePaths`] for the on-disk
/// sidecar shape, and `download_one_inner`'s own docs for exactly how a server's response is
/// interpreted (206 vs 200 vs anything else).
#[allow(clippy::too_many_arguments)]
pub async fn download_one(
    manager: &DownloadManager,
    rules: &[DomainRule],
    req: &DownloadRequest,
    resolver: &dyn RateResolver,
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
    staging_dir: &Path,
    cancel: &tokio_util::sync::CancellationToken,
    on_permit_acquired: &(dyn Fn() + Send + Sync),
    resume_key: Option<&str>,
) -> Result<DownloadedFile, DownloadError> {
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let parsed_url =
        url::Url::parse(&req.url).map_err(|e| DownloadError::InvalidUrl(req.url.clone(), e))?;
    let hostname = parsed_url.host_str().unwrap_or("").to_string();

    // FR-016 snapshot: acquired once, held for this whole function's lifetime (dropped when this
    // function returns/errors, at the very end of the transfer — see `_permit` below).
    let permit = manager.acquire(&hostname, rules, cancel).await?;
    // Fires the instant a concurrency permit is actually in hand — the caller
    // (`plugins.rs::run_managed_downloads`) uses this to flip the queue item's displayed state
    // from `Waiting` to `Downloading` at the one moment that's actually true, rather than at task
    // spawn time (which could be arbitrarily long before a busy domain's semaphore frees up).
    on_permit_acquired();
    // Each download gets its own token bucket rather than sharing one per matched rule pattern —
    // a shared pool let concurrent downloads under the same domain rule "borrow" each other's
    // unused headroom, so a single download's observed speed would spike above the configured cap
    // for several seconds until sibling downloads caught up and started actually competing for
    // tokens (issue #41). Suffixing the permit's own `matched_pattern` with a per-download UUID
    // keeps `RateLimiterMap`'s rate value resolution (still driven by the pattern's rule) while
    // giving every concurrent download an independent, always-fully-enforced quota.
    let rate_limit_key = format!("{}#{}", permit.matched_pattern, uuid::Uuid::new_v4());

    // `RateLimiterMap` never evicts entries on its own — this download's own unique-keyed entry
    // must be removed once it's done, on every exit path (success, HTTP/IO/network error, or a
    // user-requested Stop), or the map grows by one permanently-dead entry per download for the
    // life of the process. `?`/early-return inside `download_one_inner` below all funnel through
    // this one cleanup point rather than needing an async `Drop` (which Rust doesn't support).
    let result = download_one_inner(
        manager,
        req,
        resolver,
        progress_tx,
        staging_dir,
        cancel,
        &parsed_url,
        &rate_limit_key,
        resume_key,
    )
    .await;
    manager.rate_limiters().release(&rate_limit_key).await;
    result
}

/// What was learned by inspecting any existing partial file for `resume_key` before sending the
/// real request — kept separate from the request-building code below so the "is there something
/// to resume, and is it still trustworthy" decision reads as one linear check.
enum ResumeState {
    /// No `resume_key` given, or nothing on disk yet — send a plain unconditional request exactly
    /// as `download_one` always has.
    Fresh,
    /// A partial file exists and its sidecar metadata parsed successfully — attempt a `Range`
    /// resume with `If-Range` validation. Doesn't carry its own `ResumePaths` — the caller
    /// (`download_one_inner`) already computes one unconditionally from `resume_key` up front and
    /// uses that single copy consistently everywhere, so duplicating it here would just be a
    /// second copy of the same two paths that could (and during this feature's own development,
    /// briefly did) drift out of sync with which one call sites actually read.
    Resumable {
        existing_len: u64,
        meta: ResumeMetadata,
    },
}

async fn inspect_resume_state(staging_dir: &Path, resume_key: Option<&str>) -> ResumeState {
    let Some(resume_key) = resume_key else {
        return ResumeState::Fresh;
    };
    let paths = ResumePaths::new(staging_dir, resume_key);
    let Ok(part_meta) = tokio::fs::metadata(&paths.part_path).await else {
        return ResumeState::Fresh;
    };
    let Ok(raw_meta) = tokio::fs::read(&paths.meta_path).await else {
        // A `.part` file with no matching sidecar isn't trustworthy — nothing to validate against,
        // so treat it the same as if nothing existed rather than guessing. It gets overwritten
        // fresh below (the caller opens `part_path` with `create(true).truncate(true)` on the
        // `Fresh` path when `resume_key` is `Some`, exactly what "start clean" needs).
        return ResumeState::Fresh;
    };
    let Ok(meta) = serde_json::from_slice::<ResumeMetadata>(&raw_meta) else {
        return ResumeState::Fresh;
    };
    ResumeState::Resumable {
        existing_len: part_meta.len(),
        meta,
    }
}

#[allow(clippy::too_many_arguments)]
async fn download_one_inner(
    manager: &DownloadManager,
    req: &DownloadRequest,
    resolver: &dyn RateResolver,
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
    staging_dir: &Path,
    cancel: &tokio_util::sync::CancellationToken,
    parsed_url: &url::Url,
    rate_limit_key: &str,
    resume_key: Option<&str>,
) -> Result<DownloadedFile, DownloadError> {
    let method = match &req.method {
        None => Method::GET,
        Some(m) => {
            Method::from_bytes(m.as_bytes()).map_err(|_| DownloadError::InvalidMethod(m.clone()))?
        }
    };

    // Whenever `resume_key` is given, this call always writes to the same fixed `.part`/
    // `.meta.json` pair for its entire duration — whether this is the very first attempt (nothing
    // to resume from yet, but still worth writing to a findable path for a *future* retry), a
    // genuine resume, or a resume that gets rejected by the server and falls back to a fresh
    // download (in which case the old bytes are discarded and this same path is overwritten
    // clean). One `Option<ResumePaths>` computed once up front, used consistently everywhere below
    // — no second, only-sometimes-set variable to keep in sync with it.
    let resume_paths = resume_key.map(|key| ResumePaths::new(staging_dir, key));
    let resume_state = inspect_resume_state(staging_dir, resume_key).await;

    let client = reqwest::Client::new();
    let mut request = client.request(method, parsed_url.clone());
    for (name, value) in &req.headers {
        request = request.header(name, value);
    }
    if let ResumeState::Resumable {
        existing_len, meta, ..
    } = &resume_state
    {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing_len}-"));
        // `If-Range` makes the resume conditional on the server's own opinion of whether the
        // content is unchanged — if it doesn't match (or the server doesn't understand `If-Range`
        // at all, which per RFC 9110 §13.1.5 means it just ignores `Range` too), the server sends
        // a normal `200` with the *full* body, which the `200` branch below treats as "resume
        // rejected, start clean" rather than corrupting the partial file by appending mismatched
        // bytes onto it. Strong validator only (`ETag` preferred over `Last-Modified`) — a weak
        // validator (`W/"..."`) is explicitly disallowed for `If-Range` by the same RFC section.
        if let Some(etag) = &meta.etag {
            request = request.header(reqwest::header::IF_RANGE, etag);
        } else if let Some(last_modified) = &meta.last_modified {
            request = request.header(reqwest::header::IF_RANGE, last_modified);
        }
    }

    let response = request.send().await.map_err(DownloadError::Request)?;

    // Three-way split on status, not just `is_success()` — `206` needs different handling from
    // `200` (append vs. overwrite), and both are "successful" by HTTP's own definition.
    let status = response.status();
    if !status.is_success() && status != reqwest::StatusCode::PARTIAL_CONTENT {
        return Err(DownloadError::HttpStatus(req.url.clone(), status));
    }

    let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT;
    if !resumed {
        // Either there was nothing to resume from, or the server ignored `Range`/`If-Range` (no
        // resume support, or the underlying content changed since the partial file was written)
        // and sent the full body fresh instead — either way, any old partial bytes at this
        // `resume_key`'s path are no longer safe to build on top of. Discarding here (rather than
        // risking silently appending mismatched content onto them) is what makes overwriting via
        // `File::create` below always correct regardless of which of those two cases this was.
        if let Some(paths) = &resume_paths {
            paths.remove_both().await;
        }
    }

    // `content_length()` on a `206` response is the *remaining* bytes per RFC 9110, not the whole
    // resource — added onto what was already on disk so `total_bytes` (used for the progress
    // bar's percentage) reflects the true whole-file size the same way a fresh `200` download's
    // `Content-Length` already does, not just what's left to fetch this call.
    let existing_len = match &resume_state {
        ResumeState::Resumable { existing_len, .. } if resumed => *existing_len,
        _ => 0,
    };
    let total_bytes = response.content_length().map(|n| n + existing_len);

    // A resumed `206` isn't guaranteed to repeat `Content-Disposition` (some servers only send it
    // on the initial full response) — the filename recorded in the sidecar at the *original* `200`
    // is authoritative for the whole download, not re-derived here. A fresh (non-resumed) request
    // resolves it the normal way and — when resumable — persists it into a new sidecar below for
    // any future resume attempt to read back.
    let filename = if resumed {
        match &resume_state {
            ResumeState::Resumable { meta, .. } => meta.filename.clone(),
            ResumeState::Fresh => {
                unreachable!("a 206 response only ever follows a Range request, which is only ever sent when resume_state was Resumable")
            }
        }
    } else {
        resolve_filename(&response, req.filename_hint.as_deref(), parsed_url)
    };

    // Carries the resolved filename's own extension (when it has one) onto the staging file
    // itself — `lanrurugi_scanner::pipeline::catalogue_new_archive` reads *this* path (not the
    // later-renamed final destination) to compute `pagecount` via `archive_format::list_pages`,
    // which is a pure extension-based format check. A bare `download-<uuid>` with no extension at
    // all (the previous behavior, regardless of what `filename` resolved to) always fails that
    // check, so `pagecount` silently and permanently stuck at 0 for every archive ever ingested
    // through this download path, no matter how sensible `filename`/`filename_hint` was — the
    // final destination file (after `ingest_downloaded_file`'s rename) does end up correctly
    // named/extensioned, but nothing ever re-runs page-counting against it afterward. Confirmed
    // live: two real, fully-downloaded, genuinely-readable `.zip` archives (one via a plugin fix
    // that finally gave it a real filename_hint, one that already had a normal-looking name) both
    // still ended up permanently `pagecount: 0` before this fix, for exactly this reason.
    //
    // A resumable call (`resume_paths.is_some()`) writes directly to `ResumePaths::part_path` — a
    // fixed, predictable name derived from `resume_key`, deliberately *not* extensioned so a later
    // resume attempt can find it without needing to already know the extension — for the whole
    // transfer, only renamed to a properly-extensioned name once it's actually complete (see the
    // end of this function). That predictable path is what makes it findable again by a retry
    // after a crash/Stop/restart. A non-resumable call keeps the previous behavior exactly: a
    // fresh randomly-named file, extensioned upfront, with nothing left behind on any exit path.
    let staging_path = match &resume_paths {
        Some(paths) => paths.part_path.clone(),
        None => match Path::new(&filename).extension().and_then(|e| e.to_str()) {
            Some(ext) => {
                staging_dir.join(format!("download-{}.{ext}", uuid::Uuid::new_v4().simple()))
            }
            None => staging_dir.join(format!("download-{}", uuid::Uuid::new_v4().simple())),
        },
    };

    // Persists (or re-persists, harmlessly idempotent) the sidecar *before* any bytes are written —
    // if the process crashes/is killed mid-transfer, the sidecar must already be on disk for the
    // next attempt to trust the partial `.part` file it left behind; writing it after the transfer
    // completes would defeat the entire point (nothing to validate against on the crash path that
    // actually needs it). Only meaningful (and only written) for a *fresh* write — a genuine resume
    // reuses the sidecar that's already there (it describes the same underlying content, verified
    // via `If-Range` above), so overwriting it here would be redundant, not wrong, but skipped
    // anyway for clarity.
    if !resumed {
        if let Some(paths) = &resume_paths {
            let meta = ResumeMetadata {
                url: req.url.clone(),
                etag: response
                    .headers()
                    .get(reqwest::header::ETAG)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
                last_modified: response
                    .headers()
                    .get(reqwest::header::LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
                filename: filename.clone(),
            };
            if let Ok(raw) = serde_json::to_vec(&meta) {
                let _ = tokio::fs::write(&paths.meta_path, raw).await;
            }
        }
    }

    let mut file = if resumed {
        // Append, not truncate — the whole point of a `206` resume is to build on the bytes
        // already there. `.seek(End)` isn't strictly needed for pure appends on most platforms
        // when opened with `.append(true)`, but is cheap and makes the intent explicit/portable.
        let mut f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&staging_path)
            .await
            .map_err(|e| DownloadError::Io(staging_path.clone(), e))?;
        let _ = f.seek(std::io::SeekFrom::End(0)).await;
        f
    } else {
        tokio::fs::File::create(&staging_path)
            .await
            .map_err(|e| DownloadError::Io(staging_path.clone(), e))?
    };

    let mut downloaded: u64 = existing_len;
    let mut stream = response.bytes_stream();
    // Throttled so a fast local-network transfer doesn't send a `ProgressUpdate` on every single
    // chunk (a real chunk read is routinely a few KB to tens of KB — at typical transfer speeds
    // that's easily hundreds of updates/sec with no user-visible benefit over one every ~200ms,
    // which is still comfortably under SC-001's "at least 3 distinct intermediate states" bar for
    // anything but a near-instantaneous download).
    let mut last_reported = std::time::Instant::now();
    const PROGRESS_REPORT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);

    loop {
        // Raced against the stream on every iteration (rather than checked once before the loop)
        // so a Stop click lands promptly no matter how large the transfer or how long a single
        // chunk takes to arrive — cooperative cancellation, not `AbortHandle`-based, specifically
        // so this branch can run the exact same partial-file cleanup the network-error branch
        // below already has, rather than leaving an untraceable UUID-named orphan in
        // `staging_dir` (see `AppState::download_cancellations`'s own docs for why).
        //
        // A resumable call (`resume_paths.is_some()`) deliberately does NOT delete `staging_path`/
        // the sidecar on cancellation or a network error — that partial file (plus its sidecar) is
        // exactly what makes the *next* attempt resumable; deleting it here would defeat the whole
        // feature. A non-resumable call keeps the previous behavior exactly: clean up the orphaned
        // partial file on every exit path, since there's no later retry that could ever find/use
        // it.
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                drop(file);
                if resume_paths.is_none() {
                    let _ = tokio::fs::remove_file(&staging_path).await;
                }
                return Err(DownloadError::Cancelled);
            }
            chunk = stream.next() => match chunk {
                Some(chunk) => chunk,
                None => break,
            },
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(e) => {
                // A network error partway through leaves a real, partially-written file at
                // `staging_path` — clean it up rather than leaking it when this call isn't
                // resumable (spec FR-004's "no half-cataloged archive" extends to "no
                // half-downloaded litter on disk" too); a resumable call keeps it on purpose (see
                // this loop's own docs above).
                drop(file);
                if resume_paths.is_none() {
                    let _ = tokio::fs::remove_file(&staging_path).await;
                }
                return Err(DownloadError::Request(e));
            }
        };
        manager
            .rate_limiters()
            .throttle(
                rate_limit_key,
                resolver.resolve(parsed_url.host_str().unwrap_or("")).await,
                chunk.len() as u64,
            )
            .await;
        if let Err(e) = file.write_all(&chunk).await {
            drop(file);
            if resume_paths.is_none() {
                let _ = tokio::fs::remove_file(&staging_path).await;
            }
            return Err(DownloadError::Io(staging_path.clone(), e));
        }
        downloaded += chunk.len() as u64;

        if last_reported.elapsed() >= PROGRESS_REPORT_INTERVAL {
            // A closed receiver (caller stopped listening) is not a download failure — the
            // transfer itself is still succeeding, only its progress reporting is now a no-op.
            let _ = progress_tx.send((downloaded, total_bytes));
            last_reported = std::time::Instant::now();
        }
    }
    // Final report always fires regardless of the throttle interval, so the caller's last-known
    // progress reflects the true final byte count rather than whatever the last periodic sample
    // happened to be.
    let _ = progress_tx.send((downloaded, total_bytes));

    file.flush()
        .await
        .map_err(|e| DownloadError::Io(staging_path.clone(), e))?;
    drop(file);

    // Download genuinely complete — a resumable call's sidecar has done its job (nothing left to
    // resume) and its `.part` file gets the same extension-carrying rename a non-resumable
    // download's staging file already had from the start, so downstream page-counting sees the
    // same shape either way.
    let final_path = match &resume_paths {
        Some(paths) => {
            let extensioned = match Path::new(&filename).extension().and_then(|e| e.to_str()) {
                Some(ext) => {
                    staging_dir.join(format!("download-{}.{ext}", uuid::Uuid::new_v4().simple()))
                }
                None => staging_dir.join(format!("download-{}", uuid::Uuid::new_v4().simple())),
            };
            if tokio::fs::rename(&staging_path, &extensioned)
                .await
                .is_err()
            {
                tokio::fs::copy(&staging_path, &extensioned)
                    .await
                    .map_err(|e| DownloadError::Io(extensioned.clone(), e))?;
                let _ = tokio::fs::remove_file(&staging_path).await;
            }
            let _ = tokio::fs::remove_file(&paths.meta_path).await;
            extensioned
        }
        None => staging_path,
    };

    Ok(DownloadedFile {
        path: final_path,
        filename,
        bytes_downloaded: downloaded,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

    /// Spins up a real local HTTP server (`axum`, already a workspace dependency — no new mock-
    /// server crate needed) so this test exercises the actual `reqwest` streaming path end to
    /// end, not a hand-rolled stand-in for one.
    async fn spawn_test_server(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn download_one_writes_body_and_reports_final_progress() {
        let body = b"hello world, this is a test archive body".to_vec();
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(move || {
                let body = body.clone();
                async move {
                    (
                        [(
                            reqwest::header::CONTENT_DISPOSITION.as_str(),
                            "attachment; filename=\"real-name.zip\"",
                        )],
                        body,
                    )
                }
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir = std::env::temp_dir();

        let req = DownloadRequest {
            url: format!("{base_url}/archive.zip"),
            method: None,
            headers: vec![],
            filename_hint: Some("hint.zip".to_string()),
        };

        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            None,
        )
        .await
        .expect("download must succeed against a real local server");

        assert_eq!(
            result.filename, "real-name.zip",
            "Content-Disposition must win over filename_hint"
        );
        let written = tokio::fs::read(&result.path).await.unwrap();
        assert_eq!(written, b"hello world, this is a test archive body");
        assert_eq!(result.bytes_downloaded, written.len() as u64);

        let mut last_progress = (0u64, None::<u64>);
        while let Ok(update) = progress_rx.try_recv() {
            last_progress = update;
        }
        assert_eq!(
            last_progress.0,
            written.len() as u64,
            "the final progress update must reflect the true final byte count"
        );

        tokio::fs::remove_file(&result.path).await.ok();
        server.abort();
    }

    /// Opt-in, real-server regression test for a non-ASCII UTF-8 `Content-Disposition` filename
    /// (some real-world servers send raw UTF-8 bytes directly in a plain `filename="..."`
    /// parameter — not RFC 6266-compliant, that requires `filename*=UTF-8''<percent-encoded>` —
    /// which is what made `content_disposition_filename` give up and fall through to the
    /// `filename_hint`/URL-derived fallback before this fix). Both the URL and the exact expected
    /// filename come from `.env.local` (gitignored — see `.env.example`) rather than being
    /// hardcoded in source, since a real download target/filename shouldn't live in this repo's
    /// history. Skipped (not failed) when unset, since it depends on an external server that
    /// isn't this repo's to guarantee uptime/content for.
    #[tokio::test]
    async fn download_one_decodes_a_non_ascii_utf8_content_disposition_filename() {
        // `.env.local`'s own template ships these as present-but-empty (`KEY=`) rather than
        // absent, so an empty string must be treated the same as unset — `std::env::var` alone
        // would otherwise return `Ok("")` and this test would try to download from `""`.
        let url = std::env::var("TEST_REAL_DOWNLOAD_URL").unwrap_or_default();
        let expected_filename =
            std::env::var("TEST_REAL_DOWNLOAD_EXPECTED_FILENAME").unwrap_or_default();
        if url.is_empty() || expected_filename.is_empty() {
            eprintln!(
                "skipping: set TEST_REAL_DOWNLOAD_URL and TEST_REAL_DOWNLOAD_EXPECTED_FILENAME \
                 in .env.local to run this test against a real server"
            );
            return;
        }

        let manager = DownloadManager::new();
        let staging_dir = std::env::temp_dir();

        let req = DownloadRequest {
            url,
            method: None,
            headers: vec![],
            filename_hint: Some("fallback-hint-name.zip".to_string()),
        };

        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            None,
        )
        .await
        .expect("download must succeed against the real configured server");

        assert_eq!(
            result.filename, expected_filename,
            "the real UTF-8 filename must be used, not the filename_hint fallback"
        );

        tokio::fs::remove_file(&result.path).await.ok();
    }

    #[tokio::test]
    async fn download_one_reports_http_error_status_without_writing_a_file() {
        let router = axum::Router::new().route(
            "/missing.zip",
            axum::routing::get(|| async { axum::http::StatusCode::NOT_FOUND }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir = std::env::temp_dir();

        let req = DownloadRequest {
            url: format!("{base_url}/missing.zip"),
            method: None,
            headers: vec![],
            filename_hint: None,
        };

        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let err = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            None,
        )
        .await
        .expect_err("a 404 response must surface as an error, not a successful empty file");
        assert!(matches!(err, DownloadError::HttpStatus(_, status) if status == 404));
        assert_eq!(
            manager.rate_limiters().tracked_entry_count().await,
            0,
            "an errored download must still release its per-download rate-limiter entry"
        );

        server.abort();
    }

    /// Issue #41: `download_one` now gives each download its own uniquely-keyed rate-limiter
    /// entry rather than sharing one per matched domain-rule pattern. That only avoids a permanent
    /// per-download memory leak if the entry is actually released once the download finishes —
    /// proven here by checking `RateLimiterMap`'s tracked entry count is back to zero after a
    /// successful download that was actually throttled (so an entry was guaranteed to be created).
    #[tokio::test]
    async fn download_one_releases_its_rate_limiter_entry_after_a_throttled_download() {
        let body = vec![b'x'; 500];
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(move || {
                let body = body.clone();
                async move { body }
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir = std::env::temp_dir();
        let rules = vec![DomainRule {
            pattern: None,
            max_concurrent: None,
            max_bytes_per_sec: Some(1_000_000),
            description: None,
        }];

        let req = DownloadRequest {
            url: format!("{base_url}/archive.zip"),
            method: None,
            headers: vec![],
            filename_hint: Some("archive.zip".to_string()),
        };

        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = download_one(
            &manager,
            &rules,
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            None,
        )
        .await
        .expect("download must succeed");

        assert_eq!(
            manager.rate_limiters().tracked_entry_count().await,
            0,
            "a completed download must release its per-download rate-limiter entry, not leak it"
        );

        let _ = tokio::fs::remove_file(&result.path).await;
        server.abort();
    }

    #[tokio::test]
    async fn download_one_returns_cancelled_and_writes_no_staging_file_when_pre_cancelled() {
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(|| async { b"hello world".to_vec() }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir = std::env::temp_dir();

        let req = DownloadRequest {
            url: format!("{base_url}/archive.zip"),
            method: None,
            headers: vec![],
            filename_hint: None,
        };

        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();

        // `staging_dir` is the shared OS temp dir, not a private scratch directory — other tests
        // in this same `cargo test` run write their own unrelated files there concurrently, so a
        // raw whole-directory before/after diff is flaky (confirmed: failed once against a sibling
        // bundle test's own `lrr-bundle-test-*` file landing mid-run). Scoped to `download-*`,
        // matching this function's own real staging-filename convention (see `download_one`'s
        // `staging_path` construction above) — only files *this* code path could have written.
        fn own_staging_files(
            dir: &std::path::Path,
        ) -> std::collections::HashSet<std::path::PathBuf> {
            std::fs::read_dir(dir)
                .unwrap()
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("download-"))
                })
                .collect()
        }

        let before = own_staging_files(&staging_dir);

        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let err = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &cancel,
            &|| {},
            None,
        )
        .await
        .expect_err("a pre-cancelled token must short-circuit before any request is sent");
        assert!(matches!(err, DownloadError::Cancelled));

        let after = own_staging_files(&staging_dir);
        assert_eq!(
            before, after,
            "no staging file should be created for a download that never started"
        );

        server.abort();
    }

    #[tokio::test]
    async fn download_one_sends_custom_headers() {
        let router = axum::Router::new().route(
            "/page.jpg",
            axum::routing::get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(
                    headers.get("referer").map(|v| v.to_str().unwrap()),
                    Some("https://example.com/artwork/123")
                );
                b"fake image bytes".to_vec()
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir = std::env::temp_dir();

        let req = DownloadRequest {
            url: format!("{base_url}/page.jpg"),
            method: None,
            headers: vec![(
                "Referer".to_string(),
                "https://example.com/artwork/123".to_string(),
            )],
            filename_hint: None,
        };

        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            None,
        )
        .await
        .expect("download with a custom header must succeed");
        tokio::fs::remove_file(&result.path).await.ok();
        server.abort();
    }

    /// The bug this multi-resource aggregation design exists to prevent (spec FR-003, caught
    /// during this feature's own implementation before shipping): if `download_one` wrote
    /// straight to a fixed job's progress fields instead of reporting only its own resource's
    /// byte count, a second resource's byte count would silently overwrite the first's, making a
    /// multi-resource job's reported progress jump backwards every time a new resource starts
    /// downloading.
    #[tokio::test]
    async fn multiple_resources_combine_into_one_running_total_via_the_progress_channel() {
        let body_a = vec![b'a'; 100];
        let body_b = vec![b'b'; 50];
        let router = axum::Router::new()
            .route(
                "/a.jpg",
                axum::routing::get({
                    let body = body_a.clone();
                    move || {
                        let body = body.clone();
                        async move { body }
                    }
                }),
            )
            .route(
                "/b.jpg",
                axum::routing::get({
                    let body = body_b.clone();
                    move || {
                        let body = body.clone();
                        async move { body }
                    }
                }),
            );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir = std::env::temp_dir();

        // Mirrors exactly what `plugins.rs::run_managed_downloads` does: a running base offset,
        // bumped by each resource's own final byte count once it completes, added on top of each
        // resource's own channel updates — never overwritten.
        let mut combined_downloaded = 0u64;
        let mut last_seen = 0u64;
        let mut paths = Vec::new();

        for path in ["/a.jpg", "/b.jpg"] {
            let req = DownloadRequest {
                url: format!("{base_url}{path}"),
                method: None,
                headers: vec![],
                filename_hint: None,
            };
            let base = combined_downloaded;
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel();
            let result = download_one(
                &manager,
                &[],
                &req,
                &NoopRateResolver,
                progress_tx,
                &staging_dir,
                &tokio_util::sync::CancellationToken::new(),
                &|| {},
                None,
            )
            .await
            .unwrap();
            while let Ok((downloaded, _total)) = progress_rx.try_recv() {
                last_seen = base + downloaded;
            }
            combined_downloaded += result.bytes_downloaded;
            paths.push(result.path);
        }

        assert_eq!(
            last_seen,
            (body_a.len() + body_b.len()) as u64,
            "the combined total after both resources must be their sum, not the last resource's \
             own count alone"
        );
        assert_eq!(combined_downloaded, 150);

        for path in paths {
            tokio::fs::remove_file(&path).await.ok();
        }
        server.abort();
    }

    /// spec FR-004's "no half-cataloged archive on failure" extends to disk hygiene too: a
    /// network error partway through a transfer must not leave the partially-written staging
    /// file behind. Simulated with a raw TCP listener that advertises a `Content-Length` larger
    /// than the bytes it actually sends before closing the connection — a real, reliable way to
    /// make `reqwest`'s own body stream surface a genuine mid-transfer error (an axum handler
    /// can't easily be made to do this, since it manages `Content-Length`/connection lifecycle
    /// itself).
    #[tokio::test]
    async fn a_mid_transfer_network_error_removes_the_partial_staging_file() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            // Claim 1000 bytes, send only 10, then drop the connection — `reqwest` must surface
            // this as a stream read error partway through, not a clean end-of-body.
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n0123456789")
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        });

        let manager = DownloadManager::new();
        // A dedicated, empty subdirectory (not the shared process-wide temp dir, which other
        // tests running concurrently in the same process also write into) so this test can just
        // assert "nothing left behind at all" without needing a fragile before/after diff.
        let staging_dir = std::env::temp_dir().join(format!("lrr-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&staging_dir).await.unwrap();
        let req = DownloadRequest {
            url: format!("http://{addr}/archive.zip"),
            method: None,
            headers: vec![],
            filename_hint: None,
        };
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();

        let err = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            None,
        )
        .await
        .expect_err("a truncated body must surface as a real error, not a short-but-clean file");
        assert!(matches!(err, DownloadError::Request(_)));

        let remaining: Vec<_> = std::fs::read_dir(&staging_dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        assert!(
            remaining.is_empty(),
            "the partially-written staging file must be removed on a mid-transfer error, found: \
             {remaining:?}"
        );

        tokio::fs::remove_dir_all(&staging_dir).await.ok();
        server.abort();
    }

    /// Issue #88's core case: a transfer cancelled partway through, then retried with the same
    /// `resume_key`, must pick up from the byte it left off at (a real `Range`/`206` round trip
    /// against a genuine local server that supports it), not re-download from zero — and the final
    /// file's bytes must be byte-for-byte identical to what a single uninterrupted download would
    /// have produced.
    ///
    /// `multi_thread` flavor (not this file's usual `#[tokio::test]` default of a single-thread
    /// current-thread runtime) — the local axum server task, the cancel-timer task, and the client
    /// call under test all need to make genuinely concurrent progress against each other for the
    /// cancellation to land mid-stream rather than either finishing instantly (no real interruption
    /// to test) or never getting a chance to run at all before the client call completes.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_cancelled_download_resumes_from_where_it_left_off_on_retry() {
        let full_body: Vec<u8> = (0..5000u32).map(|n| (n % 256) as u8).collect();
        let full_body_for_route = full_body.clone();
        // Signaled by the route handler below once it's handed a few chunks to the response
        // stream — a real synchronization point the test waits on before cancelling (see this
        // test's own cancellation comment further down for why a guessed delay isn't good enough,
        // and why this waits for a few chunks rather than just the first), independent of
        // `progress_tx`'s own throttled reporting interval (too coarse to ever fire mid-transfer
        // for a body this small — `download_one`'s internal `PROGRESS_REPORT_INTERVAL` is 200ms,
        // longer than this whole 5000-byte transfer takes).
        let early_chunk_sent = std::sync::Arc::new(tokio::sync::Notify::new());
        let early_chunk_sent_for_route = early_chunk_sent.clone();
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(move |headers: axum::http::HeaderMap| {
                let full_body = full_body_for_route.clone();
                let early_chunk_sent = early_chunk_sent_for_route.clone();
                async move {
                    // A real, minimal `Range: bytes=N-` responder — enough to exercise the
                    // real client-side `Range`/`If-Range`/`206` handling in `download_one`
                    // without pulling in a dedicated static-file-serving crate just for this
                    // test. The body is streamed out in small, deliberately-delayed chunks
                    // (rather than sent as one instant blob) so a cancellation racing a short
                    // timeout reliably lands mid-transfer instead of before the response even
                    // starts arriving — a real, confirmed-flaky failure mode when the whole
                    // 5000-byte body was small enough to arrive in a single instant read.
                    let etag = "\"test-etag-v1\"";
                    let (status, body_bytes, extra_headers): (_, Vec<u8>, Vec<(&str, String)>) =
                        if let Some(range) = headers.get(reqwest::header::RANGE) {
                            let range_str = range.to_str().unwrap();
                            let start: usize = range_str
                                .trim_start_matches("bytes=")
                                .trim_end_matches('-')
                                .parse()
                                .unwrap();
                            let body = full_body[start..].to_vec();
                            let content_range = format!(
                                "bytes {start}-{}/{}",
                                full_body.len() - 1,
                                full_body.len()
                            );
                            (
                                axum::http::StatusCode::PARTIAL_CONTENT,
                                body,
                                vec![(reqwest::header::CONTENT_RANGE.as_str(), content_range)],
                            )
                        } else {
                            (axum::http::StatusCode::OK, full_body, vec![])
                        };
                    let chunk_stream = futures_util::stream::iter(
                        body_bytes
                            .chunks(200)
                            .map(|c| Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(c)))
                            .collect::<Vec<_>>()
                            .into_iter()
                            .enumerate(),
                    )
                    .then(move |(i, chunk)| {
                        let early_chunk_sent = early_chunk_sent.clone();
                        async move {
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                            // Notified after the *third* chunk (not the first) — signaling right
                            // as the first chunk leaves the server races the client's own
                            // read-then-write of that chunk (the network round trip plus
                            // `download_one`'s own `tokio::select!` scheduling isn't instant),
                            // confirmed live as a real failure mode (the cancellation landing
                            // before any bytes were actually written to disk). Three chunks' worth
                            // of head start (15ms of the server's own per-chunk pacing) reliably
                            // gives the client time to have written real bytes by the time this
                            // fires, while still leaving most of the 25-chunk transfer left to
                            // interrupt.
                            if i == 2 {
                                early_chunk_sent.notify_one();
                            }
                            chunk
                        }
                    });
                    let mut response = axum::body::Body::from_stream(chunk_stream).into_response();
                    *response.status_mut() = status;
                    response
                        .headers_mut()
                        .insert(reqwest::header::ETAG, etag.parse().unwrap());
                    for (name, value) in extra_headers {
                        response.headers_mut().insert(
                            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                            value.parse().unwrap(),
                        );
                    }
                    response
                }
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir =
            std::env::temp_dir().join(format!("lrr-resume-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&staging_dir).await.unwrap();
        let resume_key = format!("test-{}", uuid::Uuid::new_v4());

        let req = DownloadRequest {
            url: format!("{base_url}/archive.zip"),
            method: None,
            headers: vec![],
            filename_hint: Some("archive.zip".to_string()),
        };

        // First attempt: cancel partway through. Rather than guessing a wall-clock delay long
        // enough to land after the first chunk but short enough to land before the whole 5000-byte
        // body finishes (a real, confirmed-flaky failure mode under CI's own scheduling jitter —
        // too short cancels before any bytes arrive at all, too long lets the transfer complete
        // uninterrupted; `download_one`'s own `progress_tx` reporting is too coarse-grained to use
        // instead — its 200ms throttle interval is longer than this whole transfer takes, so it
        // never fires until the transfer's already done), this waits on `early_chunk_sent` — a
        // real synchronization signal the route handler above fires the instant it hands its first
        // chunk to the response stream — before cancelling, guaranteeing the cancellation always
        // lands after real bytes have started arriving but (per the server's own 5ms-per-chunk
        // pacing across 25 chunks) still well before the transfer completes.
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_after = cancel.clone();
        let early_chunk_sent_waiter = early_chunk_sent.clone();
        tokio::spawn(async move {
            early_chunk_sent_waiter.notified().await;
            cancel_after.cancel();
        });
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel::<ProgressUpdate>();
        let first_attempt = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &cancel,
            &|| {},
            Some(&resume_key),
        )
        .await;
        assert!(
            matches!(first_attempt, Err(DownloadError::Cancelled)),
            "expected the first attempt to be cancelled, got {first_attempt:?}"
        );

        // The resumable `.part`/sidecar must have survived cancellation (this is the entire point
        // — a non-resumable call would have deleted it, but this one must not).
        let resume_paths = ResumePaths::new(&staging_dir, &resume_key);
        let partial_len_after_cancel = tokio::fs::metadata(&resume_paths.part_path)
            .await
            .expect("the partial .part file must survive a cancelled resumable download")
            .len();
        assert!(
            tokio::fs::try_exists(&resume_paths.meta_path)
                .await
                .unwrap(),
            "the sidecar metadata must survive a cancelled resumable download"
        );

        // Second attempt, same `resume_key`, no cancellation this time — must actually resume
        // (not silently redownload from zero) and produce the exact full body.
        let (progress_tx2, _progress_rx2) = tokio::sync::mpsc::unbounded_channel();
        let second_attempt = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx2,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            Some(&resume_key),
        )
        .await
        .expect("the resumed download must succeed");

        let final_bytes = tokio::fs::read(&second_attempt.path).await.unwrap();
        assert_eq!(
            final_bytes, full_body,
            "the resumed download's final bytes must exactly match a single uninterrupted download"
        );
        assert_eq!(
            second_attempt.bytes_downloaded,
            full_body.len() as u64,
            "bytes_downloaded must reflect the TOTAL file size (existing + newly fetched), not \
             just what this second call itself fetched over the wire"
        );
        assert!(
            partial_len_after_cancel > 0 && partial_len_after_cancel < full_body.len() as u64,
            "the cancelled first attempt should have written a genuine partial prefix (neither \
             zero bytes nor the whole file) for this test to actually be exercising a resume — \
             got {partial_len_after_cancel} of {} bytes",
            full_body.len()
        );

        // Sidecar/`.part` must both be cleaned up once the download is genuinely complete — no
        // permanent litter left behind in `staging_dir` after a successful resume.
        assert!(!tokio::fs::try_exists(&resume_paths.part_path)
            .await
            .unwrap());
        assert!(!tokio::fs::try_exists(&resume_paths.meta_path)
            .await
            .unwrap());

        tokio::fs::remove_file(&second_attempt.path).await.ok();
        tokio::fs::remove_dir_all(&staging_dir).await.ok();
        server.abort();
    }

    /// A server that doesn't understand `Range`/`If-Range` at all (per RFC 9110 §13.1.5, ignoring
    /// them and sending a normal `200` with the full body is the spec-correct behavior for such a
    /// server) must not corrupt anything — `download_one` must detect the `200` (not `206`),
    /// discard whatever stale partial bytes it had, and treat the full body as a clean fresh
    /// download rather than appending it onto old bytes and producing a corrupt double-length file.
    #[tokio::test]
    async fn a_server_that_ignores_range_falls_back_to_a_clean_full_redownload() {
        let full_body = b"the entire file, sent fresh every single time".to_vec();
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(move || {
                let full_body = full_body.clone();
                // Always `200` with the full body — deliberately ignores `Range` entirely,
                // simulating a real server with no Range support.
                async move { full_body }
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let manager = DownloadManager::new();
        let staging_dir =
            std::env::temp_dir().join(format!("lrr-resume-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&staging_dir).await.unwrap();
        let resume_key = format!("test-{}", uuid::Uuid::new_v4());
        let resume_paths = ResumePaths::new(&staging_dir, &resume_key);

        // Simulates a partial file left over from some earlier attempt (against this same
        // non-Range-supporting server, or possibly a different server entirely — from this
        // function's point of view it doesn't matter which) that was never actually validated as
        // resumable — garbage bytes, deliberately not a real prefix of `full_body`, so the test
        // can tell whether they got (wrongly) appended onto or (correctly) discarded.
        tokio::fs::write(&resume_paths.part_path, b"stale garbage bytes")
            .await
            .unwrap();
        let stale_meta = ResumeMetadata {
            url: format!("{base_url}/archive.zip"),
            etag: Some("\"stale-etag\"".to_string()),
            last_modified: None,
            filename: "archive.zip".to_string(),
        };
        tokio::fs::write(
            &resume_paths.meta_path,
            serde_json::to_vec(&stale_meta).unwrap(),
        )
        .await
        .unwrap();

        let req = DownloadRequest {
            url: format!("{base_url}/archive.zip"),
            method: None,
            headers: vec![],
            filename_hint: Some("archive.zip".to_string()),
        };
        let (progress_tx, _progress_rx) = tokio::sync::mpsc::unbounded_channel();
        let result = download_one(
            &manager,
            &[],
            &req,
            &NoopRateResolver,
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
            &|| {},
            Some(&resume_key),
        )
        .await
        .expect("must succeed by falling back to a fresh download, not error out");

        let final_bytes = tokio::fs::read(&result.path).await.unwrap();
        assert_eq!(
            final_bytes, b"the entire file, sent fresh every single time",
            "must be exactly the fresh full body, with no stale garbage prepended/appended"
        );

        tokio::fs::remove_file(&result.path).await.ok();
        tokio::fs::remove_dir_all(&staging_dir).await.ok();
        server.abort();
    }
}
