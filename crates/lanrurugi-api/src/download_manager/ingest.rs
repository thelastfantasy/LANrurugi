//! Wires a completed [`super::stream::download_one`] result into the exact same
//! stage → `ingest_file` → rename-into-`archive_dir` → `LRR_FILEMAP`-fixup sequence
//! `upload.rs::upload_archive` already performs for a manually uploaded file (spec FR-004: no
//! half-cataloged archive is left behind on failure, since `ingest_file` only ever runs after the
//! full byte transfer already succeeded).

use std::path::{Path, PathBuf};

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_scanner::pipeline::{
    ingest_file_with_policy, DuplicatePolicy, DuplicateReason, IngestOptions, IngestOutcome,
};
use lanrurugi_storage::activity::Outcome;
use lanrurugi_storage::download_queue::PendingFilenameConflict;
use lanrurugi_storage::keys::CONFIG_KEY;
use serde_json::{json, Value};
use thiserror::Error;

use crate::auth_context::AuthContext;
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

/// Catalogues a download that a pre-020 build parked waiting for a filename decision: the
/// destination is derived from the bytes now (`<bucket>/<id>.<ext>`), so the question the user was
/// asked has no wrong answer and the staged file is simply ingested under its own id.
///
/// Returns the archive id the bytes ended up under. The error type is `QueueError` because the only
/// caller is the startup repair pass, which stores it on the queue row as-is.
pub async fn adopt_pending_conflict(
    state: &AppState,
    conflict: &PendingFilenameConflict,
) -> Result<String, lanrurugi_core::queue_error::QueueError> {
    let staging = Path::new(&conflict.temp_path);
    match catalogue_staged_file(
        state,
        staging,
        &conflict.original_filename,
        None,
        DuplicatePolicy::Reject,
    )
    .await
    {
        Ok(done) => Ok(done.archive_id),
        // The same bytes are already in the library: this staged copy is a redundant second copy
        // with nothing pointing at it, so it is dropped rather than left in `temp_dir` forever.
        Err(IngestDownloadError::Duplicate { existing_id, .. }) => {
            if let Err(e) = tokio::fs::remove_file(staging).await {
                tracing::warn!(
                    path = %staging.display(),
                    error = %e,
                    "could not drop a duplicate staged download"
                );
            }
            Ok(existing_id)
        }
        Err(e) => Err(lanrurugi_core::queue_error::QueueError::from(&e)),
    }
}

/// Wraps one `ingest_downloaded_file` result with the unified `archive.ingest` activity record —
/// shared by the download pipeline (`plugins::run_managed_downloads`) and the web UI's user upload
/// (`upload.rs::upload_archive`), both of which funnel through `ingest_downloaded_file` but carry
/// different actors/context. `source` is `"download"` or `"upload"`; `extra` carries that source's
/// own fields (plugin/url/queue item, or the upload's category/origin). `PendingRename` is
/// deliberately not recorded as a failure — the bytes are safely staged awaiting the user's own
/// overwrite/rename decision, the same "not a genuine failure" distinction `upload.rs` already
/// draws for its own `archive.upload` record.
pub(crate) async fn record_ingest_result(
    state: &AppState,
    auth: Option<&AuthContext>,
    source: &str,
    filename: &str,
    extra: Value,
    result: Result<IngestedDownload, IngestDownloadError>,
) -> Result<IngestedDownload, IngestDownloadError> {
    let mut base = match extra {
        Value::Object(map) => map,
        other => {
            let mut map = serde_json::Map::new();
            map.insert("details".to_string(), other);
            map
        }
    };
    base.insert("filename".to_string(), json!(filename));
    match &result {
        Ok(ingested) => {
            base.insert("archive_id".to_string(), json!(ingested.archive_id));
            base.insert("is_new".to_string(), json!(ingested.is_new));
            crate::activity::record_archive_ingest(
                state,
                crate::activity::IngestActor::Manual(auth),
                source,
                filename,
                Outcome::Success,
                Some(&ingested.archive_id),
                Value::Object(base),
            )
            .await;
        }
        Err(IngestDownloadError::PendingRename(_)) => {}
        Err(e) => {
            crate::activity::record_archive_ingest(
                state,
                crate::activity::IngestActor::Manual(auth),
                source,
                filename,
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                Value::Object(base),
            )
            .await;
        }
    }
    result
}

