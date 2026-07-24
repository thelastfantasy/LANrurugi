//! Full-directory scan: walks `library_path` recursively and ingests every *genuinely new* archive
//! file found, reusing `pipeline::ingest_file`'s existing new-vs-unchanged-vs-rekey logic per file.
//! Used by bulk operations (User Story 6's rebuild-index, and an initial cold-start scan at `serve`
//! startup) rather than relying solely on the `notify` watcher's live event stream.
//!
//! **Verified against source** (`~/LANraragi/lib/Shinobu.pm::update_filemap`): legacy's own
//! startup scan does a *cheap, path-only* diff first — `LRR_FILEMAP`'s keys (already-tracked
//! paths) vs. what's actually on disk, a single `HKEYS` plus an in-memory `grep` — and only pays
//! the expensive per-file ID computation (`compute_id`, this rewrite's `size_aware_id`: a 512000-
//! byte read) for paths that survive that filter, i.e. paths *not yet* in the filemap at all.
//! Already-tracked paths are never re-hashed by a bulk scan; legacy only re-validates an
//! already-tracked path's content reactively, via inotify's own "modify" event
//! (`add_to_filemap`/`update_filemap_entry`, mirrored here by `watcher.rs` → `pipeline::ingest_file`
//! being called per live filesystem event, not by this module). An earlier version of this
//! function skipped that pre-filter and called `ingest_file` (and therefore `size_aware_id`)
//! unconditionally for every walked path, making a bulk scan of a large, otherwise-unchanged
//! library re-hash its entire contents every time — real, measurable I/O this rewrite was paying
//! that legacy never did.
//!
//! After `lanrurugi_storage::rebuild::rekey_all` re-keys every already-tracked archive whose
//! content silently changed (T074 — itself a Rust-rewrite-only consistency check with no direct
//! legacy equivalent, so it's *not* run automatically at `serve` startup, only via the explicit
//! `rebuild-index` CLI/API), a historically false-merged pair's previously-invisible sibling file
//! is still just an ordinary file on disk with no Redis record and no filemap entry at all — this
//! scan is what actually discovers and catalogues it, completing data-model.md's "Rebuild/Reindex
//! operation" (see that module's docs for why the two steps can't be combined into one).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_storage::repository::ArchiveRepository;
use tokio::sync::Semaphore;

use crate::pipeline::{ingest_file, IngestOutcome, FILEMAP_KEY};
use crate::watcher::is_watched_archive_path;

/// Bounding concurrency rather than spawning one task per file unconditionally keeps this
/// resource-conscious at the SC-008 target scale (~100k files) while still giving multiple files'
/// I/O and hashing genuine overlap (constitution Principle III, FR-022) instead of the serial
/// one-file-at-a-time loop this function used to run.
fn scan_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        * 4
}

#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    /// Total files found by the directory walk (matches legacy's own log message shape — "found
    /// N files" — before the cheap already-tracked filter below is applied).
    pub scanned: usize,
    /// Walked paths already present in `LRR_FILEMAP` — skipped without any file I/O or hashing at
    /// all, matching legacy's own `update_filemap` (`grep { !$filemaphash{$_} } @files`).
    pub already_known: usize,
    pub catalogued: usize,
    pub rekeyed: usize,
    pub unchanged: usize,
    pub errors: usize,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if is_watched_archive_path(&path) {
            out.push(path);
        }
    }
}

