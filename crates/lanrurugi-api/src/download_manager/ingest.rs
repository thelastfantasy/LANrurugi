//! Wires a completed [`super::stream::download_one`] result into the exact same
//! stage → `ingest_file` → rename-into-`archive_dir` → `LRR_FILEMAP`-fixup sequence
//! `upload.rs::upload_archive` already performs for a manually uploaded file (spec FR-004: no
//! half-cataloged archive is left behind on failure, since `ingest_file` only ever runs after the
//! full byte transfer already succeeded).

use std::path::{Path, PathBuf};

use lanrurugi_core::ids::ArchiveId;
use lanrurugi_scanner::pipeline::{
    ingest_file_with_policy, DuplicatePolicy, DuplicateReason, IngestOptions, IngestOutcome,
};
use lanrurugi_storage::download_queue::PendingFilenameConflict;
use thiserror::Error;
use tokio::io::AsyncReadExt;

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
    /// A `Filename` collision (see [`DuplicateReason::Filename`]) that was staged to `temp_dir`
    /// instead of being immediately rejected — deliberately not folded into `Duplicate`, since a
    /// caller needs to persist the extra staging info (`temp_path`/`crc32`) onto the queue item,
    /// not just render a generic error. See `PendingFilenameConflict`'s own docs for why this
    /// exists at all (unlike a `ContentHash` collision, this one has a real, safe resolution the
    /// user can pick).
    #[error("filename collides with an existing archive, staged for user resolution: {0:?}")]
    PendingRename(PendingFilenameConflict),
    /// A `Filename` collision whose source archive's pages were already extracted and patched into
    /// the colliding target — no conflict menu to offer; the staged file can be discarded.
    #[error("source archive already patched into {existing_id}")]
    AlreadyPatched {
        existing_id: String,
        filename: String,
    },
}