/// `overwrite: false` preserves this function's original behavior for a `ContentHash` collision
/// (always rejected, `IngestDownloadError::Duplicate` — see `pipeline.rs`'s own docs on why that
/// case never respects `DuplicatePolicy::Overwrite` at all). Downloaded archives are named by
/// their content (`<library>/<bucket>/{id}.{ext}`), so a filename collision cannot occur at all
/// any more; `overwrite: true` therefore only controls whether an already-catalogued archive with
/// the same content is replaced (`DuplicatePolicy::Overwrite`) or reported
/// (`DuplicatePolicy::Reject`).
///
/// `_queue_item_id` is kept for the call sites that still carry the queue item id; ingestion no
/// longer needs it now that nothing stages a filename conflict.
pub async fn ingest_downloaded_file(
    state: &AppState,
    downloaded: &DownloadedFile,
    overwrite: bool,
    source_url: Option<&str>,
    _queue_item_id: Option<&str>,
) -> Result<IngestedDownload, IngestDownloadError> {
    let duplicate_policy = if overwrite {
        DuplicatePolicy::Overwrite
    } else {
        DuplicatePolicy::Reject
    };
    catalogue_staged_file(
        state,
        &downloaded.path,
        &downloaded.filename,
        source_url,
        duplicate_policy,
    )
    .await
}

/// Marks `archive_id` unpublished, so `lanrurugi_search` skips it until
/// [`clear_pending_visibility`] runs. Best-effort: if Redis is unreachable the archive is simply
/// visible a little early with partial metadata, which is the old behaviour — not worth failing an
/// otherwise successful ingest over.
async fn mark_pending_visibility(state: &AppState, archive_id: &str) {
    use deadpool_redis::redis::AsyncCommands;
    let Ok(mut conn) = state.redis.config.get().await else {
        return;
    };
    let key = lanrurugi_storage::keys::pending_visibility_key(archive_id);
    let _: Result<(), _> = conn
        .set_ex(
            &key,
            "1",
            lanrurugi_storage::keys::PENDING_VISIBILITY_TTL_SECS,
        )
        .await;
}

async fn clear_pending_visibility(state: &AppState, archive_id: &str) {
    use deadpool_redis::redis::AsyncCommands;
    let Ok(mut conn) = state.redis.config.get().await else {
        return;
    };
    let key = lanrurugi_storage::keys::pending_visibility_key(archive_id);
    let _: Result<(), _> = conn.del(&key).await;
}

