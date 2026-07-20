//! The real Rust-side streaming HTTP download (spec FR-001/FR-002/FR-005/FR-009 — see this
//! module's own docs for how progress/concurrency/rate-limit are all threaded through one call).

use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use reqwest::Method;
use tokio::io::AsyncWriteExt;

use super::domain_rules::{resolved_key, DomainRule};
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
    #[error("server responded with status {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("failed to write to {0}: {1}")]
    Io(PathBuf, #[source] std::io::Error),
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
) -> Result<DownloadedFile, DownloadError> {
    let parsed_url =
        url::Url::parse(&req.url).map_err(|e| DownloadError::InvalidUrl(req.url.clone(), e))?;
    let hostname = parsed_url.host_str().unwrap_or("").to_string();

    // FR-016 snapshot: acquired once, held for this whole function's lifetime (dropped when this
    // function returns/errors, at the very end of the transfer — see `_permit` below).
    let permit = manager.acquire(&hostname, rules).await;
    let rate_limit_key = resolved_key(rules, &hostname);

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
        return Err(DownloadError::HttpStatus(response.status()));
    }

    let total_bytes = response.content_length();
    let filename = resolve_filename(&response, req.filename_hint.as_deref(), &parsed_url);

    let staging_path = staging_dir.join(format!("download-{}", uuid::Uuid::new_v4().simple()));
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

    while let Some(chunk) = stream.next().await {
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
            .throttle(
                &rate_limit_key,
                permit.max_bytes_per_sec,
                chunk.len() as u64,
            )
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
fn content_disposition_filename(response: &reqwest::Response) -> Option<String> {
    let header = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .to_str()
        .ok()?;
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
        let result = download_one(&manager, &[], &req, progress_tx, &staging_dir)
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
        let err = download_one(&manager, &[], &req, progress_tx, &staging_dir)
            .await
            .expect_err("a 404 response must surface as an error, not a successful empty file");
        assert!(matches!(err, DownloadError::HttpStatus(status) if status == 404));

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
        let result = download_one(&manager, &[], &req, progress_tx, &staging_dir)
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
            let result = download_one(&manager, &[], &req, progress_tx, &staging_dir)
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

        let err = download_one(&manager, &[], &req, progress_tx, &staging_dir)
            .await
            .expect_err(
                "a truncated body must surface as a real error, not a short-but-clean file",
            );
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
