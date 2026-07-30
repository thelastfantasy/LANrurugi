//! The real Rust-side streaming HTTP download (spec FR-001/FR-002/FR-005/FR-009 — see this
//! module's own docs for how progress/concurrency/rate-limit are all threaded through one call).

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use reqwest::Method;
use tokio::io::AsyncWriteExt;

use super::domain_rules::DomainRule;
use super::DownloadManager;

/// One resource to fetch — mirrors `contracts/plugin-download-protocol.md`'s `downloads[]`
/// element (deserialized straight from a plugin's `execDownload` JSON result upstream).
#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub url: String,
    pub method: Option<String>,
    pub headers: Vec<(String, String)>,
    pub filename_hint: Option<String>,
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
/// The concurrency permit and resolved rate limit are both captured once, at the top of this
/// function, before the real request is even sent — this is the FR-016 snapshot-at-start-time
/// guarantee: whatever `rules` resolves to at this exact moment governs this download for its
/// entire duration, unaffected by any settings change that happens to land mid-transfer.
pub async fn download_one(
    manager: &DownloadManager,
    rules: &[DomainRule],
    req: &DownloadRequest,
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
    staging_dir: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<DownloadedFile, DownloadError> {
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let parsed_url =
        url::Url::parse(&req.url).map_err(|e| DownloadError::InvalidUrl(req.url.clone(), e))?;
    let hostname = parsed_url.host_str().unwrap_or("").to_string();

    // FR-016 snapshot: acquired once, held for this whole function's lifetime (dropped when this
    // function returns/errors, at the very end of the transfer — see `_permit` below).
    let permit = manager.acquire(&hostname, rules).await;
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
        progress_tx,
        staging_dir,
        cancel,
        &parsed_url,
        &permit,
        &rate_limit_key,
    )
    .await;
    manager.rate_limiters().release(&rate_limit_key).await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn download_one_inner(
    manager: &DownloadManager,
    req: &DownloadRequest,
    progress_tx: tokio::sync::mpsc::UnboundedSender<ProgressUpdate>,
    staging_dir: &Path,
    cancel: &tokio_util::sync::CancellationToken,
    parsed_url: &url::Url,
    permit: &super::DownloadPermit,
    rate_limit_key: &str,
) -> Result<DownloadedFile, DownloadError> {
    let method = match &req.method {
        None => Method::GET,
        Some(m) => {
            Method::from_bytes(m.as_bytes()).map_err(|_| DownloadError::InvalidMethod(m.clone()))?
        }
    };

    let client = reqwest::Client::new();
    let mut request = client.request(method, parsed_url.clone());
    for (name, value) in &req.headers {
        request = request.header(name, value);
    }

    let response = request.send().await.map_err(DownloadError::Request)?;
    if !response.status().is_success() {
        return Err(DownloadError::HttpStatus(
            req.url.clone(),
            response.status(),
        ));
    }

    let total_bytes = response.content_length();
    let filename = resolve_filename(&response, req.filename_hint.as_deref(), parsed_url);

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
    let staging_path = match Path::new(&filename).extension().and_then(|e| e.to_str()) {
        Some(ext) => staging_dir.join(format!("download-{}.{ext}", uuid::Uuid::new_v4().simple())),
        None => staging_dir.join(format!("download-{}", uuid::Uuid::new_v4().simple())),
    };
    let mut file = tokio::fs::File::create(&staging_path)
        .await
        .map_err(|e| DownloadError::Io(staging_path.clone(), e))?;

    let mut downloaded: u64 = 0;
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
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                drop(file);
                let _ = tokio::fs::remove_file(&staging_path).await;
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
                // `staging_path` — clean it up rather than leaking it, since this function is
                // returning `Err` and no caller ever reaches `ingest_file`/a rename for it (spec
                // FR-004's "no half-cataloged archive" extends to "no half-downloaded litter on
                // disk" too, even though only the cataloging half is spec-mandated).
                drop(file);
                let _ = tokio::fs::remove_file(&staging_path).await;
                return Err(DownloadError::Request(e));
            }
        };
        manager
            .rate_limiters()
            .throttle(rate_limit_key, permit.max_bytes_per_sec, chunk.len() as u64)
            .await;
        if let Err(e) = file.write_all(&chunk).await {
            drop(file);
            let _ = tokio::fs::remove_file(&staging_path).await;
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

    Ok(DownloadedFile {
        path: staging_path,
        filename,
        bytes_downloaded: downloaded,
    })
}

