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
use std::time::{SystemTime, UNIX_EPOCH};

use deadpool_redis::redis;
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_search::Equivalence;
use lanrurugi_storage::redis::ScannerPools;
use lanrurugi_storage::repository::ArchiveRepository;
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::events::IngestEvent;

use crate::pipeline::{ingest_file, IngestOutcome, FILEMAP_KEY};
use crate::watcher::is_watched_archive_path;

/// How long after an interactive request the scan keeps yielding.
const IDLE_GRACE_MS: u64 = 3_000;
/// Trickle that keeps a busy instance from stalling the scan forever: this many archives per minute
/// get through even while somebody is reading or the machine is under pressure. A fixed floor rate
/// rather than a timeout — a timeout makes progress bursty (nothing for minutes, then one) and, with
/// N concurrent tasks, still only N files per timeout.
const IDLE_FLOOR_PER_MINUTE: f64 = 2.0;

/// Token bucket for [`IDLE_FLOOR_PER_MINUTE`], shared by every task in one scan.
struct FloorRate {
    tokens: f64,
    last: std::time::Instant,
}

impl FloorRate {
    fn new() -> Self {
        Self {
            tokens: 1.0,
            last: std::time::Instant::now(),
        }
    }

    /// Refill from elapsed time, then take one token if there is one.
    fn try_take(&mut self) -> bool {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = self.tokens.max(0.0) + elapsed * IDLE_FLOOR_PER_MINUTE / 60.0;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    fn near_cap(&mut self) {
        if self.tokens > 2.0 {
            self.tokens = 2.0;
        }
    }
}

/// Whether the `idlescan` setting is on. Read per file: one `HGET` next to an ingest that already
/// does dozens of Redis round-trips, and it means a settings change takes effect without a restart.
async fn idle_scan_enabled(config_pool: &Pool) -> bool {
    let Ok(mut conn) = config_pool.get().await else {
        return true;
    };
    conn.hget::<_, _, Option<String>>(lanrurugi_storage::keys::CONFIG_KEY, "idlescan")
        .await
        .ok()
        .flatten()
        .map(|value| value != "false" && value != "0")
        .unwrap_or(true)
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
    /// Archives moved under `_byname/` by the relocation pass at the end of this scan (020).
    pub relocated: usize,
    /// Relocation candidates left in place for any reason other than failure: a destination that
    /// already held a different file, a file that had not settled yet, and a record pointing
    /// outside the library. Kept as the sum for compatibility; the two fields below split it so a
    /// consumer can tell a conflict that needs a human from conditions that clear themselves.
    pub relocation_skipped: usize,
    /// Files too recently written to move safely; the next pass takes them.
    pub relocation_settling: usize,
    /// Records whose path is outside the library (an external mount); permanent by nature.
    pub relocation_outside: usize,
    pub relocation_repaired: usize,
    pub relocation_failed: usize,
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

/// `new_archive_tx`, when given, receives one [`IngestEvent`] per ingest attempt — same semantics
/// as `pipeline::run`'s own parameter of the same name: `Catalogued` for a genuinely brand-new
/// archive (never `Rekeyed`/`Unchanged`/`Rejected`; see that function's own docs for why), and
/// `Failed` for a real error/timeout (or a panicked task, where the path is unknown).
pub async fn full_scan(
    library_path: &Path,
    pools: &ScannerPools,
    equivalence: Arc<Equivalence>,
    thumb_dir: &Path,
    jobs: &JobRegistry,
    job_id: &str,
    new_archive_tx: Option<tokio::sync::mpsc::UnboundedSender<IngestEvent>>,
) -> ScanSummary {
    // Every write this scan makes goes through the scanner's own pools, so it queues behind other
    // scan writes rather than behind (or in front of) the API's requests — issue #206.
    let archives = pools.archives();
    let config_pool = &pools.config;
    let search_pool = &pools.search;
    let mut paths = Vec::new();
    walk(library_path, &mut paths);
    // Newest first. The library view is sorted by date added, so a first scan that starts at the
    // oldest file leaves the user staring at an empty list until it reaches the end; starting with
    // the most recently added archives puts what they just downloaded on screen first, and the old
    // backlog fills in behind it. `Option<SystemTime>` orders `None` lowest, so unreadable mtimes
    // end up last rather than first.
    paths.sort_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .map(std::cmp::Reverse)
    });
    let mut summary = ScanSummary {
        scanned: paths.len(),
        ..Default::default()
    };
    // Issue #206: a first scan of a 1.1 TB library takes hours, and the old code said nothing at
    // all until it finished — impossible to tell "still working" from "stuck". This line lands
    // before the first file is touched (the same moment the `startup_scan` job goes active), and
    // the periodic one below gives a rate and an ETA.
    tracing::info!(
        found = paths.len(),
        library = %library_path.display(),
        "library scan: found files on disk, cataloguing the ones not yet in the library"
    );

