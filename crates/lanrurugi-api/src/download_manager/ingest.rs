//! Wires a completed [`super::stream::download_one`] result into the exact same
//! stage → `ingest_file` → rename-into-`archive_dir` → `LRR_FILEMAP`-fixup sequence
//! `upload.rs::upload_archive` already performs for a manually uploaded file (spec FR-004: no
//! half-cataloged archive is left behind on failure, since `ingest_file` only ever runs after the
//! full byte transfer already succeeded).

use std::path::Path;

use lanrurugi_scanner::pipeline::{
    ingest_file_with_policy, DuplicatePolicy, DuplicateReason, IngestOutcome,
};
use thiserror::Error;

use crate::AppState;

use super::stream::DownloadedFile;

#[derive(Debug, Error)]
pub enum IngestDownloadError {
    #[error("cataloging failed: {0}")]
    Pipeline(#[from] lanrurugi_scanner::pipeline::PipelineError),
    #[error("failed to move downloaded file into place: {0}")]
    Io(#[source] std::io::Error),
    #[error("a colliding archive already exists ({reason:?}): {existing_id}")]
    Duplicate {
        existing_id: String,
        reason: DuplicateReason,
    },
}

/// Cataloges a single already-downloaded, already-staged file, moving it into
/// `state.library.archive_dir` and fixing up the `Archive` record's `file` field + `LRR_FILEMAP`
/// entry exactly as `upload.rs::upload_archive` does for its own staged uploads. Returns the
/// resulting archive ID.
///
/// An `Unchanged` outcome (the download's content byte-for-byte matches an archive already in the
/// library) removes the just-downloaded staging file rather than leaving a redundant duplicate on
/// disk — the caller doesn't need to distinguish this from a fresh catalog for FR-004 purposes
/// (either way, no *half*-cataloged file is left behind); it's surfaced in the returned bool.
pub struct IngestedDownload {
    pub archive_id: String,
    /// `true` if this was a brand-new or re-keyed archive; `false` if the content was already
    /// tracked under an existing ID (the staged file was removed, not moved into the library).
    pub is_new: bool,
}

/// `overwrite: false` preserves this function's original behavior exactly — no filename-
/// collision check at all (`intended_filename: None`), relying on [`unique_dest_path`]'s
/// existing silent-auto-rename for the destination basename (a real download's suggested
/// filename has no uniqueness guarantee, and different suggested filenames from a source site
/// coincidentally colliding is normal, not something that should hard-fail). `overwrite: true`
/// (the download-queue's opt-in overwrite checkbox) instead passes `downloaded.filename` as the
/// intended filename under [`DuplicatePolicy::Overwrite`] — a real collision (by content hash or
/// by that filename) deletes the old archive first rather than being auto-renamed around.
pub async fn ingest_downloaded_file(
    state: &AppState,
    downloaded: &DownloadedFile,
    overwrite: bool,
) -> Result<IngestedDownload, IngestDownloadError> {
    let (duplicate_policy, intended_filename) = if overwrite {
        (
            DuplicatePolicy::Overwrite,
            Some(downloaded.filename.as_str()),
        )
    } else {
        (DuplicatePolicy::Reject, None)
    };

    let outcome = ingest_file_with_policy(
        &state.repos.archives,
        &state.redis.config,
        &state.redis.search,
        &state.library.thumb_dir,
        &downloaded.path,
        duplicate_policy,
        intended_filename,
    )
    .await?;

    let (archive_id, is_new) = match outcome {
        IngestOutcome::Unchanged { id } => {
            let _ = tokio::fs::remove_file(&downloaded.path).await;
            return Ok(IngestedDownload {
                archive_id: id,
                is_new: false,
            });
        }
        IngestOutcome::Catalogued { id } => (id, true),
        IngestOutcome::Rekeyed { new_id, .. } => (new_id, true),
        IngestOutcome::Rejected {
            existing_id,
            reason,
        } => {
            return Err(IngestDownloadError::Duplicate {
                existing_id,
                reason,
            });
        }
    };

    let dest = unique_dest_path(&state.library.archive_dir, &downloaded.filename).await;

    // Same rename-with-copy-fallback portability handling as `upload.rs::upload_archive` (a
    // cross-filesystem `EXDEV` on `rename` is the most common real cause, e.g. `temp_dir`/
    // `archive_dir` mounted as separate volumes).
    if tokio::fs::rename(&downloaded.path, &dest).await.is_err() {
        tokio::fs::copy(&downloaded.path, &dest)
            .await
            .map_err(IngestDownloadError::Io)?;
        let _ = tokio::fs::remove_file(&downloaded.path).await;
    }

    if let Ok(Some(mut archive)) = state.repos.archives.get(&archive_id).await {
        archive.file = dest.to_string_lossy().to_string();
        let _ = state.repos.archives.save(&archive).await;
    }
    if let Ok(mut conn) = state.redis.config.get().await {
        use deadpool_redis::redis::AsyncCommands;
        use lanrurugi_storage::keys::FILEMAP_KEY;
        let staging_str = downloaded.path.to_string_lossy().to_string();
        let dest_str = dest.to_string_lossy().to_string();
        let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &staging_str).await;
        let _: Result<(), _> = conn.hset(FILEMAP_KEY, &dest_str, &archive_id).await;
    }

    Ok(IngestedDownload { archive_id, is_new })
}

/// Appends a numeric suffix (`name (1).ext`, `name (2).ext`, ...) if `archive_dir/filename`
/// already exists — a real download's suggested filename (from `Content-Disposition`/
/// `filename_hint`/URL path) has no uniqueness guarantee the way a fresh UUID-derived staging
/// name does, unlike `upload.rs::upload_archive`'s destination (which trusts the client-supplied
/// name as-is and lets a same-name second upload overwrite, since that's an explicit user action
/// naming their own file — a downloaded file's name is comparatively more likely to collide
/// incidentally, e.g. two different artworks both named `archive.zip` by their respective source
/// sites).
async fn unique_dest_path(archive_dir: &Path, filename: &str) -> std::path::PathBuf {
    let candidate = archive_dir.join(filename);
    if tokio::fs::metadata(&candidate).await.is_err() {
        return candidate;
    }
    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1..10_000 {
        let name = match ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = archive_dir.join(&name);
        if tokio::fs::metadata(&candidate).await.is_err() {
            return candidate;
        }
    }
    // Practically unreachable (10,000 same-stem collisions), but a deterministic fallback is
    // still preferable to an infinite loop.
    archive_dir.join(format!("{stem}-{}", uuid::Uuid::new_v4().simple()))
}
