//! Writes reader-recommendation embedding vectors and per-archive Top-N similar-archive lists to
//! `lanrurugi_storage::recommend_cache` at catalogue/title-change time, so `recommend.rs`'s
//! request path becomes a cache read instead of the O(n) ONNX-embed-everything-then-rank pass it
//! used to run inline (issue #70 — measured 3.13s/1k, 18.06s/5k, 56.61s/20k archives cold, and
//! didn't finish inside a 2-minute timeout at 100k).
//!
//! Two write paths:
//! - [`precompute_one`]: incremental, fired (not awaited) from the archive-ingest and title-change
//!   call sites. O(1) embed for the changed archive, O(n) cosine comparison against every already-
//!   cached vector to build its own Top-N, and a **one-way** backfill — archives whose similarity
//!   to the new/changed one clears the cache's own tail-score threshold get it re-ranked into their
//!   existing Top-N list. This is deliberately O(n), not O(n²): only the one changed archive's
//!   vector is compared against the rest of the library, not every pair recomputed.
//! - [`spawn_full_precompute_job`]: full O(n²) rebuild across every archive, run as a background
//!   [`JobRegistry`] job for the first-time backfill and whenever the precision tier changes (a
//!   tier change alters `top_n`, which an incremental one-way backfill can't retroactively widen
//!   for archives that were never revisited). Resumable — skips any archive whose cached Top-N
//!   already belongs to the current `rebuild_generation`.
//!
//! CPU is deliberately budgeted (`precompute_worker_budget`) rather than let the batch job consume
//! every core: this runs on the same process that's also serving live HTTP requests, and a rebuild
//! triggered by a tier change or a fresh install shouldn't make the rest of the app sluggish.
//! [`LoadThrottle`] additionally backs off in real time if the *actual* measured system load (not
//! just this job's own thread count) climbs, since the budget alone can't see load from other
//! processes on the host.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use lanrurugi_recommend::embedding::cosine_similarity;

use crate::AppState;

/// The title-normalization rule revision the persisted vectors were embedded with — see
/// [`lanrurugi_recommend::title::RULE_REVISION`]. `main.rs` compares this against the value stored
/// in the cache's meta hash at startup and, on a mismatch, purges the vectors and forces a full
/// rebuild: the per-vector reuse checks compare *raw* titles, which a normalization-rule change
/// never alters, so without this a rule change would be silently inert against an existing
/// library.
pub const NORMALIZE_RULE_REVISION: u32 = lanrurugi_recommend::title::RULE_REVISION;

/// User-selectable recommendation-cache precision (Settings page, `recommendprecision` field —
/// see `settings.rs`). Higher tiers keep a longer Top-N per archive, which costs more Redis
/// storage and more CPU time per rebuild, but gives the LLM rerank step (`recommend_llm.rs`) a
/// wider shortlist to pick "next volume" from when Tankoubon sibling-exclusion has thinned out the
/// low end of a short list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecommendPrecision {
    Low,
    Medium,
    High,
}

impl RecommendPrecision {
    pub fn from_setting(s: &str) -> Self {
        match s {
            "high" => Self::High,
            "low" => Self::Low,
            _ => Self::Medium,
        }
    }

    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }

    /// How many candidates to keep per archive's cached Top-N list. Scales with both CPU core
    /// count (more cores → a full rebuild of a wider list is still affordable) and library size
    /// (no point caching 2,000 candidates out of a 500-archive library) rather than a fixed
    /// constant per tier. **Every tier is floored at `recommend_llm::PREFILTER_COUNT`** — the
    /// read path (`recommend.rs`) promises at least `limit.max(PREFILTER_COUNT)` candidates to the
    /// LLM rerank, and a shorter cached list silently starves it: before this floor applied to all
    /// tiers, a 200-archive library on `medium` cached 24 ids per archive, so the cache-hit path
    /// handed the model a quarter of the documented shortlist and could not even see an archive's
    /// own next volume.
    pub fn top_n(self, cores: usize, library_size: usize) -> usize {
        let cores = cores.max(1);
        let scale = match self {
            Self::Low => 4,
            Self::Medium => 12,
            Self::High => 30,
        };
        let by_hardware = cores * scale;
        let by_library = library_size / 10;
        by_hardware
            .min(by_library)
            .max(scale * 2)
            .max(crate::recommend_llm::PREFILTER_COUNT)
    }
}

