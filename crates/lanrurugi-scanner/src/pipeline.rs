//! mpsc-based ingestion pipeline (T038): consumes file paths (from the watcher or a bulk scan),
//! waits for each to stabilize (FR-006), computes its size-aware ID, and either catalogues a new
//! `Archive` or reconciles a path whose ID changed — all within a per-file timeout (T037, a new
//! Phase 1 safety net beyond anything legacy has, per research.md §6) so one pathological file
//! can't stall the whole pipeline.
//!
//! **Verified against source** (`~/LANraragi/lib/Shinobu.pm::update_filemap_entry`): legacy
//! tracks `file path -> id` in a `LRR_FILEMAP` hash on the **config** logical DB, so a file whose
//! content changes gets its existing Archive record re-keyed (via `change_archive_id`) rather than
//! creating a duplicate. This module reuses that exact key/DB placement. The one behavioral
//! change from legacy (the actual FR-005 fix): if the newly-computed ID already belongs to a
//! *different* tracked path, legacy would silently overwrite that other archive's `file`/`name`
//! fields (the false-merge defect); this pipeline only does that when the IDs match (which, under
//! the size-aware algorithm, only happens for byte-identical content) — see `size_aware_id`'s
//! module docs for why that's now safe.

use std::path::{Path, PathBuf};
use std::time::Duration;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::entities::Archive;
use lanrurugi_core::filename_lock::FilenameLocks;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_storage::id::size_aware_id;
pub(crate) use lanrurugi_storage::keys::FILEMAP_KEY;
use lanrurugi_storage::repository::ArchiveRepository;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::archive_format;
use crate::watcher::wait_until_stable;