/// Converts to the structured, translatable `QueueError` the frontend actually renders — logs
/// the original `Display` text via `tracing::warn!` here (the one place it's available before
/// being discarded) rather than serializing it anywhere user-facing. `Duplicate`/`PendingRename`
/// need no log: their fields are already fully structured and user-actionable (unlike the other
/// two, genuine internal faults).
impl From<&IngestDownloadError> for lanrurugi_core::queue_error::QueueError {
    fn from(e: &IngestDownloadError) -> Self {
        use lanrurugi_core::queue_error::QueueError;
        match e {
            IngestDownloadError::Pipeline(err) => {
                tracing::warn!(error = %err, "cataloging failed");
                QueueError::Internal
            }
            IngestDownloadError::Io(err) => {
                tracing::warn!(error = %err, "failed to move downloaded file into place");
                QueueError::WriteFailed
            }
            IngestDownloadError::Duplicate {
                existing_id,
                reason,
            } => QueueError::DuplicateArchive {
                existing_id: existing_id.clone(),
                reason: (*reason).into(),
            },
            IngestDownloadError::PendingRename(conflict) => QueueError::DuplicateFilename {
                existing_id: conflict.existing_id.clone(),
                filename: conflict.original_filename.clone(),
            },
            IngestDownloadError::AlreadyPatched {
                existing_id,
                filename,
            } => QueueError::AlreadyPatched {
                existing_id: existing_id.clone(),
                filename: filename.clone(),
            },
        }
    }
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

/// `overwrite: false` preserves this function's original behavior for a `ContentHash` collision
/// (always rejected, `IngestDownloadError::Duplicate` — see `pipeline.rs`'s own docs on why that
/// case never respects `DuplicatePolicy::Overwrite` at all) — but a `Filename` collision (content
/// is genuinely new, only the resolved filename collides) is now staged to `temp_dir` instead of
/// being silently discarded, returning `IngestDownloadError::PendingRename` so the caller can
/// persist it onto the queue item for the user to resolve later via `.../overwrite`/`.../rename`.
/// `overwrite: true` (the download-queue's opt-in overwrite checkbox) instead passes
/// `DuplicatePolicy::Overwrite`, which still only actually applies to a `Filename` collision (a
/// real collision by that filename deletes the old archive first rather than being staged/
/// rejected) — a `ContentHash` collision is rejected either way.
///
/// `queue_item_id`, when given, is the download-queue item this download is running for — used
/// only to persist a `PendingFilenameConflict` directly onto that item at staging time (the
/// caller's own generic `QueueError`-recording error handling still runs afterward and doesn't
/// need to know about this extra field).
pub async fn ingest_downloaded_file(
    state: &AppState,
    downloaded: &DownloadedFile,
    overwrite: bool,
    source_url: Option<&str>,
    queue_item_id: Option<&str>,
) -> Result<IngestedDownload, IngestDownloadError> {
    let duplicate_policy = if overwrite {
        DuplicatePolicy::Overwrite
    } else {
        DuplicatePolicy::Reject
    };
    match catalogue_staged_file(
        state,
        &downloaded.path,
        &downloaded.filename,
        source_url,
        duplicate_policy,
    )
    .await
    {
        Err(IngestDownloadError::Duplicate {
            existing_id,
            reason: DuplicateReason::Filename,
        }) if !overwrite => {
            // Before staging a full rename conflict, check whether this exact source was already
            // patched into the colliding target in a prior session — if so, the work is already
            // done, and we can discard the staged file immediately rather than re-offering the same
            // compare-and-resolve menu.
            if let Some(already) =
                check_already_patched(state, &downloaded.path, &downloaded.filename, &existing_id)
                    .await
            {
                let _ = tokio::fs::remove_file(&downloaded.path).await;
                return Err(already);
            }
            stage_pending_rename(
                state,
                &downloaded.path,
                &downloaded.filename,
                existing_id,
                queue_item_id,
            )
            .await
        }
        other => other,
    }
}

/// Runs `ingest_file_with_policy` against `staging_path` with the given `intended_filename`/
/// `duplicate_policy`, then (on a successful `Catalogued`/`Rekeyed`/`Unchanged`) finishes
/// cataloguing exactly like this module's own original single-path logic always did — move into
/// `archive_dir`, stamp the `source:` tag, fix up `LRR_FILEMAP`, run auto-plugins. Shared between
/// the original download-ingest path and both filename-conflict resolution paths
/// (`resolve_overwrite`/`resolve_rename` in `download_queue.rs`), which only differ in what
/// `intended_filename`/`duplicate_policy` they pass in.
async fn catalogue_staged_file(
    state: &AppState,
    staging_path: &Path,
    filename: &str,
    source_url: Option<&str>,
    duplicate_policy: DuplicatePolicy,
) -> Result<IngestedDownload, IngestDownloadError> {
    // Serializes this whole function's filename-collision-check-through-catalog-write sequence
    // against any other concurrent download racing to catalogue the *same* destination filename
    // (e.g. two different batch-started downloads that happen to resolve to an identical name) —
    // without this, both could pass `find_by_filename`'s check before either finished writing,
    // silently clobbering one archive's on-disk bytes with the other's while leaving two dangling
    // catalog records. Held for this entire function's body (guard drops at every return point,
    // including the early returns below), not just around `ingest_file_with_policy`'s own check.
    let _filename_lock = state.lock_filename(filename).await;

    // `title_filename` is always `filename` (the real, resolved name — from `Content-Disposition`,
    // a plugin-supplied hint, or the URL path — never the disposable staging name `staging_path`
    // sits under), independent of whether `duplicate_policy` also wants it used for the
    // filename-collision check. Without this, a `Reject`-policy download (the common case) had no
    // real name to fall back on at all and got titled after its staging path's own UUID — see
    // `catalogue_new_archive`'s own docs for the full bug this fixes.
    let options = IngestOptions {
        duplicate_policy,
        intended_filename: Some(filename),
        title_filename: Some(filename),
    };
    let outcome = ingest_file_with_policy(
        &state.repos.archives,
        &state.redis.config,
        &state.redis.search,
        &state.library.thumb_dir,
        staging_path,
        options,
    )
    .await?;

    let (archive_id, is_new) = match outcome {
        IngestOutcome::Unchanged { id } => {
            let _ = tokio::fs::remove_file(staging_path).await;
            return Ok(IngestedDownload {
                archive_id: id.into_string(),
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
                existing_id: existing_id.into_string(),
                reason,
            });
        }
    };

    let dest = unique_dest_path(&state.library.archive_dir, filename).await;

    // Same rename-with-copy-fallback portability handling as `upload.rs::upload_archive` (a
    // cross-filesystem `EXDEV` on `rename` is the most common real cause, e.g. `temp_dir`/
    // `archive_dir` mounted as separate volumes).
    if tokio::fs::rename(staging_path, &dest).await.is_err() {
        tokio::fs::copy(staging_path, &dest)
            .await
            .map_err(IngestDownloadError::Io)?;
        let _ = tokio::fs::remove_file(staging_path).await;
    }

    if let Ok(Some(mut archive)) = state.repos.archives.get(&archive_id).await {
        archive.file = dest.to_string_lossy().to_string();
        // `None` for a local upload (its callers pass the queue item's own filename-as-`url`
        // through nothing — see `upload.rs`/`download_queue.rs::resolve_conflict`) — there's no
        // real external source for a file the user picked off their own disk, so stamping one
        // would be fabricated data, not metadata. Legacy's own `Utils::Minion::download_url` task
        // (verified against source) computes `$og_url = trim_url($url)` from the URL the *user
        // originally gave* (the gallery page, never whatever internal download link a plugin
        // transformed it into) and passes `"source:$og_url"` into `handle_incoming_file` *before*
        // cataloguing — this is host-side download-task code, not something a metadata plugin adds
        // itself, and has no equivalent at all for an upload. Appended here (not a blind overwrite)
        // since a plugin's own `execDownload` could in principle have already supplied tags via
        // some other path; deduped the same way `set_tags(..., append=1)` does.
        if let Some(source_url) = source_url {
            let source_tag = format!("source:{}", crate::plugins::trim_url(source_url));
            if !archive.tags.split(',').any(|t| t.trim() == source_tag) {
                archive.tags = if archive.tags.is_empty() {
                    source_tag
                } else {
                    format!("{},{source_tag}", archive.tags)
                };
            }
        }
        let _ = state.repos.archives.save(&archive).await;
    }
    if let Ok(mut conn) = state.redis.config.get().await {
        use deadpool_redis::redis::AsyncCommands;
        use lanrurugi_storage::keys::FILEMAP_KEY;
        let staging_str = staging_path.to_string_lossy().to_string();
        let dest_str = dest.to_string_lossy().to_string();
        let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &staging_str).await;
        let _: Result<(), _> = conn.hset(FILEMAP_KEY, &dest_str, archive_id.as_str()).await;
    }