pub(crate) async fn read_precision(state: &AppState) -> RecommendPrecision {
    use deadpool_redis::redis::AsyncCommands;
    let raw: Option<String> = match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget(lanrurugi_storage::keys::CONFIG_KEY, "recommendprecision")
            .await
            .ok()
            .flatten(),
        Err(_) => None,
    };
    RecommendPrecision::from_setting(raw.as_deref().unwrap_or("medium"))
}

/// Thread budget for the CPU-bound parts of precompute work (cosine ranking over the whole
/// library, and the `Embedder::load` intra-op thread count set once at startup) — roughly 30% of
/// available cores, matching the user's own explicit hardware-proportional/dynamic-adjustment
/// requirement rather than a fixed worker count that would either starve a 4-core box or leave a
/// 32-core one mostly idle.
pub fn precompute_worker_budget() -> usize {
    (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        * 3
        / 10)
        .max(1)
}

/// Tracks system-wide CPU usage between batches of precompute work and returns how long to sleep
/// before the next batch — real-time backpressure on top of the fixed worker-count budget above,
/// since that budget alone has no visibility into load from *other* processes on the host (a
/// concurrent `mise run dev-rebuild`, another container, etc.). Not reused across job runs: a
/// fresh instance is cheap (`sysinfo::System::new_all()` is the expensive part, done once) and
/// avoids carrying stale readings from a previous, possibly very different, run.
pub struct LoadThrottle {
    sys: sysinfo::System,
}

impl LoadThrottle {
    pub fn new() -> Self {
        Self {
            sys: sysinfo::System::new_all(),
        }
    }

    /// Measures current global CPU usage and returns the delay to sleep before the next batch.
    /// Must not be called more often than `sysinfo::MINIMUM_CPU_UPDATE_INTERVAL` apart — the
    /// batch loop this is used from (`spawn_full_precompute_job`, ~200 archives/batch) naturally
    /// spaces calls further apart than that once the library is more than a couple hundred
    /// archives, so no explicit rate-limiting is added here.
    pub fn sample_and_backoff(&mut self, batch_elapsed: Duration) -> Duration {
        self.sys.refresh_cpu_usage();
        let cpu_pct = self.sys.global_cpu_usage();
        let delay = if cpu_pct > 80.0 {
            batch_elapsed * 3
        } else if cpu_pct > 50.0 {
            batch_elapsed
        } else {
            Duration::ZERO
        };
        tracing::debug!(
            cpu_pct,
            delay_ms = delay.as_millis(),
            "precompute load throttle sample"
        );
        delay
    }
}

impl Default for LoadThrottle {
    fn default() -> Self {
        Self::new()
    }
}

/// How close a candidate's cosine similarity must be to the new archive before it's considered
/// "similar enough" to backfill into that candidate's own Top-N list — same-series titles
/// (verified via `lanrurugi-recommend`'s own acceptance fixture) cluster well above this in
/// practice; this just avoids inserting every archive in the library into every other archive's
/// list regardless of relevance.
const BACKFILL_SIMILARITY_THRESHOLD: f32 = 0.5;

