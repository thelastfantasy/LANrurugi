//! `POST /archives/upload` and `/tempfolder` — shapes verified against
//! `~/LANraragi/tools/openapi.yaml`. Reuses `download_manager::ingest::ingest_downloaded_file`
//! (originally written for the download pipeline, per that module's own docs) rather than a
//! second, parallel stage→catalog→rename-into-`archive_dir`→`LRR_FILEMAP`-fixup implementation —
//! this is also what gives a local upload the exact same persisted download-queue state machine a
//! download gets: it survives a page refresh, offers the same overwrite/rename UI for a filename
//! collision instead of a flat 409 rejection, and shows a Remove button through the same
//! `DELETE /download_queue/{id}` endpoint.

use std::path::Path;

use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::delete;
use axum::Router;
use lanrurugi_storage::download_queue::{DownloadQueueState, NewQueueItem, QueueItemOrigin};
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::common::error;
use crate::download_manager::ingest::ingest_downloaded_file;
use crate::download_manager::stream::DownloadedFile;
use crate::plugins::update_queue_item_state;
use crate::AppState;

/// Axum's `Multipart` extractor enforces `DefaultBodyLimit`'s built-in 2 MB default when no
/// layer overrides it — real manga archives routinely exceed that by 100x, so uploads failed
/// with "Error parsing `multipart/form-data` request" (`multer`'s size-exceeded error) for any
/// file over 2 MB. Scoped to this route only, not applied globally, so every other endpoint
/// keeps the small default.
const MAX_UPLOAD_BYTES: usize = 2 * 1024 * 1024 * 1024;

/// The fixed `plugin_namespace` every local-upload queue item is stored under — never a real
/// installed plugin's namespace (validated nowhere against `PluginPool`, since nothing in this
/// path ever calls a plugin to move bytes the way a download does), just this item type's own
/// grouping key on the Upload page.
const LOCAL_UPLOAD_NAMESPACE: &str = "local_upload";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/archives/upload",
            axum::routing::put(upload_archive).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/tempfolder", delete(clean_tempfolder))
}

/// Filenames are taken from the multipart field only for their extension/base name — never used
/// as a path, so a client can't traverse outside `archive_dir` via `../../etc/passwd`-style names.
fn sanitize_filename(name: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("upload")
        .to_string()
}