/// `new_archive_tx`, when given, receives the id of every genuinely brand-new archive this scan
/// catalogues — same semantics as `pipeline::run`'s own parameter of the same name (only
/// `Catalogued`, never `Rekeyed`/`Unchanged`/`Rejected`; see that function's own docs for why).
#[allow(clippy::too_many_arguments)]
pub async fn full_scan(
    library_path: &Path,
    archives: &ArchiveRepository,
    config_pool: &Pool,
    search_pool: &Pool,
    thumb_dir: &Path,
    jobs: &JobRegistry,
    job_id: &str,
    new_archive_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
) -> ScanSummary {
    let mut paths = Vec::new();
    walk(library_path, &mut paths);
    let mut summary = ScanSummary {
        scanned: paths.len(),
        ..Default::default()
    };

    // The cheap path-only pre-filter (see this module's own docs) — a single `HKEYS` plus
    // in-memory set operations, no file I/O. A Redis error here just disables the optimization
    // for this run (every path falls through to `ingest_file` as before, which is still correct,
    // just not fast) rather than failing the whole scan over it.
    let on_disk: HashSet<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    if let Ok(mut conn) = config_pool.get().await {
        let known_paths: HashSet<String> = conn.hkeys(FILEMAP_KEY).await.unwrap_or_default();

        // Prune filemap entries for paths no longer on disk — matches legacy's own
        // `update_filemap` (`@deletedfiles = grep { !$fshash{$_} } @filemapfiles`). Only touches
        // the filemap's path→id bookkeeping, not the Archive record itself (a missing-file
        // archive is a separate, existing "Clean Database" concern, not this scan's job).
        let stale: Vec<&String> = known_paths.difference(&on_disk).collect();
        if !stale.is_empty() {
            let _: Result<(), _> = conn.hdel(FILEMAP_KEY, stale).await;
        }

        let before = paths.len();
        paths.retain(|p| !known_paths.contains(&p.to_string_lossy().to_string()));
        summary.already_known = before - paths.len();
    }

    let total = paths.len().max(1);
    let semaphore = Arc::new(Semaphore::new(scan_concurrency()));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();

    for path in paths {
        let semaphore = semaphore.clone();
        let archives = archives.clone();
        let config_pool = config_pool.clone();
        let search_pool = search_pool.clone();
        let thumb_dir = thumb_dir.to_path_buf();
        let jobs = jobs.clone();
        let job_id = job_id.to_string();
        let completed = completed.clone();

        tasks.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            let result =
                ingest_file(&archives, &config_pool, &search_pool, &thumb_dir, &path).await;
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            jobs.set_progress(&job_id, done as f32 / total as f32).await;
            (path, result)
        });
    }

    while let Some(joined) = tasks.join_next().await {
        // A panicked/cancelled task must not take the rest of the scan down with it — every
        // other spawned task in this `JoinSet` is still running independently and its own result
        // still needs to be collected by a later loop iteration. Counted alongside `errors` (the
        // same bucket a normal `ingest_file` failure lands in) since, from the scan's own
        // perspective, this file simply failed to ingest — the *reason* (panic vs. a returned
        // `Err`) doesn't change what the caller needs to do about it.
        let (path, result) = match joined {
            Ok(pair) => pair,
            Err(join_err) => {
                tracing::warn!(error = %join_err, "full_scan: an ingest task panicked or was cancelled");
                summary.errors += 1;
                continue;
            }
        };
        match result {
            Ok(IngestOutcome::Catalogued { id }) => {
                summary.catalogued += 1;
                if let Some(tx) = &new_archive_tx {
                    let _ = tx.send(id.clone());
                }
            }
            Ok(IngestOutcome::Rekeyed { .. }) => summary.rekeyed += 1,
            Ok(IngestOutcome::Unchanged { .. }) => summary.unchanged += 1,
            // `full_scan`/the watcher's own `run()` always call with `DuplicatePolicy::Reject,
            // None` (no filename check), so this arm is only reachable via a content-hash
            // collision — under the size-aware ID algorithm that means genuinely byte-identical
            // content, i.e. the same case `Unchanged` already covers when the path itself was
            // already tracked. Counted alongside `unchanged` rather than `errors`: nothing went
            // wrong, there's just nothing new to catalogue.
            Ok(IngestOutcome::Rejected { .. }) => summary.unchanged += 1,
            Err(e) => {
                tracing::warn!(?path, error = %e, "full_scan: failed to ingest file");
                summary.errors += 1;
            }
        }
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::entities::Archive;

    // `cargo test`'s default parallel-by-thread execution means every test in this module (and
    // in pipeline.rs's own test module, which used the exact same hardcoded db indices) ran
    // concurrently against the very same Redis databases — and both modules' tests read/write the
    // single global `LRR_FILEMAP` key, so one test's `hset`/`hdel` could interleave with another
    // test's `hkeys` scan mid-flight. This surfaced as a genuinely intermittent CI failure (not
    // reproducible locally under lighter load): `already_tracked_paths_are_skipped_without_rehashing`
    // asserting `already_known == 1` but observing `0`, meaning some other concurrently-running
    // test's `hdel`/fresh-db-state won the race against this test's own `hset` before `full_scan`'s
    // `hkeys` read it back. `db_offset` gives each of this module's 3 tests (and, by a
    // pre-arranged split with pipeline.rs's 2 tests, that module's own tests) a disjoint slice of
    // Redis's 16 logical databases, so no two tests ever share a `LRR_FILEMAP` key at all.
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

    /// End-to-end demonstration of User Story 6: a historically false-merged pair (both files
    /// share a legacy-colliding leading byte range) is fully split apart by
    /// `rekey_all` (T074, re-keys the one tracked record) followed by `full_scan` (T074/T075,
    /// discovers the previously-invisible sibling file) — matching data-model.md's
    /// "Rebuild/Reindex operation" exactly, including the "revealed as new, not recovered"
    /// Clarifications rule for the previously-untracked file.
    #[tokio::test]
    async fn rebuild_then_scan_splits_a_historically_merged_pair() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(0) else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool.clone());
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        let mut shared_prefix = vec![b'z'; 512_000];
        let path_a = dir.path().join("a.zip");
        std::fs::write(&path_a, &shared_prefix).unwrap();
        shared_prefix.extend_from_slice(b"tail that makes b different from a");
        let path_b = dir.path().join("b.zip");
        std::fs::write(&path_b, &shared_prefix).unwrap();

        let legacy_id = lanrurugi_storage::id::legacy_id(&path_a).unwrap();
        assert_eq!(
            legacy_id,
            lanrurugi_storage::id::legacy_id(&path_b).unwrap()
        );

        // Simulate the historical merge exactly as Shinobu.pm produces it: one tracked record,
        // keyed by the shared legacy id, whose `file` field points at B (scanned "last") while
        // metadata (tags) reflects whatever was set while the record still looked like A/B mixed.
        archives
            .save(&Archive {
                id: legacy_id.clone(),
                name: "b".into(),
                title: "Legacy Title".into(),
                file: path_b.to_string_lossy().to_string(),
                tags: "artist:legacy".into(),
                summary: String::new(),
                arcsize: 1,
                pagecount: 1,
                isnew: false,
                lastreadpage: 7,
                lastreadtime: 500,
                thumbhash: None,
                toc: vec![],
                stamp_ids: vec![],
            })
            .await
            .unwrap();

        let categories =
            lanrurugi_storage::repository::CategoryRepository::new(archive_pool.clone());
        let groupings =
            lanrurugi_storage::repository::GroupingRepository::new(archive_pool.clone());
        let jobs = lanrurugi_core::jobs::JobRegistry::new();
        let job_id = jobs.create("rebuild").await;

        let rekey_summary = lanrurugi_storage::rebuild::rekey_all(
            &archives,
            &categories,
            &groupings,
            &jobs,
            &job_id,
        )
        .await
        .unwrap();
        assert_eq!(rekey_summary.rekeyed.len(), 1);
        let (_, new_id_for_b) = &rekey_summary.rekeyed[0];

        // B's record kept its original metadata under its new, correct identity.
        let rekeyed = archives.get(new_id_for_b).await.unwrap().unwrap();
        assert_eq!(rekeyed.tags, "artist:legacy");
        assert_eq!(rekeyed.lastreadpage, 7);

        // A is still completely untracked at this point — only a fresh scan discovers it.
        let expected_id_for_a = lanrurugi_storage::id::size_aware_id(&path_a).unwrap();
        assert!(archives.get(&expected_id_for_a).await.unwrap().is_none());

        let scan_job_id = jobs.create("scan").await;
        let scan_summary = full_scan(
            dir.path(),
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &jobs,
            &scan_job_id,
            None,
        )
        .await;
        assert_eq!(
            scan_summary.catalogued, 1,
            "only A should be newly catalogued"
        );

        // A is "revealed as new" — no pre-existing metadata (Clarifications Q2), not recovered
        // with B's tags. It does get a fresh `date_added:` tag (`usedateadded` defaults to on,
        // same as legacy) since it's genuinely new — that's the one tag a brand-new archive is
        // expected to carry, not evidence of an accidental merge with B's own real metadata.
        let a_record = archives.get(&expected_id_for_a).await.unwrap().unwrap();
        assert!(!a_record.tags.contains("artist:legacy"));
        assert!(a_record.isnew);

        // B's originally-tracked metadata is untouched by the scan.
        let b_record_after_scan = archives.get(new_id_for_b).await.unwrap().unwrap();
        assert_eq!(b_record_after_scan.tags, "artist:legacy");

        archives.delete(new_id_for_b).await.unwrap();
        archives.delete(&expected_id_for_a).await.unwrap();
        let mut config_conn = config_pool.get().await.unwrap();
        use deadpool_redis::redis::AsyncCommands;
        let _: () = config_conn
            .hdel(
                "LRR_FILEMAP",
                vec![
                    path_a.to_string_lossy().to_string(),
                    path_b.to_string_lossy().to_string(),
                ],
            )
            .await
            .unwrap();
    }

    /// Verified against `~/LANraragi/lib/Shinobu.pm::update_filemap`'s own cheap path-only diff:
    /// a path already present in `LRR_FILEMAP` must be skipped *without* `size_aware_id` (a real
    /// 512000-byte file read) ever being called on it — proven here by putting a filemap entry
    /// under a deliberately-wrong placeholder id that doesn't match the file's real content hash
    /// at all; if the scan were still hashing this path, either the placeholder id would get
    /// silently "rekeyed" to the real one, or (since nothing is tracked under the placeholder id)
    /// the file would come back as freshly `Catalogued` — neither may happen once this path is
    /// already a filemap-known path.
    #[tokio::test]
    async fn already_tracked_paths_are_skipped_without_rehashing() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(3) else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool.clone());
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        let path = dir.path().join("known.zip");
        std::fs::write(&path, b"irrelevant content, never read by this test").unwrap();
        let path_str = path.to_string_lossy().to_string();

        let placeholder_id = "0000000000000000000000000000000000000000";
        let mut config_conn = config_pool.get().await.unwrap();
        use deadpool_redis::redis::AsyncCommands;
        let _: () = config_conn
            .hset("LRR_FILEMAP", &path_str, placeholder_id)
            .await
            .unwrap();

        let jobs = lanrurugi_core::jobs::JobRegistry::new();
        let job_id = jobs.create("scan").await;
        let scan_summary = full_scan(
            dir.path(),
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &jobs,
            &job_id,
            None,
        )
        .await;

        assert_eq!(scan_summary.scanned, 1);
        assert_eq!(
            scan_summary.already_known, 1,
            "the filemap-known path must be counted as skipped, not processed"
        );
        assert_eq!(
            scan_summary.catalogued, 0,
            "it must not be freshly catalogued"
        );
        assert_eq!(
            scan_summary.unchanged, 0,
            "it must not even reach ingest_file"
        );

        // The placeholder id is untouched — proof `size_aware_id`/`ingest_file` never ran on this
        // path (a real hash would never collide with an all-zero placeholder).
        assert!(archives.get(placeholder_id).await.unwrap().is_none());

        let real_id = lanrurugi_storage::id::size_aware_id(&path).unwrap();
        assert!(
            archives.get(&real_id).await.unwrap().is_none(),
            "the real content hash must never have been computed or catalogued either"
        );

        let _: () = config_conn.hdel("LRR_FILEMAP", &path_str).await.unwrap();
    }

    /// The other half of `update_filemap` parity: a filemap entry whose path no longer exists on
    /// disk is pruned, matching legacy's own `@deletedfiles` cleanup.
    #[tokio::test]
    async fn stale_filemap_entries_for_deleted_files_are_pruned() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(6) else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool.clone());
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // Never actually created on disk — simulates a file that was deleted since the last scan.
        let ghost_path = dir.path().join("deleted.zip").to_string_lossy().to_string();
        let mut config_conn = config_pool.get().await.unwrap();
        use deadpool_redis::redis::AsyncCommands;
        let _: () = config_conn
            .hset("LRR_FILEMAP", &ghost_path, "deadbeef")
            .await
            .unwrap();

        let jobs = lanrurugi_core::jobs::JobRegistry::new();
        let job_id = jobs.create("scan").await;
        full_scan(
            dir.path(),
            &archives,
            &config_pool,
            &search_pool,
            thumb_dir.path(),
            &jobs,
            &job_id,
            None,
        )
        .await;

        let still_present: bool = config_conn
            .hexists("LRR_FILEMAP", &ghost_path)
            .await
            .unwrap();
        assert!(
            !still_present,
            "a filemap entry for a deleted file must be pruned"
        );
    }
}