/// Incremental precompute for one archive — called (fired, not awaited) whenever an archive is
/// newly catalogued or its title changes. Embeds the title, ranks it against every already-cached
/// vector to build its own Top-N, persists both, then does a one-way backfill: any existing
/// archive whose similarity to this one clears [`BACKFILL_SIMILARITY_THRESHOLD`] gets this
/// archive spliced into its own cached Top-N (re-sorted, truncated). Never touches archives that
/// aren't already cached — those get their own entry the next time *they're* the one being
/// (re)computed, or during a full rebuild.
pub async fn precompute_one(state: &AppState, archive_id: &str, title: &str) {
    let cache = state.recommend_cache.clone();
    let normalized = lanrurugi_recommend::recommend::normalize_title(title);

    let vector = match state
        .recommender
        .embed_worker_client()
        .embed(&normalized)
        .await
    {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(archive_id, error = %e, "precompute_one: embedding failed");
            return;
        }
    };

    if let Err(e) = cache.put_vector(archive_id, title, &vector).await {
        tracing::debug!(archive_id, error = %e, "precompute_one: failed to persist vector");
        return;
    }

    let all_vectors = match cache.get_vectors_all().await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(archive_id, error = %e, "precompute_one: failed to load cached vectors");
            return;
        }
    };

    let cores = precompute_worker_budget();
    let library_size = all_vectors.len();
    let precision = read_precision(state).await;
    let top_n = precision.top_n(cores, library_size);

    let self_id = archive_id.to_string();
    let self_vector = vector.clone();
    // `Arc`-share the whole cached vector set into the blocking task instead of deep-cloning a
    // filtered `Vec<(String, Vec<f32>)>` copy of it — every entry's own `Vec<f32>` embedding can
    // be sizeable, and this runs on every archive ingest/title change, not just the one-off full
    // rebuild below (which already uses this same `Arc` pattern for the same reason).
    let all_vectors = Arc::new(all_vectors);

    let self_id_for_blocking = self_id.clone();
    // Clones: the same `Arc`/vector are needed again by the backfill pass below.
    let self_vector_for_own = self_vector.clone();
    let all_vectors_for_own = all_vectors.clone();
    let (own_top_n, backfill_targets) = tokio::task::spawn_blocking(move || {
        let mut scored: Vec<(String, f32)> = all_vectors_for_own
            .iter()
            .filter(|(id, _, _)| id != &self_id_for_blocking)
            .map(|(id, _, v)| (id.clone(), cosine_similarity(&self_vector_for_own, v)))
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let backfill_targets: Vec<String> = scored
            .iter()
            .filter(|(_, score)| *score >= BACKFILL_SIMILARITY_THRESHOLD)
            .map(|(id, _)| id.clone())
            .collect();
        scored.truncate(top_n);
        let own_top_n: Vec<String> = scored.into_iter().map(|(id, _)| id).collect();
        (own_top_n, backfill_targets)
    })
    .await
    .unwrap_or_default();

    if let Err(e) = cache.put_topn(archive_id, &own_top_n).await {
        tracing::debug!(archive_id, error = %e, "precompute_one: failed to persist top-n");
    }

    // One-way backfill, in two passes because the target's *current* list has to be read (async)
    // before it can be re-ranked (CPU). Each target's list is rebuilt as "everything it already
    // had, plus this archive, re-scored against the target's own vector and re-sorted" — the
    // earlier append-and-truncate version was a no-op in practice: with the list already at
    // `top_n`, it pushed the new id onto a full `Vec` and immediately truncated it away again, so
    // a newly added volume never entered any existing archive's candidate pool (and a library that
    // was only ever grown incrementally could never recommend a series' newest volume at all).
    if !backfill_targets.is_empty() {
        let mut pending: Vec<(String, Vec<String>)> = Vec::new();
        for target_id in backfill_targets {
            match cache.get_topn(&target_id).await {
                // Target was never precomputed, or already knows about this archive.
                Ok(Some(list)) if !list.contains(&self_id) => pending.push((target_id, list)),
                Ok(_) => {}
                Err(e) => {
                    tracing::debug!(target_id, error = %e, "precompute_one: backfill read failed");
                }
            }
        }

        if !pending.is_empty() {
            let self_id_for_blocking = self_id.clone();
            let self_vector_for_blocking = self_vector.clone();
            let all_vectors = all_vectors.clone();
            let updates: Vec<(String, Vec<String>)> = tokio::task::spawn_blocking(move || {
                let by_id: HashMap<&str, &Vec<f32>> = all_vectors
                    .iter()
                    .map(|(id, _, vector)| (id.as_str(), vector))
                    .collect();
                let mut updates = Vec::with_capacity(pending.len());
                for (target_id, existing) in pending {
                    let Some(target_vec) = by_id.get(target_id.as_str()) else {
                        continue;
                    };
                    let updated = merge_topn(&existing, top_n, &self_id_for_blocking, |id| {
                        if id == self_id_for_blocking.as_str() {
                            Some(cosine_similarity(target_vec, &self_vector_for_blocking))
                        } else {
                            by_id
                                .get(id)
                                .map(|vector| cosine_similarity(target_vec, vector))
                        }
                    });
                    if updated != existing {
                        updates.push((target_id, updated));
                    }
                }
                updates
            })
            .await
            .unwrap_or_default();

            for (target_id, updated) in updates {
                if let Err(e) = cache.put_topn(&target_id, &updated).await {
                    tracing::debug!(target_id, error = %e, "precompute_one: backfill write failed");
                }
            }
        }
    }
}