    // Legacy's real "自动运行"/`exec_enabled_plugins_on_file` mechanism (`Model::Upload.pm`) runs
    // on every newly added archive, downloads included — this code path is only ever reached for
    // a genuinely new `Catalogued`/`Rekeyed` outcome (the `Unchanged` case returns early above,
    // before ever reaching here), so no extra `is_new` guard is needed. Runs *after* the source
    // tag above is saved, so a plugin like `mems.ts`/`ehentai.ts` that falls back to parsing
    // `existing_tags` for a `source:` tag actually finds the one just added.
    crate::plugins::run_enabled_metadata_plugins_on_archive(state, archive_id.as_str()).await;

    Ok(IngestedDownload {
        archive_id: archive_id.into_string(),
        is_new,
    })
}

/// Moves `staging_path` to `temp_dir/temp_{crc32}_{filename}` (never deleting the already-
/// downloaded bytes just because the destination filename collided) and, when `queue_item_id` is
/// given, persists a [`PendingFilenameConflict`] directly onto that queue item so the frontend can
/// offer the user a real resolve action instead of just a generic error. Returns
/// [`IngestDownloadError::PendingRename`] either way (the caller's own generic `QueueError`-
/// recording error handling — see `plugins.rs::start_download`'s `Err` branch — runs afterward
/// regardless of whether the direct persist here succeeded, matching every other best-effort
/// side-channel write in this module).
async fn stage_pending_rename(
    state: &AppState,
    staging_path: &Path,
    filename: &str,
    existing_id: String,
    queue_item_id: Option<&str>,
) -> Result<IngestedDownload, IngestDownloadError> {
    let crc32 = hex_crc32_of_file(staging_path)
        .await
        .map_err(IngestDownloadError::Io)?;
    let temp_name = format!("temp_{crc32}_{filename}");
    let temp_path = state.library.temp_dir.join(&temp_name);

    if tokio::fs::rename(staging_path, &temp_path).await.is_err() {
        tokio::fs::copy(staging_path, &temp_path)
            .await
            .map_err(IngestDownloadError::Io)?;
        let _ = tokio::fs::remove_file(staging_path).await;
    }

    let conflict = PendingFilenameConflict {
        temp_path: temp_path.to_string_lossy().to_string(),
        original_filename: filename.to_string(),
        existing_id,
        crc32,
    };

    if let Some(item_id) = queue_item_id {
        if let Ok(Some(mut item)) = state.download_queue.get(item_id).await {
            item.pending_filename_conflict = Some(conflict.clone());
            if let Err(e) = state.download_queue.update(&item).await {
                tracing::warn!(%item_id, error = %e, "failed to persist pending filename conflict");
            }
        }
    }

    Err(IngestDownloadError::PendingRename(conflict))
}