    // The cheap path-only pre-filter (see this module's own docs) — a single `HKEYS` plus
    // in-memory set operations, no file I/O. A Redis error here just disables the optimization
    // for this run (every path falls through to `ingest_file` as before, which is still correct,
    // just not fast) rather than failing the whole scan over it.
    let on_disk: HashSet<String> = paths
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    if let Ok(mut conn) = config_pool.get().await {
        let mut known_paths: HashSet<String> = conn.hkeys(FILEMAP_KEY).await.unwrap_or_default();

        // Prune filemap entries for paths no longer on disk — matches legacy's own
        // `update_filemap` (`@deletedfiles = grep { !$fshash{$_} } @filemapfiles`). Only touches
        // the filemap's path→id bookkeeping, not the Archive record itself (a missing-file
        // archive is a separate, existing "Clean Database" concern, not this scan's job).
        let stale: Vec<&String> = known_paths.difference(&on_disk).collect();
        if !stale.is_empty() {
            let _: Result<(), _> = conn.hdel(FILEMAP_KEY, stale).await;
        }

        // A filemap entry whose archive no longer exists is *not* "already known": it is the
        // debris of an ingest that wrote the mapping and then failed (or was killed) before the
        // record landed, and honouring it would skip that file on every future scan forever. One
        // library here had 89 such files — invisible in the UI, and the reason its archive count
        // sat 89 short of the file count. Verify cheaply with a pipelined `EXISTS`; on any Redis
        // error the set is left alone and every path simply falls through to `ingest_file`.
        if !known_paths.is_empty() {
            let mapped: Vec<(String, String)> = conn.hgetall(FILEMAP_KEY).await.unwrap_or_default();
            let ids: Vec<&String> = mapped.iter().map(|(_, id)| id).collect();
            // Archive records live in the *archive* database, not the config one this connection
            // points at — asking the config DB for these ids returns false for every single one,
            // which would wipe the whole filemap and re-hash the entire library on every scan.
            // If the archive pool is unavailable, skip the verification entirely and trust the
            // filemap — never destroy it on a guess.
            let mut pipe = redis::pipe();
            for id in &ids {
                pipe.exists(id.as_str());
            }
            let alive: Option<Vec<bool>> = match pools.archive.get().await {
                Ok(mut conn) => pipe.query_async::<Vec<bool>>(&mut conn).await.ok(),
                Err(_) => None,
            };
            if let Some(alive) = alive {
                let dead_paths: Vec<String> = mapped
                    .iter()
                    .zip(alive)
                    .filter(|(_, exists)| !exists)
                    .map(|((path, _), _)| path.clone())
                    .collect();
                if !dead_paths.is_empty() {
                    tracing::warn!(
                        count = dead_paths.len(),
                        "library scan: filemap entries whose archive is gone; re-cataloguing them"
                    );
                    let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &dead_paths).await;
                    for path in dead_paths {
                        known_paths.remove(&path);
                    }
                }
            }
        }