/// Runs `ingest_file_with_policy` against `staging_path` with the given `intended_filename`/
/// `duplicate_policy`, then (on a successful `Catalogued`/`Rekeyed`/`Unchanged`) finishes
/// cataloguing exactly like this module's own original single-path logic always did — move into
/// `archive_dir`, stamp the `source:` tag, fix up `LRR_FILEMAP`, run auto-plugins. Shared between
/// the original download-ingest path and both filename-conflict resolution paths
/// (`resolve_overwrite`/`resolve_rename` in `download_queue.rs`), which only differ in what
/// `intended_filename`/`duplicate_policy` they pass in.
pub(crate) async fn catalogue_staged_file(
    state: &AppState,
    staging_path: &Path,
    filename: &str,
    source_url: Option<&str>,
    duplicate_policy: DuplicatePolicy,
) -> Result<IngestedDownload, IngestDownloadError> {
    // The destination is derived from the bytes, not from the name: the id is the content hash and
    // the file is `<bucket>/<id>.<ext>`, so two downloads can never want the same path for
    // different bytes (020). That removes the filename lock, the collision policy and the
    //
    // The bytes still reach the library *before* the record exists (a record is visible to
    // `/api/search` the moment it is stored), and the cross-filesystem move is the longest part of
    // ingesting a large download. The copy goes to a dot-prefixed name inside `archive_dir` first
    // so a concurrent scan cannot catalogue a half-written file, then is renamed into its bucket.
    // The id is computed from that same copy, before the rename.
    let extension = filename_extension(filename);
    let incoming = state
        .library
        .archive_dir
        .join(format!(".ingest-{}", uuid::Uuid::new_v4().simple()));
    crate::download_manager::rename_or_copy(staging_path, &incoming)
        .await
        .map_err(IngestDownloadError::Io)?;
    let id = match lanrurugi_storage::id::size_aware_id(&incoming) {
        Ok(id) => ArchiveId(id),
        Err(e) => {
            let _ = tokio::fs::remove_file(&incoming).await;
            return Err(IngestDownloadError::Io(std::io::Error::other(e)));
        }
    };
    let dest = lanrurugi_storage::paths::hash_layout_path(
        &state.library.archive_dir,
        id.as_str(),
        &extension,
    );

    // These exact bytes are already catalogued and on disk: nothing to move, nothing to update.
    if let Ok(Some(existing)) = state.repos.archives.get(&id).await {
        if Path::new(&existing.file).is_file() {
            let _ = tokio::fs::remove_file(&incoming).await;
            return Ok(IngestedDownload {
                archive_id: id.into_string(),
                is_new: false,
            });
        }
        // A record without bytes (an interrupted overwrite, or a restored dump): adopt the copy
        // just made instead of leaving a record pointing at nothing.
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(IngestDownloadError::Io)?;
        }
        tokio::fs::rename(&incoming, &dest)
            .await
            .map_err(IngestDownloadError::Io)?;
        let mut repaired = existing;
        repaired.file = dest.to_string_lossy().to_string();
        state
            .repos
            .archives
            .save(&repaired)
            .await
            .map_err(|e| IngestDownloadError::Io(std::io::Error::other(e)))?;
        return Ok(IngestedDownload {
            archive_id: id.into_string(),
            is_new: false,
        });
    }

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(IngestDownloadError::Io)?;
    }
    if let Err(e) = tokio::fs::rename(&incoming, &dest).await {
        let _ = tokio::fs::remove_file(&incoming).await;
        return Err(IngestDownloadError::Io(e));
    }

    // `intended_filename: None` — the on-disk name is the id now, so the legacy "reject a second
    // archive with the same filename" rule has nothing to compare. Titles still come from the real
    // name via `title_filename`.
    let options = IngestOptions {
        duplicate_policy,
        intended_filename: None,
        title_filename: Some(filename),
        defer_file_path: false,
    };
    let outcome = match ingest_file_with_policy(
        &state.repos.archives,
        &state.redis.config,
        &state.redis.search,
        &state.equivalence,
        &state.library.thumb_dir,
        &dest,
        options,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(e.into());
        }
    };

    let (archive_id, is_new) = match outcome {
        IngestOutcome::Unchanged { id } => {
            // The file at `dest` *is* this archive's bytes now (it was renamed into its canonical
            // path above) — removing it here would delete the only copy.
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
            let _ = tokio::fs::remove_file(&dest).await;
            return Err(IngestDownloadError::Duplicate {
                existing_id: existing_id.into_string(),
                reason,
            });
        }
    };

    // The record exists and its bytes are complete, but its tags are not: the auto-metadata run
    // below is what fills them in. Hide it from search until then, so a download never shows up in
    // the library as an untagged, unrelated-looking card (the previous order made that window as
    // long as the whole cross-filesystem copy).
    mark_pending_visibility(state, archive_id.as_str()).await;

    // The record's `file` is already correct (catalogued from its final path), so what remains is
    // the `source:` tag. Written into the same save, retried — a Redis blip here would otherwise
    // leave a live record with no source, which dedup and the startup zombie-repair sweep both
    // look at. If every retry fails, that sweep still converges it later.
    const RETRIES: u32 = 3;
    const BACKOFF_MS: [u64; RETRIES as usize] = [200, 500, 1000];
    for (i, &delay) in BACKOFF_MS.iter().enumerate() {
        match state.repos.archives.get(&archive_id).await {
            Ok(Some(mut archive)) => {
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
                match state.repos.archives.save(&archive).await {
                    Ok(()) => break,
                    Err(e) if i + 1 < RETRIES as usize => {
                        tracing::warn!(%archive_id, attempt = i + 1, delay_ms = delay, error = %e, "failed to fix up archive file path, retrying");
                        tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    }
                    Err(e) => {
                        tracing::error!(%archive_id, error = %e, "failed to fix up archive file path after {RETRIES} attempts — startup zombie sweep will repair");
                    }
                }
            }
            Ok(None) => {
                tracing::error!(%archive_id, "archive record vanished during file-path fixup");
                break;
            }
            Err(e) if i + 1 < RETRIES as usize => {
                tracing::warn!(%archive_id, attempt = i + 1, delay_ms = delay, error = %e, "failed to load archive for file-path fixup, retrying");
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            Err(e) => {
                tracing::error!(%archive_id, error = %e, "failed to load archive for file-path fixup after {RETRIES} attempts — startup zombie sweep will repair");
            }
        }
    }
    // No `LRR_FILEMAP` fixup is needed any more: the pipeline mapped the final path directly,
    // because the bytes were already there when the record was catalogued.

    // Legacy's real "自动运行"/`exec_enabled_plugins_on_file` mechanism (`Model::Upload.pm`) runs
    // on every newly added archive, downloads included — this code path is only ever reached for
    // a genuinely new `Catalogued`/`Rekeyed` outcome (the `Unchanged` case returns early above,
    // before ever reaching here), so no extra `is_new` guard is needed. Runs *after* the source
    // tag above is saved, so a plugin like `mems.ts`/`ehentai.ts` that falls back to parsing
    // `existing_tags` for a `source:` tag actually finds the one just added.
    crate::plugins::run_enabled_metadata_plugins_on_archive(state, archive_id.as_str()).await;

    // Published: the archive now carries whatever the plugins found (a failure inside the run is
    // already non-fatal and logged there), so make it visible in the library.
    clear_pending_visibility(state, archive_id.as_str()).await;

    Ok(IngestedDownload {
        archive_id: archive_id.into_string(),
        is_new,
    })
}