/// Checks whether `staging_path`'s content was already patched into the archive identified by
/// `existing_id` — reads the colliding archive's `.patch.zip` (if one exists) and compares its
/// `source_crc32` against the staged file's own CRC32. Returns `Some(AlreadyPatched)` on a match
/// (the caller should discard the staged file and record the info), or `None` if no patch exists
/// / the source doesn't match / the archive can't be looked up (all degrade to the normal
/// filename-conflict flow — the patch check is a pure optimization, never a correctness gate).
async fn check_already_patched(
    state: &AppState,
    staging_path: &std::path::Path,
    filename: &str,
    existing_id: &str,
) -> Option<IngestDownloadError> {
    let existing_archive = state
        .repos
        .archives
        .get(&ArchiveId(existing_id.to_string()))
        .await
        .ok()??;
    let archive_path = std::path::PathBuf::from(&existing_archive.file);
    let patch_path = lanrurugi_scanner::patch::patch_path_for(&archive_path);
    if !patch_path.exists() {
        return None;
    }
    let staged_crc32 = hex_crc32_of_file(staging_path).await.ok()?;
    let metadata = lanrurugi_scanner::patch::load(&patch_path, &archive_path).ok()?;
    if metadata.source_crc32.as_deref() == Some(&staged_crc32) {
        return Some(IngestDownloadError::AlreadyPatched {
            existing_id: existing_id.to_string(),
            filename: filename.to_string(),
        });
    }
    None
}

/// How long an unresolved `PendingFilenameConflict`'s staged bytes are kept around before the
/// periodic sweep (`sweep_stale_pending_renames`, spawned from `main.rs`) reclaims them.
pub const PENDING_RENAME_MAX_AGE: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);

/// Deletes any `temp_dir/temp_*` file (this module's own `stage_pending_rename` naming
/// convention) whose last-modified time is older than [`PENDING_RENAME_MAX_AGE`], and downgrades
/// the `pending_filename_conflict` of any queue item that referenced it to a plain
/// `QueueError::DuplicateFilenameCleaned` — so a stale, abandoned filename conflict doesn't keep
/// offering "overwrite"/"rename and catalog" for bytes that no longer exist (neither action is
/// possible anymore), and doesn't accumulate disk usage forever if the user never comes back to
/// it. Called on a periodic timer (see `main.rs`), not in response to any single request.
pub async fn sweep_stale_pending_renames(state: &AppState) {
    sweep_stale_pending_renames_with_max_age(state, PENDING_RENAME_MAX_AGE).await
}

/// [`sweep_stale_pending_renames`] with an injectable `max_age` — split out purely so a test can
/// pass a millisecond-scale age instead of the real 24-hour constant, without needing to actually
/// wait 24 hours or fake a file's mtime.
async fn sweep_stale_pending_renames_with_max_age(state: &AppState, max_age: std::time::Duration) {
    let stale_paths = find_stale_temp_files(&state.library.temp_dir, max_age).await;
    if stale_paths.is_empty() {
        return;
    }

    // Downgrade any queue item pointing at one of these paths *before* deleting the files
    // themselves, so a resolve request racing this sweep can never observe "conflict cleared but
    // temp file still exists" — only "both still there" or "both gone".
    if let Ok(items) = state.download_queue.list_all().await {
        for mut item in items {
            let Some(conflict) = item.pending_filename_conflict.clone() else {
                continue;
            };
            let conflict_path = Path::new(&conflict.temp_path);
            if stale_paths.iter().any(|p| p == conflict_path) {
                tracing::info!(
                    item_id = %item.id,
                    temp_path = %conflict.temp_path,
                    "clearing stale unresolved filename conflict"
                );
                item.pending_filename_conflict = None;
                item.state = lanrurugi_storage::download_queue::DownloadQueueState::Error;
                item.error = Some(
                    lanrurugi_core::queue_error::QueueError::DuplicateFilenameCleaned {
                        existing_id: conflict.existing_id,
                        filename: conflict.original_filename,
                    },
                );
                let _ = state.download_queue.update(&item).await;
            }
        }
    }

    for path in stale_paths {
        match tokio::fs::remove_file(&path).await {
            Ok(()) => tracing::info!(?path, "removed stale pending-rename temp file"),
            Err(e) => {
                tracing::warn!(?path, error = %e, "failed to remove stale pending-rename temp file")
            }
        }
    }
}