async fn upload_archive(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut file_bytes: Option<bytes::Bytes> = None;
    let mut file_name: Option<String> = None;
    let mut checksum: Option<String> = None;
    let mut category: Option<String> = None;
    let mut title: Option<String> = None;
    let mut summary: Option<String> = None;
    let mut tags: Option<String> = None;

    loop {
        let field = match multipart.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return error(StatusCode::BAD_REQUEST, "upload", e.to_string()),
        };
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                file_name = field.file_name().map(sanitize_filename);
                match field.bytes().await {
                    Ok(b) => file_bytes = Some(b),
                    Err(e) => return error(StatusCode::BAD_REQUEST, "upload", e.to_string()),
                }
            }
            "file_checksum" => checksum = field.text().await.ok(),
            "catid" => category = field.text().await.ok().filter(|s| !s.is_empty()),
            "title" => title = field.text().await.ok(),
            "summary" => summary = field.text().await.ok(),
            "tags" => tags = field.text().await.ok(),
            _ => {}
        }
    }

    let Some(bytes) = file_bytes else {
        return error(StatusCode::BAD_REQUEST, "upload", "No file provided.");
    };
    let file_name = file_name.unwrap_or_else(|| "upload.zip".to_string());

    if let Some(expected) = checksum.as_deref().filter(|s| !s.is_empty()) {
        // Full-file SHA-1 (archives run up to `MAX_UPLOAD_BYTES` = 2 GB) — off the async reactor
        // per constitution Principle III; a synchronous inline hash here would stall whichever
        // Tokio worker is running this handler for the hash's entire duration. `bytes::Bytes`'s
        // own `.clone()` is a cheap Arc refcount bump, not a buffer copy.
        let hash_bytes = bytes.clone();
        let actual = match lanrurugi_core::concurrency::run_blocking(move || {
            let mut hasher = Sha1::new();
            hasher.update(&hash_bytes);
            hex_encode(&hasher.finalize())
        })
        .await
        {
            Ok(actual) => actual,
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string()),
        };
        if !actual.eq_ignore_ascii_case(expected) {
            return (
                StatusCode::EXPECTATION_FAILED,
                axum::Json(json!({
                    "operation": "upload",
                    "error": "Checksum mismatch.",
                    "success": 0,
                })),
            )
                .into_response();
        }
    }

    // Persisted *before* the byte transfer even starts (there is none left to do — the whole
    // multipart body is already fully in `bytes` by this point — but staged, hashed, and
    // catalogued asynchronously all the same, below) so a queue item exists to record whatever
    // outcome follows. `Queued` initially, same as a fresh download — flipped to `Done`/`Error`
    // a few lines down once the real outcome is known, never left sitting in `Queued` the way a
    // download does while waiting on a user's own Start click (a local upload has nothing left
    // to start; the multipart PUT that got this far already committed to running it now).
    let queue_item = match state
        .download_queue
        .add(NewQueueItem {
            origin: QueueItemOrigin::LocalUpload,
            url: file_name.clone(),
            plugin_namespace: LOCAL_UPLOAD_NAMESPACE.to_string(),
            file_size: Some(bytes.len() as u64),
            category: category.clone(),
            auto_fetch_metadata: false,
            overwrite_on_duplicate: false,
            state: DownloadQueueState::Queued,
        })
        .await
    {
        Ok(item) => item,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string()),
    };

    // Written to `temp_dir` (never watched — `crate::watcher::watch` only recurses
    // `archive_dir`) and ingested *there* first, mirroring legacy's own `handle_incoming_file`
    // (`~/LANraragi/lib/LANraragi/Model/Upload.pm`): legacy computes the ID and registers the
    // archive in Redis *before* moving the file into the watched content folder, specifically
    // "so Shinobu doesn't do it" (its own comment). Doing it the other way around — write
    // straight into `archive_dir`, ingest afterward — raced against this project's own
    // notify-based watcher picking up the same just-written file and cataloguing it first via
    // its own `ingest_file` call: the upload handler's *own* explicit call then saw the ID as
    // already tracked and returned a spurious 409 "already exists" for what was genuinely a
    // first-time upload. Reproduced directly against a real running backend during
    // 003-ui-test-automation's implementation (confirmed by disabling the watcher via
    // `--no-watch`, which made the race disappear) before this fix.
    // Carries the uploaded file's own extension onto the staging path — mirrors the identical fix
    // in `download_manager::stream::download_one` (see that call site's own doc comment for the
    // full story). `lanrurugi_scanner::pipeline::catalogue_new_archive` computes `pagecount` via
    // `archive_format::list_pages` against *this* staging path, a pure extension-based format
    // check; a bare `upload-<uuid>` with no extension always fails it, silently and permanently
    // stuck at `pagecount: 0` regardless of how the final, renamed destination file looks.
    let staging_path = match Path::new(&file_name).extension().and_then(|e| e.to_str()) {
        Some(ext) => state
            .library
            .temp_dir
            .join(format!("upload-{}.{ext}", uuid::Uuid::new_v4().simple())),
        None => state
            .library
            .temp_dir
            .join(format!("upload-{}", uuid::Uuid::new_v4().simple())),
    };
    if let Err(e) = tokio::fs::write(&staging_path, &bytes).await {
        update_queue_item_state(
            &state.download_queue,
            &queue_item.id,
            DownloadQueueState::Error,
            None,
            Some(lanrurugi_core::queue_error::QueueError::WriteFailed),
            None,
        )
        .await;
        return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
    }

    let downloaded = DownloadedFile {
        path: staging_path,
        filename: file_name.clone(),
        bytes_downloaded: bytes.len() as u64,
    };

    // Same overwrite/reject semantics as a download's own `overwrite_on_duplicate` checkbox
    // (always `false` here — see `NewQueueItem` above; a same-name re-upload is offered the
    // same overwrite/rename choice a download would get instead of an unconditional overwrite),
    // and the exact same `ContentHash`-vs-`Filename` collision handling: a byte-identical
    // duplicate is unconditionally rejected, while a filename-only collision is staged to
    // `temp_dir` and offered to the user via `.../overwrite`/`.../rename` — no longer a flat 409
    // with the staged bytes simply deleted, unlike this handler's previous behavior.
    // `None` — a local upload has no real external source URL to stamp a `source:` tag with (see
    // `ingest_downloaded_file`'s own docs); `file_name` here is just the uploaded file's name, not
    // a URL, and was previously being written into `source:` verbatim as if it were one.
    match ingest_downloaded_file(&state, &downloaded, false, None, Some(&queue_item.id)).await {
        Ok(ingested) => {
            if let Some(catid) = &category {
                let _ =
                    crate::categories::add_archive_to_category(&state, catid, &ingested.archive_id)
                        .await;
            }

            if title.is_some() || summary.is_some() || tags.is_some() {
                if let Ok(Some(mut archive)) = state
                    .repos
                    .archives
                    .get(&lanrurugi_core::ids::ArchiveId(ingested.archive_id.clone()))
                    .await
                {
                    if let Some(t) = title {
                        archive.title = t;
                    }
                    if let Some(s) = summary {
                        archive.summary = s;
                    }
                    if let Some(t) = tags {
                        archive.tags = t;
                    }
                    let _ = state.repos.archives.save(&archive).await;
                }
            }

            // Legacy's own real order (`Model::Upload.pm`): user-supplied title/tags land first,
            // *then* every enabled metadata plugin runs and appends on top
            // (`set_tags(..., append=1)`) — matched here by calling this after, not before, the
            // user-supplied-fields block above.
            crate::plugins::run_enabled_metadata_plugins_on_archive(&state, &ingested.archive_id)
                .await;

            update_queue_item_state(
                &state.download_queue,
                &queue_item.id,
                DownloadQueueState::Done,
                None,
                None,
                Some(vec![ingested.archive_id.clone()]),
            )
            .await;

            axum::Json(json!({
                "operation": "upload",
                "success": 1,
                "id": ingested.archive_id,
            }))
            .into_response()
        }
        Err(e) => {
            // `PendingRename` is not a genuine failure the way the other variants are — the
            // bytes are safe, staged in `temp_dir`, and `stage_pending_rename` (inside
            // `ingest_downloaded_file`) already persisted the `PendingFilenameConflict` itself
            // directly onto the queue item; this only needs to additionally record the matching
            // `state`/`error`, mirroring `plugins.rs::start_download`'s own error-recording
            // convention for the identical situation on the download side. The HTTP response
            // below is still a plain error status either way — the frontend never reads this
            // response body for its own error UI (this handler doesn't return an id/`Location`
            // for the queue item), it always resolves via the queue item's next
            // `GET /download_queue` poll.
            let queue_error = lanrurugi_core::queue_error::QueueError::from(&e);
            update_queue_item_state(
                &state.download_queue,
                &queue_item.id,
                DownloadQueueState::Error,
                None,
                Some(queue_error.clone()),
                None,
            )
            .await;

            use lanrurugi_core::queue_error::QueueError;
            let (status, message, existing_id) = match &queue_error {
                QueueError::DuplicateArchive { existing_id, .. } => (
                    StatusCode::CONFLICT,
                    "This file already exists in the Library.".to_string(),
                    Some(existing_id.clone()),
                ),
                QueueError::DuplicateFilename { existing_id, .. } => (
                    StatusCode::CONFLICT,
                    "A file with this name already exists in the Library.".to_string(),
                    Some(existing_id.clone()),
                ),
                other => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("{other:?}"),
                    None,
                ),
            };
            (
                status,
                axum::Json(json!({
                    "operation": "upload",
                    "error": message,
                    "success": 0,
                    "id": existing_id,
                })),
            )
                .into_response()
        }
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

