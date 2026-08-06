//! Reader recommendation endpoint — `GET /api/reader/recommendations/{id}?limit=N`.
//!
//! Backed by `lanrurugi-recommend` (ONNX embedding of titles via
//! `paraphrase-multilingual-MiniLM-L12-v2`; series recognition is embedding similarity, see that
//! crate's own docs). This module owns the process-lifetime state: the loaded [`Embedder`] (set
//! once the startup model download+load completes, see `lanrurugi-server`'s main) and a per-id
//! vector cache keyed by title — re-embedding only happens when an archive's title changed since
//! it was cached, so the common case (recommend against an already-embedded library) is pure
//! cosine math.
//!
//! Until the model is ready the endpoint returns `503` with a machine-readable `code`
//! (`model_not_ready`); the frontend shows the panel disabled / a spinner. This keeps the reader
//! fully functional on an offline or first-boot server while the 118MB model downloads.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

use lanrurugi_recommend::embedding::Embedder;
use lanrurugi_recommend::recommend::{ArchiveMeta, Recommendation};

use crate::common::{error, not_found};
use crate::AppState;

#[derive(Debug, Error)]
pub enum RecommendServiceError {
    #[error("embedding model is not ready yet (still downloading/loading)")]
    ModelNotReady,
    #[error("archive {0} not found")]
    ArchiveNotFound(String),
    #[error("embedding failed: {0}")]
    Embedding(#[from] lanrurugi_recommend::embedding::EmbeddingError),
}

/// `archive id → (title it was embedded from, vector)`.
type VectorCache = HashMap<String, (String, Arc<Vec<f32>>)>;

/// Process-lifetime recommender state. Cheap to clone (`Arc`-backed) and shared through
/// `AppState`.
#[derive(Default)]
pub struct RecommendService {
    /// `None` until the startup model download+load finishes (`install_embedder`).
    embedder: Mutex<Option<Arc<Embedder>>>,
    /// Re-embed only when the title changed, so a steady library is recommended against with
    /// zero inference.
    vectors: Mutex<VectorCache>,
}

impl RecommendService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called once by `lanrurugi-server`'s startup task after the model files are acquired and
    /// loaded. Not behind a lock-and-replace race: this runs exactly once, before any request
    /// can meaningfully use it (the server hasn't bound its listener until main has run).
    pub fn install_embedder(&self, embedder: Embedder) {
        *self.embedder.lock().unwrap() = Some(Arc::new(embedder));
    }

    pub fn ready(&self) -> bool {
        self.embedder.lock().unwrap().is_some()
    }

    /// Shares the one process-lifetime [`Embedder`] with `recommend_precompute.rs` — there is
    /// deliberately no second `Embedder::load` for the batch precompute job. The session inside
    /// is `Mutex`-wrapped either way, so a second loaded copy would only double the ~118MB model's
    /// resident memory for zero extra parallelism; the *actual* precompute throughput knob is the
    /// `intra_threads` value this single instance was loaded with (`main.rs`'s startup task).
    pub fn embedder(&self) -> Option<Arc<Embedder>> {
        self.embedder.lock().unwrap().clone()
    }

    /// Ranks the whole library by embedding similarity to `archive_id`'s title. Prefers the
    /// persisted cache (`lanrurugi_storage::recommend_cache`, written by
    /// `recommend_precompute.rs` at catalogue/title-change time) — a hit skips both the O(n)
    /// `archives.list_all()` Redis read and any ONNX inference entirely, so latency stops growing
    /// with library size (issue #70: the un-cached path below took 3.13s/1k, 18.06s/5k,
    /// 56.61s/20k archives cold, and didn't finish within a 2-minute timeout at 100k). On a miss —
    /// a fresh install before the first-time backfill completes, or an archive
    /// `precompute_one` hasn't gotten to yet — this falls through unchanged to the original
    /// current-request embedding path (embedding work runs inside `spawn_blocking` so an HTTP
    /// handler thread isn't pinned while ONNX infers), and fires a background `precompute_one`
    /// so the *next* request for this archive hits the cache instead.
    pub async fn recommendations(
        &self,
        state: &AppState,
        archive_id: &str,
        limit: usize,
        exclude_ids: &[String],
    ) -> Result<Vec<Recommendation>, RecommendServiceError> {
        let embedder = self
            .embedder
            .lock()
            .unwrap()
            .clone()
            .ok_or(RecommendServiceError::ModelNotReady)?;
        let current = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(archive_id.to_string()))
            .await
            .map_err(|e| RecommendServiceError::Embedding(embed_err_from_repo(e)))?
            .ok_or_else(|| RecommendServiceError::ArchiveNotFound(archive_id.to_string()))?;
        let current_title = current.title.clone();

