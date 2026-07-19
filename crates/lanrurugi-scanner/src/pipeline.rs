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
use lanrurugi_storage::repository::ArchiveRepository;
use thiserror::Error;
use tokio::sync::mpsc;

use crate::archive_format;
use crate::watcher::wait_until_stable;

pub(crate) const FILEMAP_KEY: &str = "LRR_FILEMAP";
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

    let _: () = config_conn.hset(FILEMAP_KEY, &path_str, &id).await?;

    if archives.get(&id).await?.is_some() {
        // Same ID already tracked under a different path. Under the size-aware algorithm this
        // can only happen for genuinely byte-identical content (Clarifications Q2) — nothing to
        // catalogue, just record the filemap pointer above so a future rename is detected too.
        return Ok(IngestOutcome::Unchanged { id });
    }

    let thumb_settings = crate::thumbnail::read_settings(&mut config_conn).await;
    catalogue_new_archive(archives, search_pool, thumb_dir, &id, path, thumb_settings).await?;
    Ok(IngestOutcome::Catalogued { id })
}

async fn catalogue_new_archive(
    archives: &ArchiveRepository,
    search_pool: &Pool,
    thumb_dir: &Path,
    id: &str,
    path: &Path,
    thumb_settings: crate::thumbnail::ThumbSettings,
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

    let mut archive = Archive {
        id: id.to_string(),
        name: name.clone(),
        title: name,
        file: path.to_string_lossy().to_string(),
        tags: String::new(),
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
}