/// Lists every `temp_*`-named entry directly inside `dir` whose last-modified time is at least
/// `max_age` old — the pure file-scanning half of [`sweep_stale_pending_renames`], split out so
/// it's testable against a real scratch directory without needing a full `AppState`/Redis.
/// Non-`temp_*` entries (anything else that happens to live in the same `temp_dir`, e.g. a
/// download currently mid-transfer) are always ignored, regardless of age.
async fn find_stale_temp_files(dir: &Path, max_age: std::time::Duration) -> Vec<PathBuf> {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read temp_dir for stale pending-rename sweep");
            return Vec::new();
        }
    };

    let mut stale_paths = Vec::new();
    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "error while scanning temp_dir for stale pending renames");
                break;
            }
        };
        let is_own_temp_file = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with("temp_"));
        if !is_own_temp_file {
            continue;
        }
        let is_stale = match entry.metadata().await.and_then(|m| m.modified()) {
            Ok(modified) => modified.elapsed().is_ok_and(|age| age >= max_age),
            Err(_) => false,
        };
        if is_stale {
            stale_paths.push(entry.path());
        }
    }
    stale_paths
}

/// Streams `path` in fixed-size chunks (never loading the whole file into memory — a real archive
/// can be hundreds of MB) through `crc32fast::Hasher`, returning its lowercase hex CRC32.
async fn hex_crc32_of_file(path: &Path) -> std::io::Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:08x}", hasher.finalize()))
}

/// Appends a numeric suffix (`name (1).ext`, `name (2).ext`, ...) if `archive_dir/filename`
/// already exists — a real download's suggested filename (from `Content-Disposition`/
/// `filename_hint`/URL path) has no uniqueness guarantee the way a fresh UUID-derived staging
/// name does, unlike `upload.rs::upload_archive`'s destination (which trusts the client-supplied
/// name as-is and lets a same-name second upload overwrite, since that's an explicit user action
/// naming their own file — a downloaded file's name is comparatively more likely to collide
/// incidentally, e.g. two different artworks both named `archive.zip` by their respective source
/// sites). Purely a disk-basename check (any file occupying that exact path, cataloged or not) —
/// entirely independent of, and unaffected by, the `find_by_filename` *archive-record* collision
/// check `catalogue_staged_file` already resolved before ever reaching this point.
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

/// Resolves a `PendingFilenameConflict` by deleting the colliding archive and cataloguing the
/// staged file under its originally-intended filename — `download_queue.rs::overwrite_queue_item`'s
/// own implementation, kept here alongside `catalogue_staged_file` since it's the only other
/// caller of that helper.
pub async fn resolve_overwrite(
    state: &AppState,
    conflict: &PendingFilenameConflict,
    source_url: Option<&str>,
) -> Result<IngestedDownload, IngestDownloadError> {
    catalogue_staged_file(
        state,
        Path::new(&conflict.temp_path),
        &conflict.original_filename,
        source_url,
        DuplicatePolicy::Overwrite,
    )
    .await
}