/// Rebuilds one cached Top-N list with `new_id` spliced in at its true rank: re-scores every id it
/// already holds (via `score`, which resolves an id to its cosine similarity *to this list's own
/// anchor*), adds `new_id`, sorts and truncates. Ids that no longer resolve (a deleted archive's
/// vector is gone) just drop out — same dangling-reference tolerance the read path has.
fn merge_topn(
    existing: &[String],
    top_n: usize,
    new_id: &str,
    score: impl Fn(&str) -> Option<f32>,
) -> Vec<String> {
    let mut scored: Vec<(String, f32)> = existing
        .iter()
        .filter_map(|id| score(id).map(|s| (id.clone(), s)))
        .collect();
    if let Some(s) = score(new_id) {
        scored.push((new_id.to_string(), s));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen = std::collections::HashSet::new();
    scored.retain(|(id, _)| seen.insert(id.clone()));
    scored.truncate(top_n);
    scored.into_iter().map(|(id, _)| id).collect()
}

/// Full O(n²) rebuild across the whole library — the first-time backfill for a pre-existing
/// library (whose archives will otherwise never trigger [`precompute_one`], since nothing about
/// them changes after this feature ships) and whenever the precision tier changes (a tier change
/// widens/narrows `top_n`, which incremental one-way backfill can't retroactively apply to
/// archives that don't happen to get touched again). Runs as a [`JobRegistry`] job so its progress
/// is visible on the Jobs page like any other long-running maintenance task.
///
/// Resumable: every Top-N write during this job is tagged with the *current*
/// `rebuild_generation` (bumped by the caller before invoking this — see `settings.rs`'s
/// `put_settings` and `main.rs`'s backfill trigger); on each archive this skips re-embedding if a
/// vector already exists AND re-ranking if the archive's cached Top-N was already computed under
/// the current generation, so a process restart mid-rebuild resumes roughly where it left off
/// instead of starting over.
pub async fn spawn_full_precompute_job(state: &AppState, reason: &str) -> String {
    let jobs = state.jobs.clone();
    let job_id = jobs.create("recommend_precompute").await;
    let job_id_for_task = job_id.clone();
    let state = state.clone();
    let reason = reason.to_string();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;
        tracing::info!(reason, "recommend_precompute: full rebuild starting");

        let embedder = state.recommender.embed_worker_client();
        let archives = match state.repos.archives.list_all().await {
            Ok(a) => a,
            Err(e) => {
                jobs.fail(&job_id_for_task, e.to_string()).await;
                return;
            }
        };
        let generation = match state.recommend_cache.get_rebuild_generation().await {
            Ok(g) => g,
            Err(e) => {
                jobs.fail(&job_id_for_task, e.to_string()).await;
                return;
            }
        };

        let total = archives.len();
        if total == 0 {
            jobs.finish(&job_id_for_task, serde_json::json!({ "archives": 0 }))
                .await;
            return;
        }

        // Phase 1 (0.0-0.5): embed every archive not already cached with a matching title —
        // batched so `LoadThrottle` can sample real system load and back off between batches.
        const BATCH_SIZE: usize = 200;
        let mut throttle = LoadThrottle::new();
        let mut embedded_count = 0usize;
        for chunk in archives.chunks(BATCH_SIZE) {
            let batch_start = std::time::Instant::now();
            let to_embed: Vec<(String, String)> = {
                let mut pending = Vec::new();
                for archive in chunk {
                    let id = archive.id.to_string();
                    let cached = state.recommend_cache.get_vector(&id).await.ok().flatten();
                    if cached.as_ref().map(|(t, _)| t.as_str()) != Some(archive.title.as_str()) {
                        pending.push((id, archive.title.clone()));
                    }
                }
                pending
            };

            if !to_embed.is_empty() {
                // Sequential, not `rayon`-parallelized — unlike the pre-refactor in-process
                // `Embedder` (a real ONNX session this crate could fan multiple calls into via a
                // worker thread pool, `precompute_worker_budget()`'s own original reason for
                // existing), `embed()` is now one IPC round trip to `lanrurugi-embed-worker` per
                // title. That worker only ever runs one inference at a time on its own single
                // `Embedder` session either way (both `Embedder`'s own internal `Mutex` and this
                // client's own `call_lock` serialize it), so fanning these out across a `rayon`
                // pool would only add blocked-on-IPC rayon threads, not real parallelism.
                let mut embedded: Vec<(String, String, Vec<f32>)> =
                    Vec::with_capacity(to_embed.len());
                for (id, title) in &to_embed {
                    let norm = lanrurugi_recommend::recommend::normalize_title(title);
                    if let Ok(v) = embedder.embed(&norm).await {
                        embedded.push((id.clone(), title.clone(), v));
                    }
                }

                for (id, title, vector) in embedded {
                    if let Err(e) = state.recommend_cache.put_vector(&id, &title, &vector).await {
                        tracing::warn!(archive_id = %id, error = %e, "recommend_precompute: failed to persist vector");
                    }
                    embedded_count += 1;
                }
            }

            jobs.set_progress(
                &job_id_for_task,
                0.5 * (embedded_count.max(1) as f32 / total as f32).min(1.0),
            )
            .await;

            let delay = throttle.sample_and_backoff(batch_start.elapsed());
            if delay > Duration::ZERO {
                tokio::time::sleep(delay).await;
            }
        }

        // Phase 2 (0.5-1.0): rank every archive against the full cached vector set and persist its
        // Top-N, skipping any archive whose cached Top-N is already tagged with this generation.
        let all_vectors = match state.recommend_cache.get_vectors_all().await {
            Ok(v) => v,
            Err(e) => {
                jobs.fail(&job_id_for_task, e.to_string()).await;
                return;
            }
        };
        let library_size = all_vectors.len();
        let precision = read_precision(&state).await;
        let cores = precompute_worker_budget();
        let top_n = precision.top_n(cores, library_size);
        let vectors_arc = Arc::new(all_vectors);

        let mut ranked_count = 0usize;
        for chunk in archives.chunks(BATCH_SIZE) {
            let batch_start = std::time::Instant::now();
            let ids: Vec<String> = chunk.iter().map(|a| a.id.to_string()).collect();

            let mut to_rank = Vec::new();
            for id in &ids {
                let already_current = state
                    .recommend_cache
                    .get_topn_generation(id)
                    .await
                    .ok()
                    .flatten()
                    == Some(generation);
                if !already_current {
                    to_rank.push(id.clone());
                }
            }

            if !to_rank.is_empty() {
                let vectors_arc = vectors_arc.clone();
                let results: Vec<(String, Vec<String>)> = tokio::task::spawn_blocking(move || {
                    // The workspace's shared capped pool, not a fresh one per batch: this used to
                    // build a thread pool for every `BATCH_SIZE` chunk.
                    lanrurugi_core::concurrency::with_thread_pool(|| {
                        use rayon::prelude::*;
                        to_rank
                            .into_par_iter()
                            .filter_map(|id| {
                                let self_vector =
                                    vectors_arc.iter().find(|(vid, _, _)| vid == &id)?.2.clone();
                                let mut scored: Vec<(String, f32)> = vectors_arc
                                    .iter()
                                    .filter(|(vid, _, _)| vid != &id)
                                    .map(|(vid, _, v)| {
                                        (vid.clone(), cosine_similarity(&self_vector, v))
                                    })
                                    .collect();
                                scored.sort_by(|a, b| {
                                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                                });
                                scored.truncate(top_n);
                                Some((id, scored.into_iter().map(|(vid, _)| vid).collect()))
                            })
                            .collect()
                    })
                })
                .await
                .unwrap_or_default();

                for (id, list) in results {
                    if let Err(e) = state.recommend_cache.put_topn(&id, &list).await {
                        tracing::warn!(archive_id = %id, error = %e, "recommend_precompute: failed to persist top-n");
                    }
                    if let Err(e) = state
                        .recommend_cache
                        .put_topn_generation(&id, generation)
                        .await
                    {
                        tracing::warn!(archive_id = %id, error = %e, "recommend_precompute: failed to persist top-n generation");
                    }
                    ranked_count += 1;
                }
            }

            jobs.set_progress(
                &job_id_for_task,
                0.5 + 0.5 * (ranked_count.max(1) as f32 / total as f32).min(1.0),
            )
            .await;

            let delay = throttle.sample_and_backoff(batch_start.elapsed());
            if delay > Duration::ZERO {
                tokio::time::sleep(delay).await;
            }
        }

        jobs.finish(
            &job_id_for_task,
            serde_json::json!({ "archives": total, "embedded": embedded_count, "ranked": ranked_count }),
        )
        .await;
        tracing::info!(total, "recommend_precompute: full rebuild finished");
    });

    job_id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn low_precision_top_n_is_floored_at_100_regardless_of_hardware() {
        assert!(RecommendPrecision::Low.top_n(1, 10) >= 100);
        assert!(RecommendPrecision::Low.top_n(2, 500) >= 100);
        assert!(RecommendPrecision::Low.top_n(64, 1_000_000) >= 100);
    }

    #[test]
    fn every_precision_tier_can_fill_the_llm_shortlist() {
        // The read path promises `limit.max(PREFILTER_COUNT)` candidates; a 200-archive library on
        // `medium` used to cache 24, which is what starved the rerank of the anchor's own series.
        for precision in [
            RecommendPrecision::Low,
            RecommendPrecision::Medium,
            RecommendPrecision::High,
        ] {
            assert!(
                precision.top_n(2, 234) >= crate::recommend_llm::PREFILTER_COUNT,
                "{precision:?} must be able to fill the prefilter shortlist"
            );
        }
    }

    #[test]
    fn precision_scales_up_with_more_cores_and_larger_library() {
        let small = RecommendPrecision::Medium.top_n(4, 1_000);
        let large = RecommendPrecision::Medium.top_n(32, 1_000_000);
        assert!(large > small);
    }

    #[test]
    fn merge_topn_inserts_at_the_true_rank_not_the_tail() {
        // A full list at `top_n` must make room for a score that beats its weakest member — the
        // bug this replaces pushed and then truncated the newcomer straight back off.
        let existing: Vec<String> = (0..4).map(|i| format!("old{i}")).collect();
        let scores: HashMap<&str, f32> = [
            ("old0", 0.9),
            ("old1", 0.8),
            ("old2", 0.7),
            ("old3", 0.6),
            ("new", 0.95),
        ]
        .into_iter()
        .collect();
        let merged = merge_topn(&existing, 4, "new", |id| scores.get(id).copied());
        assert_eq!(merged, vec!["new", "old0", "old1", "old2"]);
    }

    #[test]
    fn merge_topn_keeps_the_list_when_the_new_id_does_not_make_the_cut() {
        let existing: Vec<String> = (0..3).map(|i| format!("old{i}")).collect();
        let scores: HashMap<&str, f32> =
            [("old0", 0.9), ("old1", 0.8), ("old2", 0.7), ("new", 0.1)]
                .into_iter()
                .collect();
        assert_eq!(
            merge_topn(&existing, 3, "new", |id| scores.get(id).copied()),
            existing
        );
    }

    #[test]
    fn merge_topn_drops_ids_whose_vector_is_gone() {
        let existing = vec!["a".to_string(), "deleted".to_string()];
        let scores: HashMap<&str, f32> = [("a", 0.5), ("new", 0.9)].into_iter().collect();
        assert_eq!(
            merge_topn(&existing, 10, "new", |id| scores.get(id).copied()),
            vec!["new", "a"]
        );
    }

    #[test]
    fn from_setting_round_trips_through_as_setting() {
        for p in [
            RecommendPrecision::Low,
            RecommendPrecision::Medium,
            RecommendPrecision::High,
        ] {
            assert_eq!(RecommendPrecision::from_setting(p.as_setting()), p);
        }
    }

    #[test]
    fn from_setting_defaults_to_medium_for_unknown_values() {
        assert_eq!(
            RecommendPrecision::from_setting("garbage"),
            RecommendPrecision::Medium
        );
    }

    #[test]
    fn worker_budget_is_always_at_least_one() {
        assert!(precompute_worker_budget() >= 1);
    }
}