        if let Some(cached) = self
            .recommendations_from_cache(state, archive_id, &current, limit, exclude_ids)
            .await
        {
            return Ok(cached);
        }

        let all = state
            .repos
            .archives
            .list_all()
            .await
            .map_err(|e| RecommendServiceError::Embedding(embed_err_from_repo(e)))?;
        let candidates: Vec<ArchiveMeta> = all
            .iter()
            .map(|a| ArchiveMeta {
                id: a.id.to_string(),
                title: a.title.clone(),
                tags: a.tags.clone(),
                pagecount: a.pagecount,
            })
            .collect();

        // Embed everything not yet cached (or whose title changed) on a blocking thread — the
        // session mutex + ONNX inference are synchronous CPU work. The cache lock is NOT held
        // across the `await` (a `std::sync::MutexGuard` isn't `Send`, which would break the
        // handler future) — collect the missing ids under the lock, release it, await the
        // blocking embed, then re-lock to insert.
        let missing: Vec<(String, String)> = {
            let cache = self.vectors.lock().unwrap();
            candidates
                .iter()
                .filter(|c| cache.get(&c.id).map(|(t, _)| t.as_str()) != Some(c.title.as_str()))
                .map(|c| (c.id.clone(), c.title.clone()))
                .collect()
        };
        if !missing.is_empty() {
            let missing_for_blocking = missing.clone();
            let embedder_for_blocking = embedder.clone();
            let embedded: Vec<(String, String, Arc<Vec<f32>>)> =
                tokio::task::spawn_blocking(move || {
                    missing_for_blocking
                        .iter()
                        .filter_map(|(id, title)| {
                            embedder_for_blocking
                                .embed(title)
                                .ok()
                                .map(|v| (id.clone(), title.clone(), Arc::new(v)))
                        })
                        .collect()
                })
                .await
                .map_err(|e| RecommendServiceError::Embedding(embed_err_from_join(e)))?;
            let mut cache = self.vectors.lock().unwrap();
            for (id, title, vec) in embedded {
                cache.insert(id, (title, vec));
            }
        }
        let vectors_snapshot = {
            let cache = self.vectors.lock().unwrap();
            cache
                .iter()
                .map(|(id, (title, vec))| {
                    (
                        ArchiveMeta {
                            id: id.clone(),
                            title: title.clone(),
                            tags: String::new(),
                            pagecount: 0,
                        },
                        vec.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        // Rank on the blocking thread too (pure dot products, but keeps the handler thread free).
        // Prefilter generously (the LLM rerank needs room to pick the true next volume even if
        // embedding ranked it a few places down); the final `limit` is applied after reranking.
        let prefilter_limit = limit.max(crate::recommend_llm::PREFILTER_COUNT);
        // Beyond the anchor itself, exclude every other member of the same Tankoubon (if the
        // caller resolved a Tankoubon down to its last volume as the anchor) — the reader has
        // already read all of them, so recommending a sibling volume back would just point at
        // something already finished instead of something new.
        let exclude_set: std::collections::HashSet<String> = exclude_ids
            .iter()
            .cloned()
            .chain([archive_id.to_string()])
            .collect();
        let embedder_for_blocking = embedder.clone();
        let current_vec = embedder_for_blocking.embed(
            &lanrurugi_recommend::recommend::normalize_title(&current_title),
        )?;
        // Two-tier: the embedding order above IS the pre-filter. If an LLM API key is
        // configured, hand the top shortlist to the LLM for the "next volume first" rerank (the
        // one thing embedding can't do); any LLM failure falls back to this embedding order.
        let ranked: Vec<Recommendation> = tokio::task::spawn_blocking(move || {
            let mut scored: Vec<Recommendation> = Vec::with_capacity(vectors_snapshot.len());
            for (meta, vec) in &vectors_snapshot {
                if exclude_set.contains(&meta.id) {
                    continue;
                }
                scored.push(Recommendation {
                    id: meta.id.clone(),
                    title: meta.title.clone(),
                    score: lanrurugi_recommend::embedding::cosine_similarity(&current_vec, vec),
                });
            }
            scored.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            scored.truncate(prefilter_limit);
            scored
        })
        .await
        .map_err(|e| RecommendServiceError::Embedding(embed_err_from_join(e)))?;

        // LLM rerank of the shortlist (next-volume-first — the one thing embedding can't do).
        // Any failure (no key, network, non-JSON) falls back to the embedding order above.
        let current_tags = current.tags.clone();
        let current_pagecount = current.pagecount;
        // The prefiltered shortlist only carries title from the embedding layer; re-attach the
        // tags and pagecount from the repository for the LLM prompt.
        let prefiltered: Vec<ArchiveMeta> = {
            let all_meta = all;
            ranked
                .iter()
                .map(|r| {
                    let meta = all_meta
                        .iter()
                        .find(|a| a.id.to_string() == r.id)
                        .map(|a| (a.tags.clone(), a.pagecount))
                        .unwrap_or_default();
                    ArchiveMeta {
                        id: r.id.clone(),
                        title: r.title.clone(),
                        tags: meta.0,
                        pagecount: meta.1,
                    }
                })
                .collect()
        };
        let result: Vec<Recommendation> = match crate::recommend_llm::llm_rerank(
            state,
            &current_title,
            &current_tags,
            current_pagecount,
            &prefiltered,
            limit,
        )
        .await
        {
            Some(llm_ranked) => llm_ranked,
            None => ranked.into_iter().take(limit).collect(),
        };

        // Fire-and-forget: this request paid the O(n) embed-everything cost because the cache
        // missed for `archive_id`. Precompute it now so the *next* request for this archive (or
        // one whose one-way backfill lands on it) hits the fast path instead. Not awaited — the
        // response above must not wait on this.
        {
            let state = state.clone();
            let archive_id = archive_id.to_string();
            let current_title = current_title.clone();
            tokio::spawn(async move {
                crate::recommend_precompute::precompute_one(&state, &archive_id, &current_title)
                    .await;
            });
        }

        Ok(result)
    }

    /// Cache-hit fast path: reads `archive_id`'s persisted Top-N list, hydrates each id into an
    /// [`ArchiveMeta`] via `archives.get()` (a `HashMap` keyed by id avoids the O(n) linear
    /// `.find()` the fallback path uses — irrelevant there since it already holds the full list in
    /// memory, but this path deliberately never loads the full list at all), re-scores against a
    /// freshly-embedded current title (cheap for ≤~few hundred cached ids — avoids serving a
    /// score that's gone stale relative to whatever the cache last computed it against), then
    /// runs the same LLM-rerank-with-fallback tail as the miss path. Returns `None` on any miss
    /// (no cached Top-N, cache read error, or every cached id having since been deleted) so the
    /// caller falls through to full recomputation — never an error, since a miss here is an
    /// expected, common state (fresh install, archive not yet precomputed).
    async fn recommendations_from_cache(
        &self,
        state: &AppState,
        archive_id: &str,
        current: &lanrurugi_core::entities::Archive,
        limit: usize,
        exclude_ids: &[String],
    ) -> Option<Vec<Recommendation>> {
        let cached_ids = state.recommend_cache.get_topn(archive_id).await.ok()??;
        if cached_ids.is_empty() {
            return None;
        }

        let exclude_set: std::collections::HashSet<String> = exclude_ids
            .iter()
            .cloned()
            .chain([archive_id.to_string()])
            .collect();

        let embedder = self.embedder()?;
        let current_title = current.title.clone();
        let embedder_for_blocking = embedder.clone();
        let current_vec = tokio::task::spawn_blocking(move || {
            embedder_for_blocking.embed(&lanrurugi_recommend::recommend::normalize_title(
                &current_title,
            ))
        })
        .await
        .ok()?
        .ok()?;

        // Hydrate each cached id into an ArchiveMeta + its persisted vector in one pass, skipping
        // any id that's since been deleted (a dangling reference `recommend_cache::delete_for`
        // deliberately leaves behind rather than scrubbing every other archive's Top-N on every
        // delete — see that method's own docs) or excluded (Tankoubon sibling).
        let prefilter_limit = limit.max(crate::recommend_llm::PREFILTER_COUNT);
        let mut scored: Vec<Recommendation> = Vec::with_capacity(cached_ids.len());
        for id in cached_ids.iter().take(prefilter_limit * 2) {
            if exclude_set.contains(id) {
                continue;
            }
            let Some((_, vec)) = state.recommend_cache.get_vector(id).await.ok().flatten() else {
                continue;
            };
            let Some(meta) = state
                .repos
                .archives
                .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
                .await
                .ok()
                .flatten()
            else {
                continue;
            };
            scored.push(Recommendation {
                id: id.clone(),
                title: meta.title,
                score: lanrurugi_recommend::embedding::cosine_similarity(&current_vec, &vec),
            });
            if scored.len() >= prefilter_limit {
                break;
            }
        }
        if scored.is_empty() {
            return None;
        }
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        match crate::recommend_llm::llm_rerank(
            state,
            &current.title,
            &current.tags,
            current.pagecount,
            &self.rehydrate_tags_and_pagecount(state, &scored).await,
            limit,
        )
        .await
        {
            Some(llm_ranked) => Some(llm_ranked),
            None => Some(scored.into_iter().take(limit).collect()),
        }
    }

    /// Re-attaches `tags`/`pagecount` (not stored in the vector cache — see
    /// `recommend_cache`'s own docs, only `title` rides alongside the vector) to a scored
    /// shortlist for the LLM rerank prompt, which wants both fields (`recommend_llm.rs`'s prompt
    /// format).
    async fn rehydrate_tags_and_pagecount(
        &self,
        state: &AppState,
        scored: &[Recommendation],
    ) -> Vec<ArchiveMeta> {
        let mut out = Vec::with_capacity(scored.len());
        for r in scored {
            let (tags, pagecount) = state
                .repos
                .archives
                .get(&lanrurugi_core::ids::ArchiveId(r.id.clone()))
                .await
                .ok()
                .flatten()
                .map(|a| (a.tags, a.pagecount))
                .unwrap_or_default();
            out.push(ArchiveMeta {
                id: r.id.clone(),
                title: r.title.clone(),
                tags,
                pagecount,
            });
        }
        out
    }
}

fn embed_err_from_repo(
    e: impl std::fmt::Display,
) -> lanrurugi_recommend::embedding::EmbeddingError {
    lanrurugi_recommend::embedding::EmbeddingError::BadOutput(e.to_string())
}

fn embed_err_from_join(
    e: tokio::task::JoinError,
) -> lanrurugi_recommend::embedding::EmbeddingError {
    lanrurugi_recommend::embedding::EmbeddingError::BadOutput(format!("blocking task failed: {e}"))
}

pub fn router() -> Router<AppState> {
    Router::new().route("/reader/recommendations/{id}", get(get_recommendations))
}

#[derive(Debug, Deserialize, Default)]
pub struct RecommendationQuery {
    limit: Option<usize>,
}

async fn get_recommendations(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<RecommendationQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(10).clamp(1, 20);

    // A Tankoubon has no title/tags of its own to embed — recommendations are computed against
    // its *last* member archive instead (the boundary overlay only ever opens after finishing
    // that last volume's last page, so "what's similar to the volume just finished" is exactly
    // the anchor a reader closing out a Tankoubon actually wants). The response still reports
    // back the original Tankoubon `id` (not the resolved archive id) so the frontend's cache key
    // and displayed "recommendations for X" stay keyed on what the reader was actually viewing.
    // Also exclude every other member of the same Tankoubon from the candidate pool — the
    // reader has already read all of them, so recommending a sibling volume back would just
    // point at something already finished instead of something new.
    let (anchor_id, exclude_ids) = if id.starts_with("TANK_") {
        match state
            .repos
            .groupings
            .get(&lanrurugi_core::ids::TankId(id.clone()))
            .await
        {
            Ok(Some(tank)) => match tank.archives.last() {
                Some(last) => (
                    last.to_string(),
                    tank.archives.iter().map(|a| a.to_string()).collect(),
                ),
                None => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "get_recommendations",
                        "Tankoubon has no member archives.",
                    )
                }
            },
            Ok(None) => return not_found("get_recommendations", format!("{id} does not exist.")),
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "get_recommendations",
                    e.to_string(),
                )
            }
        }
    } else {
        (id.clone(), Vec::new())
    };

    match state
        .recommender
        .recommendations(&state, &anchor_id, limit, &exclude_ids)
        .await
    {
        Ok(list) => {
            // Enrich each recommendation with the card-status fields the frontend's badge
            // overlays need (🆕 new / 👑 read / 📚 tankoubon) — `is_read` matches the
            // display-side rule elsewhere (`progress >= pagecount`), `isnew` is the raw stored
            // flag (the badge mode filter lives on the search path; here the Library-card
            // equivalent is just informational).
            let mut enriched = Vec::with_capacity(list.len());
            for r in &list {
                let meta = state
                    .repos
                    .archives
                    .get(&lanrurugi_core::ids::ArchiveId(r.id.clone()))
                    .await
                    .ok()
                    .flatten();
                enriched.push(json!({
                    "archive_id": r.id,
                    "title": r.title,
                    "score": r.score,
                    "isnew": meta.as_ref().map(|a| a.isnew).unwrap_or(false),
                    "is_read": meta.as_ref()
                        .map(|a| a.lastreadpage > 0 && a.lastreadpage >= a.pagecount)
                        .unwrap_or(false),
                    "is_tank": r.id.starts_with("TANK_"),
                }));
            }
            axum::Json(json!({ "archive_id": id, "recommendations": enriched })).into_response()
        }
        Err(RecommendServiceError::ModelNotReady) => {
            (axum::http::StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({
                "error": "model_not_ready",
                "message": "The recommendation model is still downloading or loading — try again shortly.",
            })))
            .into_response()
        }
        Err(RecommendServiceError::ArchiveNotFound(id)) => crate::common::not_found(
            "get_recommendations",
            format!("Archive {id} doesn't exist in the database!"),
        ),
        Err(e) => error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "get_recommendations",
            e.to_string(),
        ),
    }
}