/// Resolves a `PendingFilenameConflict` by cataloguing the staged file under a new, user-supplied
/// filename instead — the existing archive is left untouched. If `new_filename` *also* collides
/// (rare — the user picked a name that happens to already be taken by some other archive), this
/// returns another `IngestDownloadError::PendingRename` for the same staged content under the new
/// name, so the user can simply try again with yet another name rather than losing the staged
/// bytes.
pub async fn resolve_rename(
    state: &AppState,
    conflict: &PendingFilenameConflict,
    new_filename: &str,
    source_url: Option<&str>,
    queue_item_id: Option<&str>,
) -> Result<IngestedDownload, IngestDownloadError> {
    match catalogue_staged_file(
        state,
        Path::new(&conflict.temp_path),
        new_filename,
        source_url,
        DuplicatePolicy::Reject,
    )
    .await
    {
        Err(IngestDownloadError::Duplicate {
            existing_id,
            reason: DuplicateReason::Filename,
        }) => {
            stage_pending_rename(
                state,
                Path::new(&conflict.temp_path),
                new_filename,
                existing_id,
                queue_item_id,
            )
            .await
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, uniquely-named path under the shared OS temp dir — same convention
    /// `download_manager::stream`'s own tests already use (this crate has no `tempfile` dev-
    /// dependency), just with a per-test-unique filename so concurrent test runs don't collide.
    fn scratch_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lrr-ingest-test-{}-{name}", uuid::Uuid::new_v4()))
    }

    #[tokio::test]
    async fn hex_crc32_of_file_matches_a_known_value() {
        let path = scratch_path("sample.txt");
        // A well-known CRC32 test vector: CRC32("123456789") = 0xCBF43926.
        tokio::fs::write(&path, b"123456789").await.unwrap();
        assert_eq!(hex_crc32_of_file(&path).await.unwrap(), "cbf43926");
        tokio::fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn hex_crc32_of_file_handles_content_larger_than_one_read_buffer() {
        let path = scratch_path("large.bin");
        // Larger than the function's own 64KB read buffer, to exercise the multi-chunk loop.
        let content = vec![0x42u8; 200 * 1024];
        tokio::fs::write(&path, &content).await.unwrap();
        let expected = format!("{:08x}", crc32fast::hash(&content));
        assert_eq!(hex_crc32_of_file(&path).await.unwrap(), expected);
        tokio::fs::remove_file(&path).await.ok();
    }

    #[tokio::test]
    async fn hex_crc32_of_file_is_deterministic_and_content_sensitive() {
        let a = scratch_path("a.bin");
        let b = scratch_path("b.bin");
        tokio::fs::write(&a, b"hello world").await.unwrap();
        tokio::fs::write(&b, b"hello worlD").await.unwrap();
        let crc_a = hex_crc32_of_file(&a).await.unwrap();
        let crc_b = hex_crc32_of_file(&b).await.unwrap();
        assert_ne!(crc_a, crc_b);
        assert_eq!(crc_a, hex_crc32_of_file(&a).await.unwrap());
        tokio::fs::remove_file(&a).await.ok();
        tokio::fs::remove_file(&b).await.ok();
    }

    /// A fresh, uniquely-named scratch *directory* under the shared OS temp dir — isolates each
    /// `find_stale_temp_files` test's own entries from whatever else (including other tests'
    /// `temp_*`-prefixed scratch files) might transiently exist in the shared OS temp dir itself.
    async fn scratch_dir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lrr-ingest-sweep-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&dir).await.unwrap();
        dir
    }

    #[tokio::test]
    async fn finds_a_temp_prefixed_file_older_than_max_age() {
        let dir = scratch_dir().await;
        let path = dir.join("temp_deadbeef_archive.zip");
        tokio::fs::write(&path, b"x").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let stale = find_stale_temp_files(&dir, std::time::Duration::from_millis(10)).await;
        assert_eq!(stale, vec![path]);

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn ignores_a_temp_prefixed_file_younger_than_max_age() {
        let dir = scratch_dir().await;
        let path = dir.join("temp_deadbeef_archive.zip");
        tokio::fs::write(&path, b"x").await.unwrap();

        let stale = find_stale_temp_files(&dir, std::time::Duration::from_secs(3600)).await;
        assert!(stale.is_empty());

        tokio::fs::remove_dir_all(&dir).await.ok();
    }

    #[tokio::test]
    async fn ignores_old_files_that_do_not_have_the_temp_prefix() {
        let dir = scratch_dir().await;
        // A download's own in-flight staging file (`download-<uuid>.zip`, per `stream.rs`'s own
        // naming) sitting in the very same `temp_dir` — must never be swept just for being old;
        // only this module's own `temp_*`-prefixed pending-rename files are this sweep's concern.
        let path = dir.join("download-not-a-pending-rename.zip");
        tokio::fs::write(&path, b"x").await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;

        let stale = find_stale_temp_files(&dir, std::time::Duration::from_millis(10)).await;
        assert!(stale.is_empty());

        tokio::fs::remove_dir_all(&dir).await.ok();
    }
}
