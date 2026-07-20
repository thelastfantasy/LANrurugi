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
    Catalogued { id: String },
    /// The path was already tracked under this same ID — no change (e.g. duplicate inotify
    /// events, or a byte-identical rewrite).
    Unchanged { id: String },
    /// The path's content changed; its existing Archive record was re-keyed to the new ID.
    Rekeyed { old_id: String, new_id: String },
    /// [`DuplicatePolicy::Reject`] found a colliding archive (by content hash or by
    /// `intended_filename`) and left it untouched — the caller's new file was not catalogued.
    Rejected {
        existing_id: String,
        reason: DuplicateReason,
    },
}

/// How [`ingest_file`] should handle a colliding archive — either by content hash (the ID
/// computed for this file already belongs to a different tracked path) or by
/// `intended_filename` (a different archive already occupies that destination basename).
/// Every caller except a download-queue "start with overwrite" request uses [`Self::Reject`],
/// preserving the exact behavior every existing caller already had before this enum existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicatePolicy {
    /// Leave the existing archive untouched; report [`IngestOutcome::Rejected`] instead of
    /// cataloguing the new file (the new file itself is not deleted — the caller decides that).
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

/// Runs the watcher-driven ingestion loop: pulls paths off `rx` and ingests them one at a time
/// until the channel closes. Bulk/initial scans use `hashing::hash_batch` directly instead (they
/// parallelize hashing itself rather than serializing through this channel).
pub async fn run(
    mut rx: mpsc::UnboundedReceiver<PathBuf>,
    archives: ArchiveRepository,
    config_pool: Pool,
    search_pool: Pool,
    thumb_dir: PathBuf,
) {
    while let Some(path) = rx.recv().await {
        match tokio::time::timeout(
            INGEST_TIMEOUT,
            ingest_file(&archives, &config_pool, &search_pool, &thumb_dir, &path),
        )
        .await
        {
            Ok(Ok(outcome)) => tracing::info!(?path, ?outcome, "ingested file"),
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
        DuplicatePolicy::Reject,
        None,
    )
    .await
}

/// Like [`ingest_file`], but with control over how a colliding archive (by content hash or by
/// `intended_filename`) is handled — see [`DuplicatePolicy`]. `intended_filename` is the
/// destination basename this file will ultimately be known by (which may differ from `path`'s
/// own basename, e.g. a staged upload written under a temp UUID name) — `None` skips the
/// filename-collision check entirely (content-hash collision is still always checked).
pub async fn ingest_file_with_policy(
    archives: &ArchiveRepository,
    config_pool: &Pool,
    search_pool: &Pool,
    thumb_dir: &Path,
    path: &Path,
    duplicate_policy: DuplicatePolicy,
    intended_filename: Option<&str>,
) -> Result<IngestOutcome, PipelineError> {
    wait_until_stable(path).await?;

    let id = size_aware_id(path)?;
    let path_str = path.to_string_lossy().to_string();

    let mut config_conn = config_pool.get().await?;
    let previous_id: Option<String> = config_conn.hget(FILEMAP_KEY, &path_str).await?;

    if let Some(previous_id) = previous_id {
        if previous_id == id {
            return Ok(IngestOutcome::Unchanged { id });
        }

        // Content changed: re-key the existing record (non-destructive — metadata carries over),
        // exactly matching legacy's `change_archive_id` semantics for a rewritten file.
        match archives.rename_id(&previous_id, &id).await {
            Ok(()) => {
                let _: () = config_conn.hset(FILEMAP_KEY, &path_str, &id).await?;
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

    if let Some(existing) = archives.get(&id).await? {
        // Same content-hash ID already tracked under a different path. Under the size-aware
        // algorithm this can only happen for genuinely byte-identical content (Clarifications
        // Q2).
        match duplicate_policy {
            DuplicatePolicy::Reject => {
                return Ok(IngestOutcome::Rejected {
                    existing_id: id,
                    reason: DuplicateReason::ContentHash,
                });
            }
            DuplicatePolicy::Overwrite => {
                delete_existing_archive(archives, config_pool, &existing).await?;
            }
        }
    }

    if let Some(intended_filename) = intended_filename {
        if let Some(existing) = archives.find_by_filename(intended_filename).await? {
            if existing.id != id {
                match duplicate_policy {
                    DuplicatePolicy::Reject => {
                        return Ok(IngestOutcome::Rejected {
                            existing_id: existing.id,
                            reason: DuplicateReason::Filename,
                        });
                    }
                    DuplicatePolicy::Overwrite => {
                        delete_existing_archive(archives, config_pool, &existing).await?;
                    }
                }
            }
        }
    }

    let _: () = config_conn.hset(FILEMAP_KEY, &path_str, &id).await?;

    let thumb_settings = crate::thumbnail::read_settings(&mut config_conn).await;
    let date_added_settings = read_date_added_settings(&mut config_conn).await;
    catalogue_new_archive(
        archives,
        search_pool,
        thumb_dir,
        &id,
        path,
        thumb_settings,
        date_added_settings,
    )
    .await?;
    Ok(IngestOutcome::Catalogued { id })
}

#[derive(Debug, Clone, Copy)]
struct DateAddedSettings {
    enabled: bool,
    use_last_modified: bool,
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
    existing: &Archive,
) -> Result<(), PipelineError> {
    let _ = tokio::fs::remove_file(&existing.file).await;
    archives.delete(&existing.id).await?;
    let mut config_conn = config_pool.get().await?;
    let _: () = config_conn.hdel(FILEMAP_KEY, &existing.file).await?;
    Ok(())
}

async fn catalogue_new_archive(
    archives: &ArchiveRepository,
    search_pool: &Pool,
    thumb_dir: &Path,
    id: &str,
    path: &Path,
    thumb_settings: crate::thumbnail::ThumbSettings,
    date_added_settings: DateAddedSettings,
) -> Result<(), PipelineError> {
    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
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

    let mut archive = Archive {
        id: id.to_string(),
        name: name.clone(),
        title: name,
        file: path.to_string_lossy().to_string(),
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
    };
    archives.save(&archive).await?;
    if let Err(e) =
        lanrurugi_search::indexer::index_new_archive(search_pool, id, &archive.title).await
    {
        tracing::warn!(%id, error = %e, "failed to index new archive for search");
    }

    if pagecount > 0 {
        let shard = &id[0..2.min(id.len())];
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
    fn test_pools(
        db_offset: u8,
    ) -> Option<(
        deadpool_redis::Pool,
        deadpool_redis::Pool,
        deadpool_redis::Pool,
    )> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let mk = |db: u8| {
            deadpool_redis::Config::from_url(format!("{}/{db}", base.trim_end_matches('/')))
                .create_pool(Some(deadpool_redis::Runtime::Tokio1))
        };
        Some((
            mk(db_offset).ok()?,
            mk(db_offset + 1).ok()?,
            mk(db_offset + 2).ok()?,
        ))
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
        let Some((archive_pool, config_pool, search_pool)) = test_pools(9) else {
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
    async fn changed_file_content_rekeys_existing_archive() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(12) else {
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
    async fn overwrite_policy_recatalogues_at_new_path_on_content_hash_collision() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(15) else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // Two distinct paths, byte-identical content -> the size-aware ID is a deterministic
        // content hash, so both necessarily resolve to the *same* ID — "delete old, catalogue new
        // under a fresh ID" is only meaningful for the filename-collision case (see the sibling
        // test below); here, `Overwrite` deletes the old on-disk file and re-catalogues at the
        // new path, which — given the ID is content-derived — is inherently the same ID, just
        // pointed at the new location instead of silently doing nothing (the `Reject`/`Unchanged`
        // behavior).
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
            DuplicatePolicy::Overwrite,
            None,
        )
        .await
        .unwrap();
        let IngestOutcome::Catalogued { id: second_id } = second else {
            panic!("expected Catalogued, got {second:?}");
        };
        assert_eq!(second_id, first_id, "content-derived ID is deterministic");

        let replacement = archives.get(&second_id).await.unwrap().unwrap();
        assert_eq!(
            std::path::Path::new(&replacement.file).file_name().unwrap(),
            "second.zip"
        );
        assert!(
            !first_path.exists(),
            "old archive's on-disk file should be gone"
        );

        archives.delete(&second_id).await.unwrap();
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

    #[tokio::test]
    async fn overwrite_policy_deletes_old_archive_on_filename_collision() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(18) else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // The "existing" archive: catalogued under its own staging path, but its stored `file`
        // basename ("shared-name.zip") is what the collision check below matches against —
        // mirroring how a real download's staging path differs from its intended filename.
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
        existing_archive.file = dir
            .path()
            .join("shared-name.zip")
            .to_string_lossy()
            .to_string();
        archives.save(&existing_archive).await.unwrap();
        // The "existing" archive's real file, at the filename the new file will collide with.
        std::fs::copy(&existing_staged_path, dir.path().join("shared-name.zip")).unwrap();

        // A new, content-distinct file staged under its own temp name but destined for that same
        // "shared-name.zip" basename.
        let new_staged_path = make_zip_with_pages(dir.path(), "new-staged.zip", 4);

        let outcome = ingest_file_with_policy(
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &new_staged_path,
            DuplicatePolicy::Overwrite,
            Some("shared-name.zip"),
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
            !dir.path().join("shared-name.zip").exists(),
            "old archive's on-disk file should be gone (new file is still at its staged path)"
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
        let Some((archive_pool, config_pool, search_pool)) = test_pools(21) else {
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
            DuplicatePolicy::Reject,
            None,
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
}