async fn clean_tempfolder(State(state): State<AppState>) -> Response {
    let mut newsize: u64 = 0;
    let mut error_msg: Option<String> = None;

    let mut entries = match tokio::fs::read_dir(&state.library.temp_dir).await {
        Ok(e) => e,
        Err(e) => {
            return axum::Json(json!({
                "operation": "cleantemp",
                "success": 1,
                "error": e.to_string(),
                "newsize": 0,
            }))
            .into_response()
        }
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let result = if path.is_dir() {
            tokio::fs::remove_dir_all(&path).await
        } else {
            tokio::fs::remove_file(&path).await
        };
        if let Err(e) = result {
            error_msg = Some(e.to_string());
        }
    }

    if let Ok(mut remaining) = tokio::fs::read_dir(&state.library.temp_dir).await {
        while let Ok(Some(entry)) = remaining.next_entry().await {
            if let Ok(meta) = entry.metadata().await {
                newsize += meta.len();
            }
        }
    }

    axum::Json(json!({
        "operation": "cleantemp",
        "success": 1,
        "error": error_msg,
        "newsize": newsize,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The archive filename itself (as opposed to filenames *inside* the archive, covered in
    // `lanrurugi-scanner::archive_format`'s tests) goes through `std::path::Path`/`OsString`
    // throughout this codebase, which is lossless for any filename the OS accepts — no
    // ASCII-only assumption anywhere. These tests just confirm `sanitize_filename` (the one place
    // that touches the raw multipart field name) doesn't mangle spaces, Japanese punctuation, or
    // CJK characters while still stripping directory-traversal components.
    #[test]
    fn sanitize_filename_preserves_spaces() {
        assert_eq!(
            sanitize_filename("my manga volume 1.zip"),
            "my manga volume 1.zip"
        );
    }

    #[test]
    fn sanitize_filename_preserves_japanese_punctuation_and_cjk() {
        assert_eq!(
            sanitize_filename("【サークル】第一巻「初版」.zip"),
            "【サークル】第一巻「初版」.zip"
        );
    }

    #[test]
    fn sanitize_filename_preserves_simplified_and_traditional_chinese() {
        assert_eq!(
            sanitize_filename("测试文件（第一卷）.zip"),
            "测试文件（第一卷）.zip"
        );
        assert_eq!(
            sanitize_filename("測試檔案（第一卷）.zip"),
            "測試檔案（第一卷）.zip"
        );
    }

    #[test]
    fn sanitize_filename_preserves_korean() {
        assert_eq!(
            sanitize_filename("테스트 파일 1권.zip"),
            "테스트 파일 1권.zip"
        );
    }

    #[test]
    fn sanitize_filename_strips_directory_traversal_but_keeps_special_chars() {
        assert_eq!(
            sanitize_filename("../../etc/passwd/日本語 テスト.zip"),
            "日本語 テスト.zip"
        );
        assert_eq!(sanitize_filename("../../../etc/passwd"), "passwd");
    }

    #[test]
    fn sanitize_filename_falls_back_when_empty_or_only_traversal() {
        assert_eq!(sanitize_filename(""), "upload");
        assert_eq!(sanitize_filename("../.."), "upload");
    }
}
