//! Sharing blobs: the engine-side half lives in `lanrurugi_share::blobs`; this module keeps the
//! background preparation worker, which is a host concern (jobs, load backoff).

use lanrurugi_core::ids::ArchiveId;

use deadpool_redis::redis::AsyncCommands;

use crate::AppState;

pub use lanrurugi_share::blobs::{ensure_ready, prepare, ready, BlobInfo, BLOCK_SIZE};

/// Archives fingerprinted per batch of the background job.
const PREPARE_BATCH: isize = 16;
const PREPARE_IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// Drains the preparation queue in the background, one `share_prepare` job per busy period, with
/// the same load backoff as the other background jobs (research §13). An importer that needs an
/// archive first does not wait for this: its `Prepare` request hashes that archive directly.
pub fn spawn_prepare_worker(state: AppState) {
    tokio::spawn(async move {
        let mut throttle = crate::recommend_precompute::LoadThrottle::new();
        let mut job: Option<String> = None;
        loop {
            let batch: Vec<String> = match state.redis.config.get().await {
                Ok(mut conn) => conn
                    .zrange(
                        lanrurugi_storage::keys::SHARE_PREPARE_KEY,
                        0,
                        PREPARE_BATCH - 1,
                    )
                    .await
                    .unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            if batch.is_empty() {
                if let Some(id) = job.take() {
                    state.jobs.finish(&id, serde_json::json!({})).await;
                }
                tokio::time::sleep(PREPARE_IDLE).await;
                continue;
            }
            if job.is_none() {
                let id = state.jobs.create("share_prepare").await;
                state.jobs.mark_active(&id).await;
                job = Some(id);
            }
            let started = std::time::Instant::now();
            let ids: Vec<ArchiveId> = batch.iter().cloned().map(ArchiveId).collect();
            prepare(&state, &ids).await;
            if let Ok(mut conn) = state.redis.config.get().await {
                let _: Result<(), _> = conn
                    .zrem(lanrurugi_storage::keys::SHARE_PREPARE_KEY, &batch)
                    .await;
            }
            tokio::time::sleep(throttle.sample_and_backoff(started.elapsed())).await;
        }
    });
}
