//! `POST /archives/upload` and `/tempfolder` — shapes verified against
//! `~/LANraragi/tools/openapi.yaml`. Reuses `pipeline::ingest_file`'s size-aware cataloguing logic
//! (T041) rather than duplicating it, so uploaded and watched-in files go through the exact same
//! non-merging path.

use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::delete;
use axum::Router;
use lanrurugi_scanner::pipeline::{ingest_file, IngestOutcome};
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::common::error;
use crate::AppState;

/// Axum's `Multipart` extractor enforces `DefaultBodyLimit`'s built-in 2 MB default when no
/// layer overrides it — real manga archives routinely exceed that by 100x, so uploads failed
/// with "Error parsing `multipart/form-data` request" (`multer`'s size-exceeded error) for any
/// file over 2 MB. Scoped to this route only, not applied globally, so every other endpoint
/// keeps the small default.
const MAX_UPLOAD_BYTES: usize = 2 * 1024 * 1024 * 1024;

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

use std::path::Path;

async fn upload_archive(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut checksum: Option<String> = None;
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
                    Ok(b) => file_bytes = Some(b.to_vec()),
                    Err(e) => return error(StatusCode::BAD_REQUEST, "upload", e.to_string()),
                }
            }
            "file_checksum" => checksum = field.text().await.ok(),
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
        let mut hasher = Sha1::new();
        hasher.update(&bytes);
        let actual = hex_encode(&hasher.finalize());
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

    let dest = state.library.archive_dir.join(&file_name);
    if let Err(e) = tokio::fs::write(&dest, &bytes).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
    }

    let outcome = ingest_file(
        &state.repos.archives,
        &state.redis.config,
        &state.redis.search,
        &state.library.thumb_dir,
        &dest,
    )
    .await;

    let id = match outcome {
        Ok(IngestOutcome::Catalogued { id }) => id,
        Ok(IngestOutcome::Unchanged { id }) => {
            return (
                StatusCode::CONFLICT,
                axum::Json(json!({
                    "operation": "upload",
                    "error": "This file already exists in the Library.",
                    "success": 0,
                    "id": id,
                })),
            )
                .into_response();
        }
        Ok(IngestOutcome::Rekeyed { new_id, .. }) => new_id,
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "upload", e.to_string());
        }
    };

    if title.is_some() || summary.is_some() || tags.is_some() {
        if let Ok(Some(mut archive)) = state.repos.archives.get(&id).await {
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

    axum::Json(json!({ "operation": "upload", "success": 1, "id": id })).into_response()
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
