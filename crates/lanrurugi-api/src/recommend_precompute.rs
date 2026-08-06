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
//!   to the new/changed one clears the cache's own tail-score threshold get it spliced into their
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

use std::sync::Arc;
use std::time::Duration;

use lanrurugi_recommend::embedding::cosine_similarity;

use crate::AppState;

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
    /// constant per tier. **Low is floored at 100** regardless of hardware/library size — the read
    /// path (`recommend.rs`) always prefilters to at least
    /// `limit.max(recommend_llm::PREFILTER_COUNT)` candidates before the Tankoubon
    /// sibling-exclusion filter runs, and a cache shorter than that would starve the LLM rerank of
    /// a shortlist to choose from on any archive with several already-owned sibling volumes.
    pub fn top_n(self, cores: usize, library_size: usize) -> usize {
        let cores = cores.max(1);
        let scale = match self {
            Self::Low => 4,
            Self::Medium => 12,
            Self::High => 30,
        };
        let by_hardware = cores * scale;
        let by_library = library_size / 10;
        let n = by_hardware.min(by_library).max(scale * 2);
        match self {
            Self::Low => n.max(100),
            _ => n,
        }
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
    let Some(embedder) = state.recommender.embedder() else {
        return; // Model not loaded yet — nothing to do; the ingest/rename call sites don't retry.
    };
    let cache = state.recommend_cache.clone();
    let normalized = lanrurugi_recommend::recommend::normalize_title(title);

    let embedder_for_blocking = embedder.clone();
    let vector = match tokio::task::spawn_blocking(move || embedder_for_blocking.embed(&normalized))
        .await
    {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::debug!(archive_id, error = %e, "precompute_one: embedding failed");
            return;
        }
        Err(e) => {
            tracing::debug!(archive_id, error = %e, "precompute_one: blocking task join failed");
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
    let others: Vec<(String, Vec<f32>)> = all_vectors
        .iter()
        .filter(|(id, _, _)| id != &self_id)
        .map(|(id, _, v)| (id.clone(), v.clone()))
        .collect();

    let (own_top_n, backfill_targets) = tokio::task::spawn_blocking(move || {
        let mut scored: Vec<(String, f32)> = others
            .iter()
            .map(|(id, v)| (id.clone(), cosine_similarity(&self_vector, v)))
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

    for target_id in backfill_targets {
        let existing = match cache.get_topn(&target_id).await {
            Ok(Some(list)) => list,
            Ok(None) => continue, // Target was never precomputed — leave it for its own turn.
            Err(e) => {
                tracing::debug!(target_id, error = %e, "precompute_one: backfill read failed");
                continue;
            }
        };
        if existing.contains(&self_id) {
            continue;
        }
        let mut updated = existing;
        updated.push(self_id.clone());
        updated.truncate(top_n.max(updated.len().min(top_n + 1)));
        updated.truncate(top_n);
        if let Err(e) = cache.put_topn(&target_id, &updated).await {
            tracing::debug!(target_id, error = %e, "precompute_one: backfill write failed");
        }
    }
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

        let Some(embedder) = state.recommender.embedder() else {
            jobs.fail(&job_id_for_task, "embedding model not ready")
                .await;
            return;
        };
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
                let embedder = embedder.clone();
                let normalized: Vec<(String, String, String)> = to_embed
                    .iter()
                    .map(|(id, title)| {
                        (
                            id.clone(),
                            title.clone(),
                            lanrurugi_recommend::recommend::normalize_title(title),
                        )
                    })
                    .collect();
                let cores = precompute_worker_budget();
                let embedded: Vec<(String, String, Vec<f32>)> =
                    tokio::task::spawn_blocking(move || {
                        let pool = rayon::ThreadPoolBuilder::new()
                            .num_threads(cores)
                            .build()
                            .expect("rayon pool build");
                        pool.install(|| {
                            use rayon::prelude::*;
                            normalized
                                .into_par_iter()
                                .filter_map(|(id, title, norm)| {
                                    embedder.embed(&norm).ok().map(|v| (id, title, v))
                                })
                                .collect()
                        })
                    })
                    .await
                    .unwrap_or_default();

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
                    let pool = rayon::ThreadPoolBuilder::new()
                        .num_threads(cores)
                        .build()
                        .expect("rayon pool build");
                    pool.install(|| {
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
    fn precision_scales_up_with_more_cores_and_larger_library() {
        let small = RecommendPrecision::Medium.top_n(4, 1_000);
        let large = RecommendPrecision::Medium.top_n(32, 1_000_000);
        assert!(large > small);
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