/// Default `tempmaxsize` (MB) — matches `NUMBER_FIELDS`' default in `settings.rs`, used when the
/// field is missing from Redis entirely.
pub(crate) const DEFAULT_TEMP_MAX_SIZE_MB: i64 = 500;

/// Enforces the `tempmaxsize` setting (MB, Redis `LRR_CONFIG` hash field) against the unbounded,
/// self-regenerating content under `temp_dir` (a cache miss just regenerates it; nothing is lost):
/// `resize_page/`, the reader's WebP/JXL resize cache (`archives.rs::resize_cache_path`), and the
/// rendered e-book pages under `book/` (`lanrurugi_book`'s page cache; its small `info.json` files
/// are left alone). Deliberately does not touch anything else in `temp_dir`:
/// Anything else in `temp_dir` (a download's in-flight staging file, for instance) is not a
/// cache and is left alone. Called on a periodic timer (see `main.rs`), not in response to any
/// request.
pub async fn sweep_resize_cache_size(state: &AppState) {
    let max_bytes = match state.redis.config.get().await {
        Ok(mut conn) => {
            let fields: std::collections::HashMap<String, String> =
                conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
            fields
                .get("tempmaxsize")
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(DEFAULT_TEMP_MAX_SIZE_MB)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to read tempmaxsize from redis, using default");
            DEFAULT_TEMP_MAX_SIZE_MB
        }
    }
    .max(0) as u64
        * 1024
        * 1024;

    let mut entries = Vec::new();
    // Merged patch copies and bao outboards are regenerable, so they join the reclaimable set;
    // `<temp>/share/import` is deliberately absent — those are resumable partial transfers.
    for dir in ["resize_page", "book", "share/merged", "share/outboards"] {
        if let Some(found) = list_resize_cache_entries(&state.library.temp_dir.join(dir)).await {
            entries.extend(found);
        }
    }

    let total_bytes: u64 = entries.iter().map(|e| e.size).sum();
    if total_bytes <= max_bytes {
        return;
    }

    // Oldest first, so the least-recently-generated pages (least likely to still be in a reader's
    // prefetch window) are evicted before anything newer.
    entries.sort_by_key(|e| e.modified);

    let mut freed = 0u64;
    let mut remaining = total_bytes;
    for entry in entries {
        if remaining <= max_bytes {
            break;
        }
        // The `.dims` sidecar (see `resize_cache_path`'s doc comment) has no independent size
        // tracked above; delete it alongside its image so a future cache hit doesn't try to read
        // stale dimensions for a file that no longer exists.
        let extension = entry
            .path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let sidecar = entry.path.with_extension(format!("{extension}.dims"));
        let _ = tokio::fs::remove_file(&sidecar).await;
        match tokio::fs::remove_file(&entry.path).await {
            Ok(()) => {
                freed += entry.size;
                remaining = remaining.saturating_sub(entry.size);
            }
            Err(e) => {
                tracing::warn!(path = ?entry.path, error = %e, "failed to remove resize cache entry")
            }
        }
    }
    tracing::info!(
        freed_bytes = freed,
        max_bytes,
        "trimmed resize cache to stay under tempmaxsize"
    );
}