        let before = paths.len();
        paths.retain(|p| !known_paths.contains(&p.to_string_lossy().to_string()));
        summary.already_known = before - paths.len();
    }

    let total_files = paths.len();
    let total = total_files.max(1);
    // ~5% steps, but never so fine that a small library logs a line per file.
    let progress_every = (total_files / 20).max(25);
    let started = std::time::Instant::now();
    // Bounding concurrency rather than spawning one task per file keeps this resource-conscious at
    // the SC-008 target scale (~100k files) while still overlapping files' I/O and hashing
    // (constitution Principle III, FR-022). See `lanrurugi_core::concurrency::scan_concurrency`.
    let semaphore = Arc::new(Semaphore::new(
        lanrurugi_core::concurrency::scan_concurrency(),
    ));
    let floor = Arc::new(tokio::sync::Mutex::new(FloorRate::new()));
    let completed = Arc::new(AtomicUsize::new(0));
    let mut tasks = tokio::task::JoinSet::new();

    for path in paths {
        let semaphore = semaphore.clone();
        let floor = floor.clone();
        let archives = archives.clone();
        let config_pool = config_pool.clone();
        let search_pool = search_pool.clone();
        let equivalence = equivalence.clone();
        let thumb_dir = thumb_dir.to_path_buf();
        let jobs = jobs.clone();
        let job_id = job_id.to_string();
        let completed = completed.clone();

        tasks.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .expect("semaphore is never closed");
            // Yield while somebody is using the instance, or while the kernel reports the machine
            // is stalling on I/O or CPU (which also covers scrubs, backups and other containers —
            // the request beacon cannot see those). The floor rate guarantees progress either way.
            while idle_scan_enabled(&config_pool).await {
                let busy = lanrurugi_core::activity::request_within(
                    lanrurugi_core::activity::now_ms(),
                    IDLE_GRACE_MS,
                ) || lanrurugi_core::activity::under_pressure();
                if !busy {
                    break;
                }
                let allowed = {
                    let mut bucket = floor.lock().await;
                    bucket.near_cap();
                    bucket.try_take()
                };
                if allowed {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            let result = ingest_file(
                &archives,
                &config_pool,
                &search_pool,
                &equivalence,
                &thumb_dir,
                &path,
            )
            .await;
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            jobs.set_progress(&job_id, done as f32 / total as f32).await;
            if done.is_multiple_of(progress_every) || done == total_files {
                let elapsed = started.elapsed().as_secs_f64();
                // Documents per second, not bytes: archive sizes differ by two orders of
                // magnitude, so a byte-based ETA would swing wildly.
                let rate = done as f64 / elapsed.max(0.001);
                let eta = (total_files.saturating_sub(done)) as f64 / rate;
                tracing::info!(
                    done,
                    total = total_files,
                    elapsed_s = elapsed as u64,
                    eta_s = eta as u64,
                    "library scan progress"
                );
            }
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
                if let Some(tx) = &new_archive_tx {
                    let _ = tx.send(IngestEvent::Failed {
                        path: None,
                        reason: format!("ingest task panicked or was cancelled: {join_err}"),
                    });
                }
                continue;
            }
        };
        match result {
            Ok(IngestOutcome::Catalogued { id }) => {
                summary.catalogued += 1;
                if let Some(tx) = &new_archive_tx {
                    let _ = tx.send(IngestEvent::Catalogued { id: id.to_string() });
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
                if let Some(tx) = &new_archive_tx {
                    let _ = tx.send(IngestEvent::Failed {
                        path: Some(path.to_string_lossy().to_string()),
                        reason: e.to_string(),
                    });
                }
            }
        }
    }

    // Everything catalogued by filename — pre-020 records and files dropped into the mount since the
    // last scan — belongs under `_byname/`. One pass at the end covers both; a failure is counted,
    // never fatal: a file left in place is still readable, it just sits outside the convention.
    match crate::relocate::relocate_library(&archives, config_pool, library_path).await {
        Ok(report) => {
            summary.relocated = report.moved as usize;
            summary.relocation_skipped =
                (report.skipped_occupied + report.settling + report.outside) as usize;
            summary.relocation_settling = report.settling as usize;
            summary.relocation_outside = report.outside as usize;
            summary.relocation_repaired = report.repaired as usize;
            summary.relocation_failed = report.failed as usize;
            if !report.is_empty() {
                tracing::info!(
                    moved = report.moved,
                    skipped_occupied = report.skipped_occupied,
                    settling = report.settling,
                    outside = report.outside,
                    failed = report.failed,
                    "_byname relocation pass"
                );
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "cannot list archives for the _byname relocation pass");
        }
    }

    summary
}

/// Why a single archive's `pagecount`/`arcsize` heal attempt in [`heal_pagecounts`] came out the
/// way it did — kept per-archive (not just a top-level count) so `/jobs`'s `result` JSON actually
/// names which archives were touched, per issue #2's explicit "能看到修了哪个档案" requirement.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum HealOutcome {
    Healed {
        old_pagecount: u32,
        new_pagecount: u32,
        new_arcsize: u64,
    },
    /// `archive_format::list_pages` itself failed or returned zero entries — this means the
    /// *archive container* is unreadable/corrupt (or an unsupported format), not that some
    /// individual page's image bytes are damaged. `list_pages` only inspects entry names/types
    /// (`archive_format::list_pages`'s own docs — it `skip_data()`s every entry, never decoding
    /// image content), so a single corrupt *page* inside an otherwise-intact archive is invisible
    /// to this check and never lands here — that's `Archive::corrupted_pages`'s job instead
    /// (detected when the reader/thumbnail path actually decodes a page's bytes).
    Failed { reason: String },
    /// Already has `heal_failed_at` set from a previous run — not retried (see this module's own
    /// docs on why), so it doesn't even reach the point of calling `list_pages` again.
    SkippedKnownFailed,
}