/// New in Phase 1 (research.md §6) — legacy has no equivalent per-file timeout.
const INGEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("file did not stabilize: {0}")]
    Stability(#[from] crate::watcher::StabilityError),
    #[error("archive-ID computation failed: {0}")]
    Id(#[from] lanrurugi_storage::id::IdError),
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("pool error: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("repository error: {0}")]
    Repository(#[from] lanrurugi_storage::repository::RepositoryError),
    #[error("timed out after {0:?} ingesting file")]
    Timeout(Duration),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    /// A brand-new archive was catalogued.
    Catalogued { id: ArchiveId },
    /// The path was already tracked under this same ID — no change (e.g. duplicate inotify
    /// events, or a byte-identical rewrite).
    Unchanged { id: ArchiveId },
    /// The path's content changed; its existing Archive record was re-keyed to the new ID.
    Rekeyed {
        old_id: ArchiveId,
        new_id: ArchiveId,
    },
    /// [`DuplicatePolicy::Reject`] found a colliding archive (by content hash or by
    /// `intended_filename`) and left it untouched — the caller's new file was not catalogued.
    Rejected {
        existing_id: ArchiveId,
        reason: DuplicateReason,
    },
}

/// How [`ingest_file`] should handle a colliding archive — either by content hash (the ID
/// computed for this file already belongs to a different tracked path) or by
/// `intended_filename` (a different archive already occupies that destination basename).
/// Every caller except a download-queue "start with overwrite" request uses [`Self::Reject`],
/// preserving the exact behavior every existing caller already had before this enum existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DuplicatePolicy {
    /// Leave the existing archive untouched; report [`IngestOutcome::Rejected`] instead of
    /// cataloguing the new file (the new file itself is not deleted — the caller decides that).
    #[default]
    Reject,
    /// Delete the existing archive (record + on-disk file) first, then catalogue the new file as
    /// a brand-new archive — the old ID is never reused, matching legacy's real `replacedupe`
    /// semantics (`~/LANraragi/lib/LANraragi/Model/Upload.pm::handle_incoming_file`).
    Overwrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateReason {
    /// The computed content-hash ID already belongs to a different tracked path.
    ContentHash,
    /// `intended_filename` already belongs to a different archive.
    Filename,
}

impl From<DuplicateReason> for lanrurugi_core::queue_error::DuplicateReasonKind {
    fn from(reason: DuplicateReason) -> Self {
        match reason {
            DuplicateReason::ContentHash => Self::ContentHash,
            DuplicateReason::Filename => Self::Filename,
        }
    }
}

/// Runs the watcher-driven ingestion loop: pulls paths off `rx` and ingests them one at a time
/// until the channel closes. Bulk/initial scans use `hashing::hash_batch` directly instead (they
/// parallelize hashing itself rather than serializing through this channel).
/// `new_archive_tx`, when given, receives the id of every genuinely brand-new archive this loop
/// catalogues (`IngestOutcome::Catalogued` only — never `Rekeyed`, matching legacy's own
/// `Shinobu.pm::update_filemap_entry`, whose rekey branch explicitly preserves old metadata and
/// returns without calling `exec_enabled_plugins_on_file`; only its "brand-new file" fallthrough,
/// `add_new_file`, does). This crate has no access to `AppState`/plugin execution (avoiding a
/// circular dependency on `lanrurugi-api`), so the actual "run every enabled metadata plugin on
/// this id" work happens on the *other* end of this channel, in the API crate — this loop only
/// ever reports which ids are eligible.
///
/// `locks` is the *same* `FilenameLocks` instance `lanrurugi-api`'s download-ingest path
/// (`download_manager::ingest::catalogue_staged_file`) reserves a filename in around its own
/// rename-into-`archive_dir`-then-catalogue sequence — without also acquiring it here, a file the
/// download path just renamed into the watched directory triggers this exact loop iteration
/// essentially simultaneously via the `notify` event for that rename, and both paths independently
/// ingest the same file with no mutual exclusion — the root cause of a real, observed corruption
/// bug (a 331MB archive left with `pagecount: 0` and an `arcsize` far under its real size, having
/// been ingested three times with two racing `Rekeyed` outcomes). By the time this acquires the
/// lock after the download path releases it, `ingest_file`'s own `Unchanged` branch (content hash
/// unchanged since the download path already catalogued it) makes this a safe, cheap no-op rather
/// than a second real ingestion.
pub async fn run(
    mut rx: mpsc::UnboundedReceiver<PathBuf>,
    archives: ArchiveRepository,
    config_pool: Pool,
    search_pool: Pool,
    thumb_dir: PathBuf,
    new_archive_tx: Option<mpsc::UnboundedSender<String>>,
    locks: FilenameLocks,
) {
    while let Some(path) = rx.recv().await {
        let filename = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        // Held across the entire ingest below (including its own stability wait), matching the
        // download path's own "held for the whole cataloguing sequence" scope — dropped when this
        // loop iteration's block ends, before the next `rx.recv()`.
        let _guard = locks.lock(&filename).await;

        match tokio::time::timeout(
            INGEST_TIMEOUT,
            ingest_file(&archives, &config_pool, &search_pool, &thumb_dir, &path),
        )
        .await
        {
            Ok(Ok(outcome)) => {
                if let (IngestOutcome::Catalogued { id }, Some(tx)) = (&outcome, &new_archive_tx) {
                    let _ = tx.send(id.to_string());
                }
                tracing::info!(?path, ?outcome, "ingested file")
            }
            Ok(Err(e)) => tracing::warn!(?path, error = %e, "failed to ingest file"),
            Err(_) => tracing::warn!(?path, timeout = ?INGEST_TIMEOUT, "ingestion timed out"),
        }
    }
}

pub async fn ingest_file(
    archives: &ArchiveRepository,
    config_pool: &Pool,
    search_pool: &Pool,
    thumb_dir: &Path,
    path: &Path,
) -> Result<IngestOutcome, PipelineError> {
    ingest_file_with_policy(
        archives,
        config_pool,
        search_pool,
        thumb_dir,
        path,
        IngestOptions::default(),
    )
    .await
}

/// How [`ingest_file_with_policy`] should handle a colliding archive, and what name a newly
/// catalogued archive's `title`/`name` should take.
#[derive(Debug, Clone, Copy, Default)]
pub struct IngestOptions<'a> {
    pub duplicate_policy: DuplicatePolicy,
    /// The destination basename this file will ultimately be known by (which may differ from
    /// `path`'s own basename, e.g. a staged upload written under a temp UUID name) — `None` skips
    /// the filename-collision check entirely (content-hash collision is still always checked).
    pub intended_filename: Option<&'a str>,
    /// Source for a newly catalogued archive's `title`/`name`, in preference to `path`'s own
    /// (possibly disposable-staging-name) basename — see `catalogue_new_archive`'s own docs for
    /// the real bug this fixes. Usually the same value as `intended_filename` (and every caller
    /// but one just sets both to the same thing via [`Self::named`]) — kept as its own separate
    /// field only because `download_manager::ingest::ingest_downloaded_file`'s `overwrite: false`
    /// path wants the title benefit *without* the collision check (a same-named-but-different-
    /// content download is expected there and should auto-rename around rather than reject), so it
    /// sets `intended_filename: None` but still needs a real name for `title_filename`.
    pub title_filename: Option<&'a str>,
    /// When `true`, a newly catalogued archive's `file` field is written EMPTY instead of
    /// `path` — for callers that ingest a *staged* file which will only be moved to its final
    /// `archive_dir` location afterward (download/upload ingest). The caller must fill `file`
    /// in after the move; if it never does (crash mid-way), the record shows an empty path
    /// rather than a dangling temp path whose bytes get swept — see `download_manager::ingest`'s
    /// fixup + the startup zombie-repair sweep in `main.rs`. The watcher path leaves this
    /// `false` (`path` IS already final there).
    pub defer_file_path: bool,
}

impl<'a> IngestOptions<'a> {
    /// The common case: `name` used for collision-checking and title on equal footing.
    pub fn named(duplicate_policy: DuplicatePolicy, name: &'a str) -> Self {
        Self {
            duplicate_policy,
            intended_filename: Some(name),
            title_filename: Some(name),
            defer_file_path: false,
        }
    }
}

/// Like [`ingest_file`], but with control over how a colliding archive is handled and what a
/// newly catalogued archive's `title`/`name` should be — see [`IngestOptions`].
pub async fn ingest_file_with_policy(
    archives: &ArchiveRepository,
    config_pool: &Pool,
    search_pool: &Pool,
    thumb_dir: &Path,
    path: &Path,
    options: IngestOptions<'_>,
) -> Result<IngestOutcome, PipelineError> {
    wait_until_stable(path).await?;
    let IngestOptions {
        duplicate_policy,
        intended_filename,
        title_filename,
        defer_file_path,
    } = options;

    let id = ArchiveId(size_aware_id(path)?);
    let path_str = path.to_string_lossy().to_string();

    let mut config_conn = config_pool.get().await?;
    let previous_id: Option<String> = config_conn.hget(FILEMAP_KEY, &path_str).await?;

    if let Some(previous_id) = previous_id.map(ArchiveId) {
        if previous_id == id {
            return Ok(IngestOutcome::Unchanged { id });
        }

        // Content changed: re-key the existing record (non-destructive — metadata carries over),
        // exactly matching legacy's `change_archive_id` semantics for a rewritten file.
        match archives.rename_id(&previous_id, &id).await {
            Ok(()) => {
                let _: () = config_conn
                    .hset(FILEMAP_KEY, &path_str, id.as_str())
                    .await?;
                return Ok(IngestOutcome::Rekeyed {
                    old_id: previous_id,
                    new_id: id,
                });
            }
            Err(e) if e.to_string().contains("no such key") => {
                // The filemap pointed at an ID that no longer exists — most likely
                // `lanrurugi_storage::rebuild::rekey_all` already re-keyed this exact archive by
                // its own (equivalent) path without going through this filemap entry. Fall
                // through to the "no recorded previous ID" path below: refresh the filemap and
                // treat `id` on its own merits (already tracked vs. genuinely new).
            }
            Err(e) => return Err(e.into()),
        }
    }

    if archives.get(&id).await?.is_some() {
        // Same content-hash ID already tracked under a different path. Under the size-aware
        // algorithm this can only happen for genuinely byte-identical content (Clarifications
        // Q2). Always rejected — unlike a `Filename` collision below, `DuplicatePolicy::Overwrite`
        // never applies here: deleting and recataloguing an archive whose bytes are provably
        // identical to what's already tracked would just churn its ID/timestamps for zero real
        // benefit (explicit product decision — a checkbox that silently does this on a true
        // duplicate would be confusing, not useful).
        return Ok(IngestOutcome::Rejected {
            existing_id: id,
            reason: DuplicateReason::ContentHash,
        });
    }

    if let Some(intended_filename) = intended_filename {
        if let Some(existing) = archives
            .find_by_exact_file_basename(intended_filename)
            .await?
        {
            if existing.id != id {
                match duplicate_policy {
                    DuplicatePolicy::Reject => {
                        return Ok(IngestOutcome::Rejected {
                            existing_id: existing.id,
                            reason: DuplicateReason::Filename,
                        });
                    }
                    DuplicatePolicy::Overwrite => {
                        delete_existing_archive(archives, config_pool, search_pool, &existing)
                            .await?;
                    }
                }
            }
        }
    }

    let _: () = config_conn
        .hset(FILEMAP_KEY, &path_str, id.as_str())
        .await?;

    let catalogue_settings = CatalogueSettings {
        thumb: crate::thumbnail::read_settings(&mut config_conn).await,
        date_added: read_date_added_settings(&mut config_conn).await,
        defer_file_path,
    };
    catalogue_new_archive(
        archives,
        search_pool,
        thumb_dir,
        &id,
        path,
        title_filename,
        catalogue_settings,
    )
    .await?;
    Ok(IngestOutcome::Catalogued { id })
}

#[derive(Debug, Clone, Copy)]
struct DateAddedSettings {
    enabled: bool,
    use_last_modified: bool,
}

/// The two independent settings groups `catalogue_new_archive` needs — bundled into one struct
/// purely to keep that function's own argument count under clippy's limit, not because thumbnail
/// generation and date-added tagging are conceptually related.
#[derive(Debug, Clone, Copy)]
struct CatalogueSettings {
    thumb: crate::thumbnail::ThumbSettings,
    date_added: DateAddedSettings,
    /// See [`IngestOptions::defer_file_path`]'s own docs — record gets an empty `file`,
    /// filled in later by the caller once the staged file reaches its final path.
    defer_file_path: bool,
}

/// Reads the live `usedateadded`/`usedatemodified` values from the same `LRR_CONFIG` hash
/// `lanrurugi-api::settings` reads/writes — mirrors `crate::thumbnail::read_settings`'s own
/// pattern. Matches `Model/Config.pm::enable_dateadded`/`use_lastmodified`'s real defaults
/// (`"1"`/`"0"` respectively).
async fn read_date_added_settings(conn: &mut deadpool_redis::Connection) -> DateAddedSettings {
    use deadpool_redis::redis::AsyncCommands;
    use lanrurugi_storage::keys::CONFIG_KEY;
    let fields: std::collections::HashMap<String, String> =
        conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    DateAddedSettings {
        enabled: fields.get("usedateadded").map(|v| v != "0").unwrap_or(true),
        use_last_modified: fields
            .get("usedatemodified")
            .map(|v| v != "0")
            .unwrap_or(false),
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Deletes an existing archive's record and its on-disk file (best-effort — a missing/already-
/// gone file is not an error) plus its own `LRR_FILEMAP` entry, as part of
/// [`DuplicatePolicy::Overwrite`]. The new file is catalogued as a brand-new archive afterward
/// (never reusing `existing`'s own ID), matching legacy's real `replacedupe` semantics.
async fn delete_existing_archive(
    archives: &ArchiveRepository,
    config_pool: &Pool,
    search_pool: &Pool,
    existing: &Archive,
) -> Result<(), PipelineError> {
    let _ = tokio::fs::remove_file(&existing.file).await;
    // A sidecar `.patch.zip` (issue #77's own follow-on design) is associated purely by filename
    // convention — left behind here, it would either sit as a dangling orphan (this archive's
    // record is gone) or, worse, get silently picked up by whatever new archive next lands on
    // this same filename (the `Overwrite`-policy caller's own new archive, about to be catalogued
    // right after this). Same cleanup `archives.rs::delete_archive`'s own docs already established
    // for the single-delete API path — best-effort, matching that path's reasoning.
    let _ = tokio::fs::remove_file(crate::patch::patch_path_for(Path::new(&existing.file))).await;
    archives.delete(&existing.id).await?;
    let mut config_conn = config_pool.get().await?;
    let _: () = config_conn.hdel(FILEMAP_KEY, &existing.file).await?;
    // Confirmed missing live (issue found during unrelated manual QA): without this, the deleted
    // archive's title/tags stayed in the search index (`LRR_TITLES`/tag sets), and since the new
    // archive being catalogued right after this reuses the exact same filename, a subsequent
    // library-list hover/search could still surface the OLD title/source/uploader tags — the new
    // archive's own real ones were correct once opened directly, only the index-backed summary
    // view was stale. Mirrors `archives.rs::delete_archive`'s own best-effort index cleanup
    // (a failure here shouldn't undo the deletion already committed above, just leave a ghost
    // index entry for a future rescan to reconcile, logged for visibility).
    if let Err(e) = lanrurugi_search::indexer::remove_archive_index(
        search_pool,
        existing.id.as_str(),
        &existing.title,
        &existing.tags,
    )
    .await
    {
        tracing::warn!(id = %existing.id, error = %e, "failed to remove overwritten archive from search index");
    }
    Ok(())
}

async fn catalogue_new_archive(
    archives: &ArchiveRepository,
    search_pool: &Pool,
    thumb_dir: &Path,
    id: &ArchiveId,
    path: &Path,
    title_filename: Option<&str>,
    settings: CatalogueSettings,
) -> Result<(), PipelineError> {
    let CatalogueSettings {
        thumb: thumb_settings,
        date_added: date_added_settings,
        defer_file_path,
    } = settings;
    // `title_filename` (when given) is the archive's real, destination-facing basename — used
    // in preference to `path`'s own basename since a caller staging a file under a disposable temp
    // name (`upload.rs`'s `upload-<uuid>`, the download pipeline's `download-<uuid>`) still wants
    // the archive's `title`/`name` to reflect the real filename the user/plugin actually intended,
    // not that throwaway staging name. A real, shipped bug until this fix: both the web-upload and
    // download-queue paths always pass a UUID-named staging path here, and with nothing to fall
    // back on, every archive ingested through either path got titled after the meaningless UUID
    // instead of its real name (confirmed live: uploading "Real Test Filename Should Show.zip"
    // produced an archive titled `upload-dcad970d75f9...`, and downloading "[King Angel] Maiden
    // Under the Sun" produced `download-9602cda7e6d0...`). The watcher/full-scan callers correctly
    // pass `None` here, since a file discovered already sitting in the watched archive directory
    // already has its own real name as its `path`.
    let name = title_filename
        .and_then(|f| Path::new(f).file_stem())
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string()
        });
    let arcsize = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let pagecount = archive_format::list_pages(path)
        .map(|p| p.len() as u32)
        .unwrap_or(0);

    // `Utils/Database.pm::add_timestamp_tag`, called by both legacy's watcher (`Shinobu.pm:386`)
    // and its upload handler (`Model/Upload.pm:169`) right after a new archive is first
    // catalogued — mirrored here so every ingestion path (watcher, full scan, upload, download)
    // gets the same tag for free via this one shared function, same as legacy's own two call
    // sites both just delegate to one shared helper.
    let tags = if date_added_settings.enabled {
        let timestamp = if date_added_settings.use_last_modified {
            std::fs::metadata(path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or_else(now_unix)
        } else {
            now_unix()
        };
        format!("date_added:{timestamp}")
    } else {
        String::new()
    };

    // A sidecar `.patch.zip` (issue #77's own follow-on design) may already sit next to this file
    // at first-scan time — e.g. a user drops both a manually-authored patch and its target archive
    // into the library directory together, rather than only ever via the compare-flow endpoints
    // that write one server-side. Checked here so a fresh catalogue doesn't start with a stale
    // `has_patch: false` that would only get corrected the next time some other code path happens
    // to touch this archive's record.
    let has_patch = crate::patch::patch_path_for(path).exists();
    let mut archive = Archive {
        id: id.clone(),
        name: name.clone(),
        title: name,
        // `defer_file_path` callers (download/upload ingest of a *staged* file) get an empty
        // `file` here and fill it in after the move to `archive_dir` — never the staging path
        // itself, whose bytes get swept and would leave a dangling record (see
        // `IngestOptions::defer_file_path`'s own docs).
        file: if defer_file_path {
            String::new()
        } else {
            path.to_string_lossy().to_string()
        },
        tags,
        summary: String::new(),
        arcsize,
        pagecount,
        isnew: true,
        lastreadpage: 0,
        lastreadtime: 0,
        thumbhash: None,
        toc: Vec::new(),
        stamp_ids: Vec::new(),
        heal_failed_at: None,
        corrupted_pages: Vec::new(),
        has_patch,
    };
    archives.save(&archive).await?;
    if let Err(e) =
        lanrurugi_search::indexer::index_new_archive(search_pool, id.as_str(), &archive.title).await
    {
        tracing::warn!(%id, error = %e, "failed to index new archive for search");
    }

    if pagecount > 0 {
        let shard = &id.as_str()[0..2.min(id.len())];
        let output = thumb_dir
            .join(shard)
            .join(format!("{id}.{}", thumb_settings.format.extension()));
        match crate::thumbnail::generate(
            path.to_path_buf(),
            1,
            output,
            thumb_settings.format,
            thumb_settings.quality,
        )
        .await
        {
            Ok(thumbhash) => {
                archive.thumbhash = thumbhash;
                archives.save(&archive).await?;
            }
            Err(e) => {
                tracing::warn!(%id, error = %e, "thumbnail generation failed for new archive")
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // See full_scan.rs's own identical helper for why this takes a db offset — both modules'
    // tests hit the same global `LRR_FILEMAP` key, so `cargo test`'s default parallel-by-thread
    // execution let one test's `hset`/`hdel` interleave with another's read, a real (if rare)
    // source of CI flakiness. This module's two tests use offsets disjoint from full_scan.rs's
    // three (which claim 0-8), so no two tests across either module ever share a Redis DB.
    async fn test_pools(
        db_offset: u8,
    ) -> Option<(
        deadpool_redis::Pool,
        deadpool_redis::Pool,
        deadpool_redis::Pool,
    )> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let url = |db: u8| format!("{}/{db}", base.trim_end_matches('/'));
        let archive = lanrurugi_storage::test_support::test_pool_for_url(&url(db_offset)).await?;
        let config =
            lanrurugi_storage::test_support::test_pool_for_url(&url(db_offset + 1)).await?;
        let search =
            lanrurugi_storage::test_support::test_pool_for_url(&url(db_offset + 2)).await?;
        Some((archive, config, search))
    }

    fn make_zip_with_pages(dir: &Path, name: &str, n_pages: usize) -> PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        use std::io::Write;
        for i in 0..n_pages {
            writer.start_file(format!("page{i}.jpg"), options).unwrap();
            writer.write_all(b"fake image bytes").unwrap();
        }
        writer.finish().unwrap();
        path
    }

    #[tokio::test]
    async fn new_file_is_catalogued_and_filemap_updated() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(9).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();
        let path = make_zip_with_pages(dir.path(), "book.zip", 3);

        let outcome = ingest_file(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &path,
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id } = outcome else {
            panic!("expected Catalogued, got {outcome:?}");
        };

        let archive = archives.get(&id).await.unwrap().unwrap();
        assert_eq!(archive.pagecount, 3);
        assert!(archive.isnew);

        // Re-ingesting the same byte-identical file should be a no-op, not a duplicate.
        let second = ingest_file(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &path,
        )
        .await
        .unwrap();
        assert_eq!(second, IngestOutcome::Unchanged { id: id.clone() });

        archives.delete(&id).await.unwrap();
        let mut conn = config_pool.get().await.unwrap();
        let _: () = conn
            .hdel(FILEMAP_KEY, path.to_string_lossy().to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn title_filename_overrides_a_disposable_staging_path_basename() {
        // Regression test for a real, shipped bug: `upload.rs`/`download_manager::ingest.rs` both
        // stage their file under a throwaway UUID name (`upload-<uuid>`/`download-<uuid>`) before
        // ingesting, and — before `title_filename` existed — `catalogue_new_archive` always
        // derived `title`/`name` from *that* staging path's own basename, so every uploaded or
        // downloaded archive ended up titled after a meaningless UUID instead of its real name.
        let Some((archive_pool, config_pool, search_pool)) = test_pools(24).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();
        // Named the way a real staging file is: a disposable, meaningless basename that must NOT
        // end up as the archive's title once `title_filename` is given.
        let staged_path = make_zip_with_pages(dir.path(), "download-9602cda7e6d04444.zip", 2);

        let outcome = ingest_file_with_policy(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &staged_path,
            IngestOptions {
                // No collision check (`intended_filename: None`) — mirrors
                // `ingest_downloaded_file`'s own real `overwrite: false` case exactly.
                title_filename: Some("[King Angel] Maiden Under the Sun.zip"),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id } = outcome else {
            panic!("expected Catalogued, got {outcome:?}");
        };

        let archive = archives.get(&id).await.unwrap().unwrap();
        assert_eq!(archive.title, "[King Angel] Maiden Under the Sun");
        assert_eq!(archive.name, "[King Angel] Maiden Under the Sun");
        // The on-disk `file` path is untouched by `title_filename` — still the real staging path
        // at this point (a caller's own separate rename-into-place step, already covered by
        // `upload.rs`/`ingest_downloaded_file`'s own logic, isn't exercised by this pipeline-level
        // function).
        assert_eq!(archive.file, staged_path.to_string_lossy());

        archives.delete(&id).await.unwrap();
        let mut conn = config_pool.get().await.unwrap();
        let _: () = conn
            .hdel(FILEMAP_KEY, staged_path.to_string_lossy().to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn changed_file_content_rekeys_existing_archive() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(12).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();
        let path = make_zip_with_pages(dir.path(), "book.zip", 1);

        let first = ingest_file(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &path,
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id: old_id } = first else {
            panic!("expected Catalogued");
        };

        // Rewrite the file with different content -> different ID.
        let _ = make_zip_with_pages(dir.path(), "book.zip", 5);
        let second = ingest_file(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &path,
        )
        .await
        .unwrap();
        let IngestOutcome::Rekeyed {
            old_id: reported_old,
            new_id,
        } = second
        else {
            panic!("expected Rekeyed, got {second:?}");
        };
        assert_eq!(reported_old, old_id);
        assert!(archives.get(&old_id).await.unwrap().is_none());
        assert!(archives.get(&new_id).await.unwrap().is_some());

        archives.delete(&new_id).await.unwrap();
        let mut conn = config_pool.get().await.unwrap();
        let _: () = conn
            .hdel(FILEMAP_KEY, path.to_string_lossy().to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn overwrite_policy_still_rejects_a_content_hash_collision() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(15).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // Two distinct paths, byte-identical content -> the size-aware ID is a deterministic
        // content hash, so both necessarily resolve to the *same* ID. `DuplicatePolicy::Overwrite`
        // does NOT apply to a content-hash collision (explicit product decision — deleting and
        // recataloguing an archive whose bytes are provably identical to what's already tracked
        // would just churn its ID/timestamps for zero benefit); only a `Filename` collision (see
        // the sibling test below) still respects `Overwrite`.
        let first_path = make_zip_with_pages(dir.path(), "first.zip", 2);
        let first = ingest_file(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &first_path,
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id: first_id } = first else {
            panic!("expected Catalogued, got {first:?}");
        };

        std::fs::copy(&first_path, dir.path().join("second.zip")).unwrap();
        let second_path = dir.path().join("second.zip");

        let second = ingest_file_with_policy(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &second_path,
            IngestOptions {
                duplicate_policy: DuplicatePolicy::Overwrite,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            second,
            IngestOutcome::Rejected {
                existing_id: first_id.clone(),
                reason: DuplicateReason::ContentHash,
            }
        );

        // The original archive/file must be left fully untouched by the rejected attempt.
        assert!(first_path.exists(), "original archive's file must survive");
        let original = archives.get(&first_id).await.unwrap().unwrap();
        assert_eq!(
            std::path::Path::new(&original.file).file_name().unwrap(),
            "first.zip"
        );

        archives.delete(&first_id).await.unwrap();
        let mut conn = config_pool.get().await.unwrap();
        let _: () = conn
            .hdel(FILEMAP_KEY, first_path.to_string_lossy().to_string())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn overwrite_policy_deletes_old_archive_on_filename_collision() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(18).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();
        // Unique per run — `find_by_exact_file_basename` does a full `list_all()` scan and takes
        // the first match, so a fixed literal here can collide with a *previous* (possibly
        // panicked-before-cleanup) test run's own leftover record in this same shared test Redis
        // DB, silently deleting the wrong stale archive instead of this run's real `existing` one
        // (confirmed live as a real, reproducible flake, 2026-08-31 — same root cause
        // `activity.rs`'s own test-timestamp doc comment already documents for a different
        // fixed-literal collision). No `FLUSHDB` needed once the name itself can't collide.
        let shared_name = format!("shared-name-{}.zip", uuid::Uuid::new_v4());

        // The "existing" archive: catalogued under its own staging path, but its stored `file`
        // basename (`shared_name`) is what the collision check below matches against — mirroring
        // how a real download's staging path differs from its intended filename.
        let existing_staged_path = make_zip_with_pages(dir.path(), "existing-staged.zip", 1);
        let existing = ingest_file(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &existing_staged_path,
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id: existing_id } = existing else {
            panic!("expected Catalogued, got {existing:?}");
        };
        let mut existing_archive = archives.get(&existing_id).await.unwrap().unwrap();
        existing_archive.file = dir.path().join(&shared_name).to_string_lossy().to_string();
        archives.save(&existing_archive).await.unwrap();
        // The "existing" archive's real file, at the filename the new file will collide with.
        std::fs::copy(&existing_staged_path, dir.path().join(&shared_name)).unwrap();

        // A new, content-distinct file staged under its own temp name but destined for that same
        // `shared_name` basename.
        let new_staged_path = make_zip_with_pages(dir.path(), "new-staged.zip", 4);

        let outcome = ingest_file_with_policy(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &new_staged_path,
            IngestOptions::named(DuplicatePolicy::Overwrite, &shared_name),
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id: new_id } = outcome else {
            panic!("expected Catalogued (old archive replaced), got {outcome:?}");
        };
        assert_ne!(new_id, existing_id);

        assert!(
            archives.get(&existing_id).await.unwrap().is_none(),
            "the filename-colliding archive should have been deleted, not reused"
        );
        let replacement = archives.get(&new_id).await.unwrap().unwrap();
        assert_eq!(replacement.pagecount, 4);
        assert!(
            !dir.path().join(&shared_name).exists(),
            "old archive's on-disk file should be gone (new file is still at its staged path)"
        );

        // Confirmed missing live: the deleted archive's search-index title entry must go with it,
        // or a library-list hover could keep surfacing the overwritten archive's stale
        // title/tags/source even after the new archive replaced it on disk under the same name.
        let mut search_conn = search_pool.get().await.unwrap();
        let stale_score: Option<f64> = search_conn
            .zscore(
                lanrurugi_search::keys::TITLES_KEY,
                format!("{}\0{}", existing_archive.title.to_lowercase(), existing_id),
            )
            .await
            .unwrap();
        assert_eq!(
            stale_score, None,
            "overwritten archive's title should no longer be in the search index"
        );

        archives.delete(&new_id).await.unwrap();
        let mut conn = config_pool.get().await.unwrap();
        let _: () = conn
            .hdel(
                FILEMAP_KEY,
                &[
                    existing_staged_path.to_string_lossy().to_string(),
                    new_staged_path.to_string_lossy().to_string(),
                ],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn reject_policy_reports_rejected_on_content_hash_collision() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(21).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        let first_path = make_zip_with_pages(dir.path(), "first.zip", 2);
        let first = ingest_file(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &first_path,
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id: first_id } = first else {
            panic!("expected Catalogued, got {first:?}");
        };

        std::fs::copy(&first_path, dir.path().join("second.zip")).unwrap();
        let second_path = dir.path().join("second.zip");

        // Explicit `DuplicatePolicy::Reject` (what `ingest_file` itself always uses) must behave
        // identically to `ingest_file`'s own pre-existing `Unchanged`-on-collision behavior, just
        // surfaced through the new `Rejected` variant instead.
        let second = ingest_file_with_policy(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &second_path,
            IngestOptions::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            second,
            IngestOutcome::Rejected {
                existing_id: first_id.clone(),
                reason: DuplicateReason::ContentHash,
            }
        );
        assert!(
            archives.get(&first_id).await.unwrap().is_some(),
            "the existing archive must be untouched under Reject"
        );
        assert!(
            second_path.exists(),
            "the new file itself is not deleted by ingest_file_with_policy"
        );

        archives.delete(&first_id).await.unwrap();
        let mut conn = config_pool.get().await.unwrap();
        let _: () = conn
            .hdel(
                FILEMAP_KEY,
                &[
                    first_path.to_string_lossy().to_string(),
                    second_path.to_string_lossy().to_string(),
                ],
            )
            .await
            .unwrap();
    }

    /// The scenario `run()`'s new `locks` parameter exists for: another caller (standing in for
    /// `lanrurugi-api`'s download-ingest path) already holds this exact filename's lock when a
    /// filesystem event for it arrives — `run()` must not ingest until that lock is released.
    /// Doesn't need a real `notify` watcher — `run()` only consumes an `mpsc` receiver, so this
    /// sends one path through a plain channel to simulate the event.
    #[tokio::test]
    async fn run_blocks_on_a_filename_lock_already_held_by_another_caller() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(27).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();
        let path = make_zip_with_pages(dir.path(), "book.zip", 2);

        let locks = lanrurugi_core::filename_lock::FilenameLocks::new();
        // Simulates the download path already holding this filename's lock across its own
        // rename-then-catalogue window.
        let held_guard = locks.lock("book.zip").await;

        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(path.clone()).unwrap();
        drop(tx); // Closes the channel so `run` returns once this one item is drained.

        let run_locks = locks.clone();
        let run_task = tokio::spawn(run(
            rx,
            archives.clone(),
            config_pool.clone(),
            search_pool,
            thumb_dir.path().to_path_buf(),
            None,
            run_locks,
        ));

        // `run` must not have catalogued the file yet — it's blocked waiting on the lock
        // `held_guard` still holds.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !run_task.is_finished(),
            "run() must block on the lock rather than ingesting immediately"
        );

        drop(held_guard);
        tokio::time::timeout(std::time::Duration::from_secs(5), run_task)
            .await
            .expect("run() must finish once the lock is released")
            .unwrap();

        // Sanity: the file was in fact catalogued once the lock freed up.
        let mut conn = config_pool.get().await.unwrap();
        let id: Option<String> = conn
            .hget(FILEMAP_KEY, path.to_string_lossy().to_string())
            .await
            .unwrap();
        assert!(
            id.is_some(),
            "run() must have catalogued the file after unblocking"
        );

        archives.delete(&ArchiveId(id.unwrap())).await.unwrap();
        let _: () = conn
            .hdel(FILEMAP_KEY, path.to_string_lossy().to_string())
            .await
            .unwrap();
    }
}