struct ResizeCacheEntry {
    path: PathBuf,
    size: u64,
    modified: std::time::SystemTime,
}

/// Lists every `.webp` / `.jxl` file one directory level below `cache_dir` (`resize_page/<id>/…`,
/// `book/<fingerprint>/…`) with its size and mtime. Returns `None` (rather than an empty vec) when the directory doesn't exist yet — a
/// fresh install with no resize activity yet, not a cache to trim.
async fn list_resize_cache_entries(cache_dir: &Path) -> Option<Vec<ResizeCacheEntry>> {
    let mut per_archive_dirs = match tokio::fs::read_dir(cache_dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!(error = %e, "failed to read resize cache dir");
            return None;
        }
    };

    let mut out = Vec::new();
    loop {
        let archive_dir = match per_archive_dirs.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!(error = %e, "error scanning resize cache dir");
                break;
            }
        };
        if !archive_dir.file_type().await.is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let mut files = match tokio::fs::read_dir(archive_dir.path()).await {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        loop {
            let file = match files.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(_) => break,
            };
            let is_resize_cache = file
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".webp") || name.ends_with(".jxl"));
            if !is_resize_cache {
                continue;
            }
            let Ok(metadata) = file.metadata().await else {
                continue;
            };
            let modified = metadata
                .modified()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            out.push(ResizeCacheEntry {
                path: file.path(),
                size: metadata.len(),
                modified,
            });
        }
    }
    Some(out)
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
    _queue_item_id: Option<&str>,
) -> Result<IngestedDownload, IngestDownloadError> {
    catalogue_staged_file(
        state,
        Path::new(&conflict.temp_path),
        new_filename,
        source_url,
        DuplicatePolicy::Reject,
    )
    .await
}