/// Determines the real filename for a downloaded resource, in priority order (contracts/
/// plugin-download-protocol.md's `filename_hint` field docs):
/// 1. The response's own `Content-Disposition` header (`filename=`/`filename*=`), when present
///    and parseable — matches legacy `Model::Upload.pm::download_url`'s own real-file-name
///    behavior (verified against that Perl source).
/// 2. The plugin-supplied `filename_hint`, when the header is absent or unparseable.
/// 3. A name derived from the URL's own path, as a last resort.
fn resolve_filename(
    response: &reqwest::Response,
    filename_hint: Option<&str>,
    url: &url::Url,
) -> String {
    if let Some(name) = content_disposition_filename(response) {
        return sanitize_filename(&name);
    }
    if let Some(hint) = filename_hint {
        return sanitize_filename(hint);
    }
    let from_path = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|s| !s.is_empty());
    sanitize_filename(from_path.unwrap_or("download"))
}

/// Parses a `Content-Disposition` header's `filename=`/`filename*=` parameter. Deliberately
/// simple (no full RFC 6266 `filename*=UTF-8''...` percent-decoding) since every real corpus
/// source (Chaika/EHentai-style direct-file-download endpoints) sends the plain `filename=` form —
/// good enough for this project's actual plugin corpus without pulling in a dedicated MIME/header
/// parsing crate for a rarely-exercised edge case.
///
/// Reads the header's **raw bytes** (`.as_bytes()`), not `HeaderValue::to_str()` — a real,
/// confirmed-live bug this fixes: `to_str()` only succeeds for visible-ASCII byte sequences and
/// returns `Err` (silently swallowed by the old code's `.ok()?`) for anything else, so a server
/// that puts a real UTF-8-encoded non-ASCII filename directly into a plain `filename="..."`
/// parameter — not RFC-compliant (that should be `filename*=UTF-8''...`, percent-encoded), but
/// confirmed against a real download source that sends raw UTF-8 bytes in a plain `filename=`
/// parameter — made this function give up entirely and fall through to the plugin's own
/// `filename_hint`/URL-derived fallback, discarding a perfectly real filename the server did
/// provide.
fn content_disposition_filename(response: &reqwest::Response) -> Option<String> {
    let bytes = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .as_bytes();
    // UTF-8 first (the real, confirmed-live case above) — Latin-1 as a last-resort fallback for a
    // server that genuinely sends single-byte-per-character bytes (Latin-1 maps every byte to a
    // Unicode code point of the identical value, so this conversion can never itself fail).
    let header = String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
    for part in header.split(';') {
        let part = part.trim();
        if let Some(name) = part.strip_prefix("filename=") {
            return Some(name.trim_matches('"').to_string());
        }
        if let Some(name) = part.strip_prefix("filename*=") {
            // `UTF-8''actual-name` — strip the charset/lang prefix, leave percent-encoding as-is
            // (good enough; a plugin-supplied `filename_hint` or the URL-derived fallback covers
            // the rare case this doesn't fully resolve to something sane).
            let name = name.rsplit("''").next().unwrap_or(name);
            return Some(name.trim_matches('"').to_string());
        }
    }
    None
}

/// Same reasoning as `upload.rs::sanitize_filename` — never used as a path, so a
/// server/plugin-supplied name can't traverse outside the staging/archive directory.
fn sanitize_filename(name: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_directory_traversal() {
        assert_eq!(
            sanitize_filename("../../etc/passwd/archive.zip"),
            "archive.zip"
        );
    }

    #[test]
    fn sanitize_filename_falls_back_when_empty() {
        assert_eq!(sanitize_filename(""), "download");
    }

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
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
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
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
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
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
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
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
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
        let err = download_one(&manager, &[], &req, progress_tx, &staging_dir, &cancel)
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
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
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
                progress_tx,
                &staging_dir,
                &tokio_util::sync::CancellationToken::new(),
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
            progress_tx,
            &staging_dir,
            &tokio_util::sync::CancellationToken::new(),
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
}