#[derive(Debug, Clone, Serialize)]
pub struct HealDetail {
    pub archive_id: String,
    pub filename: String,
    #[serde(flatten)]
    pub outcome: HealOutcome,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct HealSummary {
    /// Archives with `pagecount == 0` and an on-disk file — the full candidate set before the
    /// `heal_failed_at` skip-filter, so `checked == healed + failed + skipped_known_failed`.
    pub checked: usize,
    pub healed: usize,
    pub failed: usize,
    pub skipped_known_failed: usize,
    pub details: Vec<HealDetail>,
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Detects and repairs archives whose `pagecount`/`arcsize` were corrupted by the double-ingest
/// race this issue fixed (`lanrurugi_core::filename_lock`'s own docs have the full incident) —
/// and, going forward, any *other* cause that leaves a tracked archive with `pagecount: 0` despite
/// a real, readable file on disk. Re-runs `archive_format::list_pages` + a fresh `stat` against
/// the archive's current on-disk file and writes the corrected values back.
///
/// Deliberately **not retried** once an attempt fails (`Archive::heal_failed_at` is set and the
/// archive is skipped by every subsequent call to this function) — a permanently unreadable file
/// (deleted, replaced with garbage, a format `archive_format` doesn't support) would otherwise be
/// re-attempted on every `startup_scan`/`rebuild-index` run forever, each one just re-confirming
/// the same failure. Only a fresh catalogue of that exact archive ID (e.g. deleting it and
/// re-downloading, which produces a brand-new `Archive` record with `heal_failed_at: None`) clears
/// the marker and makes it eligible again.
pub async fn heal_pagecounts(archives: &ArchiveRepository) -> HealSummary {
    let all = match archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "heal_pagecounts: failed to list archives, skipping this run");
            return HealSummary::default();
        }
    };

    let mut summary = HealSummary::default();
    for mut archive in all {
        if archive.pagecount != 0 || archive.file.is_empty() || !Path::new(&archive.file).exists() {
            continue;
        }
        summary.checked += 1;

        let filename = Path::new(&archive.file)
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| archive.file.clone());

        // E-books are exempt from "never retry": before e-book support every one failed here and
        // was marked, and a book that still cannot be opened fails fast and cheaply.
        let is_book = crate::archive_format::is_book(Path::new(&archive.file));
        if archive.heal_failed_at.is_some() && !is_book {
            summary.skipped_known_failed += 1;
            summary.details.push(HealDetail {
                archive_id: archive.id.to_string(),
                filename,
                outcome: HealOutcome::SkippedKnownFailed,
            });
            continue;
        }

        match crate::archive_format::list_pages(Path::new(&archive.file)) {
            Ok(pages) if !pages.is_empty() => {
                let new_arcsize = std::fs::metadata(&archive.file)
                    .map(|m| m.len())
                    .unwrap_or(archive.arcsize);
                let old_pagecount = archive.pagecount;
                let new_pagecount = pages.len() as u32;
                archive.pagecount = new_pagecount;
                archive.arcsize = new_arcsize;
                archive.heal_failed_at = None;
                if is_book && archive.toc.is_empty() {
                    archive.toc = crate::pipeline::book_outline_toc(Path::new(&archive.file));
                }
                if let Err(e) = archives.save(&archive).await {
                    tracing::warn!(archive_id = %archive.id, error = %e, "heal_pagecounts: failed to save healed archive");
                    summary.failed += 1;
                    summary.details.push(HealDetail {
                        archive_id: archive.id.to_string(),
                        filename,
                        outcome: HealOutcome::Failed {
                            reason: e.to_string(),
                        },
                    });
                    continue;
                }
                tracing::info!(
                    archive_id = %archive.id,
                    %filename,
                    old_pagecount,
                    new_pagecount,
                    new_arcsize,
                    "heal_pagecounts: healed"
                );
                summary.healed += 1;
                summary.details.push(HealDetail {
                    archive_id: archive.id.to_string(),
                    filename,
                    outcome: HealOutcome::Healed {
                        old_pagecount,
                        new_pagecount,
                        new_arcsize,
                    },
                });
            }
            list_result => {
                // Either an `Err` (container itself unreadable/corrupt/unsupported) or `Ok(vec![])`
                // (readable but genuinely has no image entries) — both mean "not fixable by this
                // pass" and get the same `heal_failed_at` treatment so neither is retried forever.
                let reason = match list_result {
                    Err(e) => e.to_string(),
                    Ok(_) => "archive contains no recognized image entries".to_string(),
                };
                // Books skip the "known failed" shortcut above (a book that failed last month may
                // open after a worker upgrade), so a permanently broken book retries on *every*
                // pass. Log only the first failure at WARN and the repeats at debug: three test
                // fixtures left in a library turned into 173 identical warning events in a few
                // hours, which drowns the issues list (2026-11-01).
                let already_failing = archive.heal_failed_at.is_some();
                archive.heal_failed_at = Some(now_unix_secs());
                if let Err(e) = archives.save(&archive).await {
                    tracing::warn!(archive_id = %archive.id, error = %e, "heal_pagecounts: failed to persist heal_failed_at marker");
                }
                if already_failing {
                    tracing::debug!(archive_id = %archive.id, %filename, %reason, "heal_pagecounts: still failing");
                } else {
                    tracing::warn!(archive_id = %archive.id, %filename, %reason, "heal_pagecounts: failed, will not retry");
                }
                summary.failed += 1;
                summary.details.push(HealDetail {
                    archive_id: archive.id.to_string(),
                    filename,
                    outcome: HealOutcome::Failed { reason },
                });
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

    /// A scan's pools, from the scratch pools these tests already hold — the scan only ever takes
    /// `ScannerPools`, so a test cannot accidentally hand it a repository on the wrong pool.
    fn scanner_pools(
        archive: &deadpool_redis::Pool,
        config: &deadpool_redis::Pool,
        search: &deadpool_redis::Pool,
    ) -> ScannerPools {
        ScannerPools {
            archive: archive.clone(),
            config: config.clone(),
            search: search.clone(),
        }
    }

    /// End-to-end demonstration of User Story 6: a historically false-merged pair (both files
    /// share a legacy-colliding leading byte range) is fully split apart by
    /// `rekey_all` (T074, re-keys the one tracked record) followed by `full_scan` (T074/T075,
    /// discovers the previously-invisible sibling file) — matching data-model.md's
    /// "Rebuild/Reindex operation" exactly, including the "revealed as new, not recovered"
    /// Clarifications rule for the previously-untracked file.
    #[tokio::test]
    async fn rebuild_then_scan_splits_a_historically_merged_pair() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(0).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool.clone());
        let pools = scanner_pools(&archive_pool, &config_pool, &search_pool);
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
                id: lanrurugi_core::ids::ArchiveId(legacy_id.clone()),
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
                heal_failed_at: None,
                corrupted_pages: vec![],
                has_patch: false,
                blake3: None,
            })
            .await
            .unwrap();

        let categories =
            lanrurugi_storage::repository::CategoryRepository::new(archive_pool.clone());
        let groupings =
            lanrurugi_storage::repository::GroupingRepository::new(archive_pool.clone());
        let stamps = lanrurugi_storage::repository::StampRepository::new(archive_pool.clone());
        let jobs = lanrurugi_core::jobs::JobRegistry::new();
        let job_id = jobs.create("rebuild").await;

        let rekey_summary = lanrurugi_storage::rebuild::rekey_all(
            &archives,
            &categories,
            &groupings,
            &stamps,
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
        let expected_id_for_a =
            lanrurugi_core::ids::ArchiveId(lanrurugi_storage::id::size_aware_id(&path_a).unwrap());
        assert!(archives.get(&expected_id_for_a).await.unwrap().is_none());

        let scan_job_id = jobs.create("scan").await;
        let scan_summary = full_scan(
            dir.path(),
            &pools,
            std::sync::Arc::new(Equivalence::default()),
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
        let Some((archive_pool, config_pool, search_pool)) = test_pools(3).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool.clone());
        let pools = scanner_pools(&archive_pool, &config_pool, &search_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        let path = dir.path().join("known.zip");
        std::fs::write(&path, b"irrelevant content, never read by this test").unwrap();
        // The relocation pass leaves files alone until they have settled, so a file written a
        // moment ago would stay at the root and this test would be asserting the wrong path.
        let aged = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(aged)
            .unwrap();
        let path_str = path.to_string_lossy().to_string();

        let placeholder_id = "0000000000000000000000000000000000000000";
        // The mapping only counts as "already known" while its archive record exists: the scan
        // prunes entries whose archive is gone (the debris of an ingest that died between writing
        // the mapping and writing the record), so this test needs a real record behind it.
        archives
            .save(&Archive {
                id: lanrurugi_core::ids::ArchiveId(placeholder_id.to_string()),
                name: "known".into(),
                title: "Known".into(),
                file: path_str.clone(),
                tags: String::new(),
                summary: String::new(),
                arcsize: 1,
                pagecount: 1,
                isnew: false,
                lastreadpage: 0,
                lastreadtime: 0,
                thumbhash: None,
                toc: vec![],
                stamp_ids: vec![],
                heal_failed_at: None,
                corrupted_pages: vec![],
                has_patch: false,
                blake3: None,
            })
            .await
            .unwrap();
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
            &pools,
            std::sync::Arc::new(Equivalence::default()),
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
        // The record still exists but nothing touched it: `ingest_file` would have rewritten
        // `file`/`title` for a real re-ingest.
        let untouched = archives
            .get(&lanrurugi_core::ids::ArchiveId(placeholder_id.to_string()))
            .await
            .unwrap()
            .expect("the placeholder record is kept");
        assert_eq!(untouched.title, "Known");

        // The relocation pass moves every by-name archive under `_byname/`, known or not, so the
        // file the test placed at the root is gone from there and lives one directory deeper.
        assert_eq!(
            untouched.file,
            lanrurugi_storage::paths::byname_dir(dir.path())
                .join(path.file_name().unwrap())
                .to_string_lossy()
        );
        let moved_path = std::path::PathBuf::from(&untouched.file);
        let real_id = lanrurugi_core::ids::ArchiveId(
            lanrurugi_storage::id::size_aware_id(&moved_path).unwrap(),
        );
        // Hermetic: an earlier run of this same test (before the filemap-pruning behaviour existed)
        // could have left a record under the size-aware id, and `size_aware_id` depends only on the
        // file's own size and mtime, not on the temp path.
        let _ = archives.delete(&real_id).await;
        assert!(
            archives.get(&real_id).await.unwrap().is_none(),
            "the real content hash must never have been computed or catalogued either"
        );

        let _: () = config_conn
            .hdel("LRR_FILEMAP", untouched.file)
            .await
            .unwrap();
    }

    /// The other half of `update_filemap` parity: a filemap entry whose path no longer exists on
    /// disk is pruned, matching legacy's own `@deletedfiles` cleanup.
    #[tokio::test]
    async fn stale_filemap_entries_for_deleted_files_are_pruned() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(6).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let pools = scanner_pools(&archive_pool, &config_pool, &search_pool);
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
            &pools,
            std::sync::Arc::new(Equivalence::default()),
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

    /// 020 T023/T019: a file that arrives behind the scanner's back (SMB, rsync, `cp`) is
    /// catalogued under its own name first and then relocated into `_byname/` by the same pass —
    /// once, not twice, and with its title still derived from the filename.
    #[tokio::test]
    async fn a_dropped_file_is_catalogued_then_relocated_into_byname() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(36).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool.clone());
        let pools = scanner_pools(&archive_pool, &config_pool, &search_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        let path = dir.path().join("Series/Dropped Book.zip");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"dropped over smb").unwrap();
        let aged = std::time::SystemTime::now() - std::time::Duration::from_secs(600);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(aged)
            .unwrap();

        let jobs = lanrurugi_core::jobs::JobRegistry::new();
        let job_id = jobs.create("scan").await;
        let first = full_scan(
            dir.path(),
            &pools,
            std::sync::Arc::new(Equivalence::default()),
            thumb_dir.path(),
            &jobs,
            &job_id,
            None,
        )
        .await;

        assert_eq!(first.catalogued, 1, "the dropped file is catalogued once");
        assert_eq!(first.relocated, 1, "and relocated in the same pass");

        let records = archives.list_all().await.unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        let expected = dir
            .path()
            .join("_byname/Series/Dropped Book.zip")
            .to_string_lossy()
            .to_string();
        assert_eq!(record.file, expected);
        assert_eq!(record.name, "Dropped Book");
        assert_eq!(record.title, "Dropped Book");
        assert!(!path.exists(), "the original path no longer holds the file");

        // The next scan sees it where it now lives and leaves it alone.
        let job_id = jobs.create("scan").await;
        let second = full_scan(
            dir.path(),
            &pools,
            std::sync::Arc::new(Equivalence::default()),
            thumb_dir.path(),
            &jobs,
            &job_id,
            None,
        )
        .await;
        assert_eq!(second.catalogued, 0);
        assert_eq!(second.relocated, 0);
        assert_eq!(archives.list_all().await.unwrap().len(), 1);

        archives.delete(&record.id).await.unwrap();
        let mut conn = config_pool.get().await.unwrap();
        use deadpool_redis::redis::AsyncCommands;
        let _: () = conn.hdel("LRR_FILEMAP", &expected).await.unwrap();
    }

    /// 020 T022: after the records are gone, a scan rebuilds titles for both layouts from what is
    /// on disk — the sidecar for a hash-named archive, the filename for one under `_byname/`.
    #[tokio::test]
    async fn a_scan_recovers_titles_for_both_layouts_after_the_records_are_lost() {
        let Some((archive_pool, config_pool, search_pool)) = test_pools(39).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool.clone());
        let pools = scanner_pools(&archive_pool, &config_pool, &search_pool);
        let dir = tempfile::tempdir().unwrap();
        let thumb_dir = tempfile::tempdir().unwrap();

        // This database slice is this test's own (offset 39), but a previous run that failed
        // before its cleanup would leave records behind and make the next one see `unchanged`
        // instead of a fresh catalogue.
        for stale in archives.list_all().await.unwrap() {
            archives.delete(&stale.id).await.unwrap();
        }
        let mut scratch = config_pool.get().await.unwrap();
        use deadpool_redis::redis::AsyncCommands;
        let _: () = scratch.del("LRR_FILEMAP").await.unwrap();

        // Hash layout: the disk name is opaque, so the title can only come from the sidecar. The
        // id is the content id, so the bytes are written first and moved into their bucket after.
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let staging = dir.path().join("incoming");
        std::fs::write(&staging, format!("hash-layout bytes {nonce}")).unwrap();
        let id = lanrurugi_storage::id::size_aware_id(&staging).unwrap();
        let hashed = lanrurugi_storage::paths::hash_layout_path(dir.path(), &id, "zip");
        std::fs::create_dir_all(hashed.parent().unwrap()).unwrap();
        std::fs::rename(&staging, &hashed).unwrap();
        let sidecar = lanrurugi_storage::sidecar::Sidecar::for_archive(
            &id,
            &hashed,
            "[Group] The Book [DL版].zip",
            "The Book",
        )
        .unwrap();
        lanrurugi_storage::sidecar::write(dir.path(), &sidecar).unwrap();

        // By-name layout: the filename already says everything, and it gets no sidecar.
        let byname_dir = lanrurugi_storage::paths::byname_dir(dir.path()).join("Series");
        std::fs::create_dir_all(&byname_dir).unwrap();
        std::fs::write(
            byname_dir.join("Old Book.zip"),
            format!("by-name bytes {nonce}"),
        )
        .unwrap();

        let jobs = lanrurugi_core::jobs::JobRegistry::new();
        let job_id = jobs.create("scan").await;
        let summary = full_scan(
            dir.path(),
            &pools,
            std::sync::Arc::new(Equivalence::default()),
            thumb_dir.path(),
            &jobs,
            &job_id,
            None,
        )
        .await;
        assert_eq!(summary.catalogued, 2, "{summary:?}");
        assert_eq!(summary.relocated, 0, "both files are already in a layout");

        let records = archives.list_all().await.unwrap();
        assert_eq!(records.len(), 2);
        let hashed_record = records.iter().find(|a| a.id.as_str() == id).unwrap();
        assert_eq!(hashed_record.title, "The Book");
        assert_eq!(hashed_record.name, "[Group] The Book [DL版]");
        let byname_record = records
            .iter()
            .find(|a| a.file.contains("_byname"))
            .expect("the by-name file is catalogued where it lives");
        assert_eq!(byname_record.title, "Old Book");
        assert_eq!(byname_record.name, "Old Book");
        assert!(
            !lanrurugi_storage::paths::sidecar_path(dir.path(), byname_record.id.as_str()).exists(),
            "by-name archives get no sidecar"
        );
        // Cataloguing rewrites the sidecar with the record's title. The original filename has no
        // other home — a rebuild must reuse it rather than fall back to `<id>.zip`.
        let rewritten =
            lanrurugi_storage::sidecar::read(dir.path(), &id).expect("the sidecar is still there");
        assert_eq!(rewritten.original_filename, "[Group] The Book [DL版].zip");
        assert_eq!(rewritten.title, "The Book");

        for record in &records {
            archives.delete(&record.id).await.unwrap();
        }
        let mut conn = config_pool.get().await.unwrap();
        let _: () = conn
            .hdel("LRR_FILEMAP", hashed.to_string_lossy().to_string())
            .await
            .unwrap();
        let _: () = conn
            .hdel(
                "LRR_FILEMAP",
                byname_dir
                    .join("Old Book.zip")
                    .to_string_lossy()
                    .to_string(),
            )
            .await
            .unwrap();
    }

    fn make_zip_with_pages(dir: &std::path::Path, name: &str, n_pages: usize) -> PathBuf {
        let path = dir.join(name);
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, '_, ()> = zip::write::FileOptions::default();
        use std::io::Write;
        for i in 0..n_pages {
            writer.start_file(format!("page{i}.jpg"), options).unwrap();
            writer.write_all(b"fake image bytes").unwrap();
        }
        writer.finish().unwrap();
        path
    }

    fn archive_with_zero_pagecount(
        id: &str,
        file: &std::path::Path,
    ) -> lanrurugi_core::entities::Archive {
        lanrurugi_core::entities::Archive {
            id: lanrurugi_core::ids::ArchiveId(id.to_string()),
            name: "n".into(),
            title: "t".into(),
            file: file.to_string_lossy().to_string(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 123, // Deliberately wrong — this is what `heal_pagecounts` must correct.
            pagecount: 0,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: false,
            blake3: None,
        }
    }

    #[tokio::test]
    async fn heal_pagecounts_recomputes_pagecount_and_arcsize_for_a_readable_archive() {
        let Some((archive_pool, _, _)) = test_pools(30).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        let path = make_zip_with_pages(dir.path(), "book.zip", 5);
        let real_size = std::fs::metadata(&path).unwrap().len();

        let id = lanrurugi_core::ids::ArchiveId("1".repeat(40));
        archives
            .save(&archive_with_zero_pagecount(id.as_str(), &path))
            .await
            .unwrap();

        let summary = heal_pagecounts(&archives).await;

        assert_eq!(summary.checked, 1);
        assert_eq!(summary.healed, 1);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.skipped_known_failed, 0);
        assert_eq!(summary.details.len(), 1);
        assert_eq!(summary.details[0].archive_id, id.to_string());
        assert!(matches!(
            summary.details[0].outcome,
            HealOutcome::Healed {
                new_pagecount: 5,
                ..
            }
        ));

        let healed = archives.get(&id).await.unwrap().unwrap();
        assert_eq!(healed.pagecount, 5);
        assert_eq!(healed.arcsize, real_size);
        assert_eq!(healed.heal_failed_at, None);

        archives.delete(&id).await.unwrap();
    }

    #[tokio::test]
    async fn heal_pagecounts_marks_an_unreadable_archive_failed_and_never_retries_it() {
        let Some((archive_pool, _, _)) = test_pools(33).await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(archive_pool);
        let dir = tempfile::tempdir().unwrap();
        // A file with an archive-like extension but bytes that aren't a real archive at all —
        // `list_pages` must fail on this (the container itself is unreadable), matching the
        // "corrupt/unsupported archive" case, not the "one page's image is corrupt" case.
        let path = dir.path().join("broken.zip");
        std::fs::write(&path, b"this is not a real zip file").unwrap();

        let id = lanrurugi_core::ids::ArchiveId("2".repeat(40));
        archives
            .save(&archive_with_zero_pagecount(id.as_str(), &path))
            .await
            .unwrap();

        let first = heal_pagecounts(&archives).await;
        assert_eq!(first.checked, 1);
        assert_eq!(first.healed, 0);
        assert_eq!(first.failed, 1);
        assert_eq!(first.skipped_known_failed, 0);
        assert!(matches!(
            first.details[0].outcome,
            HealOutcome::Failed { .. }
        ));

        let after_first = archives.get(&id).await.unwrap().unwrap();
        assert!(
            after_first.heal_failed_at.is_some(),
            "a failed heal attempt must record heal_failed_at so it isn't retried forever"
        );
        assert_eq!(
            after_first.pagecount, 0,
            "a failed heal must not touch pagecount/arcsize"
        );

        // A second run must not attempt `list_pages` on this archive again — it's already marked
        // failed, so it's skipped outright (counted separately from a fresh failure).
        let second = heal_pagecounts(&archives).await;
        assert_eq!(second.checked, 1);
        assert_eq!(second.healed, 0);
        assert_eq!(second.failed, 0);
        assert_eq!(second.skipped_known_failed, 1);
        assert!(matches!(
            second.details[0].outcome,
            HealOutcome::SkippedKnownFailed
        ));

        archives.delete(&id).await.unwrap();
    }
}

#[cfg(test)]
mod recycle_scan_tests {
    use super::*;

    /// A full walk must not see recycled payloads: `walk` feeds `full_scan`, and the same filter
    /// backs the live watcher (specs/019 FR-005 / SC-003).
    #[test]
    fn walk_skips_the_recycle_bin_directory() {
        let root = tempfile::tempdir().unwrap();
        let normal = root.path().join("book.zip");
        std::fs::write(&normal, b"normal").unwrap();
        let bin = root
            .path()
            .join(lanrurugi_storage::recycle::RECYCLE_DIR_NAME)
            .join("a".repeat(40))
            .join("payload");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("book.zip"), b"recycled").unwrap();

        let mut found = Vec::new();
        walk(root.path(), &mut found);
        assert_eq!(found, vec![normal]);
    }
}