/// The extension to keep on the hash-layout file, lowercased. The name carries no meaning any
/// more, but the extension still selects a container format when the archive is opened.
fn filename_extension(filename: &str) -> String {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// A real (if minimal) zip: the scanner dispatches the format by extension, but the page count
    /// heuristic still opens it, so a truncated header would fail ingestion for the wrong reason.
    fn staged_zip(dir: &std::path::Path, nonce: &str) -> std::path::PathBuf {
        let path = dir.join("staging-upload.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        writer
            .start_file("0001.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"\xff\xd8\xff\xd9").unwrap();
        // Both tests share a Redis; identical bytes would collide on the content hash.
        writer
            .start_file("note.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(nonce.as_bytes()).unwrap();
        writer.finish().unwrap();
        path
    }

    async fn state_with_temp_dirs(root: &std::path::Path) -> Option<AppState> {
        let mut state = crate::plugins::tests::test_state().await?;
        state.library.archive_dir = root.join("content");
        state.library.temp_dir = root.join("temp");
        std::fs::create_dir_all(&state.library.archive_dir).unwrap();
        std::fs::create_dir_all(&state.library.temp_dir).unwrap();
        Some(state)
    }

    /// 020 FR-001/FR-004: the bytes land under their content id, the reader-visible title comes from
    /// the name the download carried, and the original filename survives in the sidecar.
    #[tokio::test]
    async fn a_download_lands_on_its_content_id_with_its_title_and_a_sidecar() {
        let root = tempfile::tempdir().unwrap();
        let Some(state) = state_with_temp_dirs(root.path()).await else {
            return;
        };
        let staging = staged_zip(root.path(), "first");

        let ingested = catalogue_staged_file(
            &state,
            &staging,
            "Some Title [DL版].zip",
            Some("https://example.org/file"),
            DuplicatePolicy::Reject,
        )
        .await
        .expect("the download is catalogued");

        assert!(ingested.is_new);
        assert!(!staging.exists(), "the staging file moved into the library");

        let archive = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId::from(
                ingested.archive_id.clone(),
            ))
            .await
            .unwrap()
            .expect("a record was written");
        let expected = lanrurugi_storage::paths::hash_layout_path(
            &state.library.archive_dir,
            &ingested.archive_id,
            "zip",
        );
        assert_eq!(std::path::Path::new(&archive.file), expected);
        assert!(expected.exists());
        assert_eq!(archive.name, "Some Title [DL版]");
        assert_eq!(archive.title, "Some Title");

        let sidecar =
            lanrurugi_storage::sidecar::read(&state.library.archive_dir, &ingested.archive_id)
                .expect("the sidecar travels with the archive");
        assert_eq!(sidecar.original_filename, "Some Title [DL版].zip");
        assert_eq!(sidecar.title, "Some Title");
    }

    /// 020 FR-001: a name no filesystem accepts is no longer a reason to fail an ingest — the
    /// library path is derived from the bytes, and only the record keeps the long title.
    #[tokio::test]
    async fn a_name_longer_than_any_filesystem_allows_still_ingests() {
        let root = tempfile::tempdir().unwrap();
        let Some(state) = state_with_temp_dirs(root.path()).await else {
            return;
        };
        let staging = staged_zip(root.path(), "second");
        let long = format!("{}.zip", "長い名前 ".repeat(40));

        let ingested =
            catalogue_staged_file(&state, &staging, &long, None, DuplicatePolicy::Reject)
                .await
                .expect("the title length is not the filename length");

        let archive = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId::from(
                ingested.archive_id.clone(),
            ))
            .await
            .unwrap()
            .expect("a record was written");
        assert!(archive.name.len() > 255);
        assert_eq!(archive.file.rsplit('/').next().unwrap().len(), 44);
    }

    /// 020 adoption pass: a queue row parked by a pre-020 build (waiting for a filename decision)
    /// is catalogued by its bytes on the next start. The same bytes arriving twice must not create a
    /// second archive, and the second staged copy is discarded rather than left in the temp dir.
    #[tokio::test]
    async fn a_legacy_pending_conflict_is_adopted_by_content_once() {
        let root = tempfile::tempdir().unwrap();
        let Some(state) = state_with_temp_dirs(root.path()).await else {
            return;
        };
        let conflict =
            |path: &std::path::Path| lanrurugi_storage::download_queue::PendingFilenameConflict {
                temp_path: path.to_string_lossy().into_owned(),
                original_filename: "Legacy Title [DL版].zip".to_string(),
                existing_id: String::new(),
                crc32: "deadbeef".to_string(),
                staged_at: 0,
            };

        let first = staged_zip(root.path(), "legacy");
        let id = adopt_pending_conflict(&state, &conflict(&first))
            .await
            .expect("catalogued");
        assert!(!first.exists());
        let archive = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId::from(id.clone()))
            .await
            .unwrap()
            .expect("a record was written");
        assert_eq!(archive.name, "Legacy Title [DL版]");
        assert_eq!(archive.title, "Legacy Title");

        // The same bytes staged again resolve to the archive that already holds them.
        // A byte-identical copy of what the library holds: a second `staged_zip` call would embed a
        // different zip mtime and so hash differently, which is not the case under test.
        let staged_dir = root.path().join("again");
        std::fs::create_dir_all(&staged_dir).unwrap();
        let second = staged_dir.join("staging-upload.zip");
        std::fs::copy(&archive.file, &second).unwrap();
        let same = adopt_pending_conflict(&state, &conflict(&second))
            .await
            .expect("already catalogued");
        assert_eq!(same, id);
        assert!(!second.exists(), "the duplicate staged copy is discarded");
    }

    /// 020 FR-003: a name that is nothing but release markers must not end up with an empty title —
    /// the filename is the fallback, brackets and all, because an empty title is unsearchable.
    #[tokio::test]
    async fn a_name_made_only_of_release_markers_keeps_a_non_empty_title() {
        let root = tempfile::tempdir().unwrap();
        let Some(state) = state_with_temp_dirs(root.path()).await else {
            return;
        };
        let staging = staged_zip(root.path(), "markers-only");

        let ingested = catalogue_staged_file(
            &state,
            &staging,
            "[DL版] (中国翻訳).zip",
            None,
            DuplicatePolicy::Reject,
        )
        .await
        .expect("catalogued");
        let archive = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId::from(ingested.archive_id))
            .await
            .unwrap()
            .expect("a record was written");
        assert_eq!(archive.name, "[DL版] (中国翻訳)");
        assert_eq!(archive.title, archive.name, "an empty title is unusable");
    }
}
