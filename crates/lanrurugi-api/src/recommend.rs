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
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

use lanrurugi_recommend::embedding::Embedder;
use lanrurugi_recommend::recommend::{ArchiveMeta, Recommendation};

use crate::common::error;
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

    /// Ranks the whole library by embedding similarity to `archive_id`'s title. Embedding work
    /// (the slow first pass over un-cached titles) runs inside `spawn_blocking` so an HTTP
    /// handler thread isn't pinned while ONNX infers.
    pub async fn recommendations(
        &self,
        state: &AppState,
        archive_id: &str,
        limit: usize,
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
                        },
                        vec.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        // Rank on the blocking thread too (pure dot products, but keeps the handler thread free).
        let embedder_for_blocking = embedder.clone();
        let current_vec = embedder_for_blocking.embed(
            &lanrurugi_recommend::recommend::normalize_title(&current_title),
        )?;
        let exclude_id = archive_id.to_string();
        tokio::task::spawn_blocking(move || {
            let mut scored: Vec<Recommendation> = Vec::with_capacity(vectors_snapshot.len());
            for (meta, vec) in &vectors_snapshot {
                if meta.id == exclude_id {
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
            scored.truncate(limit);
            scored
        })
        .await
        .map_err(|e| RecommendServiceError::Embedding(embed_err_from_join(e)))
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
    match state.recommender.recommendations(&state, &id, limit).await {
        Ok(list) => axum::Json(json!({
            "archive_id": id,
            "recommendations": list.iter().map(|r| json!({
                "archive_id": r.id,
                "title": r.title,
                "score": r.score,
            })).collect::<Vec<_>>(),
        }))
        .into_response(),
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
