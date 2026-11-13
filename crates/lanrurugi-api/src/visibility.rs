//! Ingest visibility gate (spec 025): an archive is hidden from every search and read endpoint
//! from just before its bytes reach the library until the ingest's metadata plugins have returned.
//!
//! The state is one Redis set in the archive DB ([`PENDING_VISIBILITY_SET`]) — the DB the search
//! engine and the archive repository read. There is no TTL: an ingest publishes explicitly, and
//! startup ([`clear_all_pending`]) publishes whatever a crash left behind, so an archive can show
//! up late but never stay hidden.

use std::collections::HashSet;
use std::time::Duration;

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_storage::keys::{LEGACY_PENDING_VISIBILITY_PREFIX, PENDING_VISIBILITY_SET};

use crate::state::AppState;

#[derive(Debug, thiserror::Error)]
pub enum VisibilityError {
    #[error("redis pool: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("redis: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
}

/// Hides `archive_id`. An error must fail the ingest: carrying on would make it visible early.
pub async fn mark_pending(state: &AppState, archive_id: &str) -> Result<(), VisibilityError> {
    mark_pending_in(&state.redis.archive, archive_id).await
}

/// Publishes `archive_id`. Retried a few times; a final failure is logged and left to the startup
/// sweep, since the ingest itself already succeeded.
pub async fn publish(state: &AppState, archive_id: &str) {
    publish_in(&state.redis.archive, archive_id).await
}

/// Whether `archive_id` is still hidden. A Redis error counts as hidden: the record itself lives in
/// the same DB, so nothing useful could be served anyway.
pub async fn is_pending(state: &AppState, archive_id: &str) -> bool {
    is_pending_in(&state.redis.archive, archive_id).await
}

/// Every hidden id; empty on a Redis error (callers that list archives cannot list them either).
pub async fn pending_ids(state: &AppState) -> HashSet<String> {
    pending_ids_in(&state.redis.archive).await
}

/// Startup convergence: publishes everything still hidden and deletes the per-archive marker keys
/// the first implementation wrote. Returns how many archives were published.
pub async fn clear_all_pending(state: &AppState) -> usize {
    clear_all_pending_in(&state.redis.archive, &state.redis.config).await
}

/// Publishes its archive if dropped before [`Self::publish`]/[`Self::hand_over`]: a request future
/// can be cancelled (an upload
/// whose client disconnects mid plugin run), and without this the archive would stay hidden until
/// the next restart. The normal paths call [`Self::publish`] or [`Self::hand_over`].
pub struct PendingGuard {
    state: Option<AppState>,
    archive_id: String,
}

impl PendingGuard {
    pub fn new(state: &AppState, archive_id: &str) -> Self {
        Self {
            state: Some(state.clone()),
            archive_id: archive_id.to_string(),
        }
    }

    pub async fn publish(mut self) {
        if let Some(state) = self.state.take() {
            publish(&state, &self.archive_id).await;
        }
    }

    /// The caller has taken over publishing (an upload's second plugin pass).
    pub fn hand_over(mut self) {
        self.state = None;
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.take() {
            let archive_id = std::mem::take(&mut self.archive_id);
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move { publish(&state, &archive_id).await });
            }
        }
    }
}

pub(crate) async fn mark_pending_in(pool: &Pool, archive_id: &str) -> Result<(), VisibilityError> {
    let mut conn = pool.get().await?;
    let _: () = conn.sadd(PENDING_VISIBILITY_SET, archive_id).await?;
    tracing::info!(%archive_id, "ingest visibility: hidden until its metadata run finishes");
    Ok(())
}

pub(crate) async fn publish_in(pool: &Pool, archive_id: &str) {
    let mut last_error = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
        }
        let result: Result<(), VisibilityError> = async {
            let mut conn = pool.get().await?;
            let _: () = conn.srem(PENDING_VISIBILITY_SET, archive_id).await?;
            Ok(())
        }
        .await;
        match result {
            Ok(()) => {
                tracing::info!(%archive_id, "ingest visibility: published");
                return;
            }
            Err(e) => last_error = Some(e),
        }
    }
    if let Some(error) = last_error {
        tracing::error!(%archive_id, %error, "ingest visibility: publish failed; startup will publish it");
    }
}

pub(crate) async fn is_pending_in(pool: &Pool, archive_id: &str) -> bool {
    let result: Result<bool, VisibilityError> = async {
        let mut conn = pool.get().await?;
        Ok(conn.sismember(PENDING_VISIBILITY_SET, archive_id).await?)
    }
    .await;
    result.unwrap_or(true)
}

pub(crate) async fn pending_ids_in(pool: &Pool) -> HashSet<String> {
    let result: Result<HashSet<String>, VisibilityError> = async {
        let mut conn = pool.get().await?;
        Ok(conn.smembers(PENDING_VISIBILITY_SET).await?)
    }
    .await;
    result.unwrap_or_default()
}

pub(crate) async fn clear_all_pending_in(archive: &Pool, config: &Pool) -> usize {
    let mut published = 0;
    match archive.get().await {
        Ok(mut conn) => {
            let ids: Vec<String> = conn
                .smembers(PENDING_VISIBILITY_SET)
                .await
                .unwrap_or_default();
            published = ids.len();
            if let Err(error) = conn.del::<_, ()>(PENDING_VISIBILITY_SET).await {
                tracing::error!(%error, "ingest visibility: could not clear the pending set");
                published = 0;
            } else if !ids.is_empty() {
                tracing::info!(count = ids.len(), ids = ?ids, "ingest visibility: published archives a crash left hidden");
            }
        }
        Err(error) => {
            tracing::error!(%error, "ingest visibility: archive DB unavailable at startup")
        }
    }
    for pool in [archive, config] {
        delete_legacy_markers(pool).await;
    }
    published
}

/// The first implementation wrote one key per archive into the config DB; nothing ever read them.
async fn delete_legacy_markers(pool: &Pool) {
    let Ok(mut conn) = pool.get().await else {
        return;
    };
    let pattern = format!("{LEGACY_PENDING_VISIBILITY_PREFIX}*");
    let mut cursor: u64 = 0;
    loop {
        let step: Result<(u64, Vec<String>), _> = deadpool_redis::redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(500)
            .query_async(&mut conn)
            .await;
        let Ok((next, keys)) = step else {
            return;
        };
        if !keys.is_empty() {
            let _: Result<(), _> = conn.del(&keys).await;
        }
        if next == 0 {
            return;
        }
        cursor = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::test_support::{test_pool_for_url, RedisTestLock};

    /// The pending set is one shared key, so every test that touches it holds this lock (the
    /// search crate's tests take the same one).
    const LOCK: &str = "pending_visibility";

    async fn pool(db: u8) -> Option<Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        test_pool_for_url(&format!("{}/{db}", base.trim_end_matches('/'))).await
    }

    fn unreachable_pool() -> Pool {
        deadpool_redis::Config::from_url("redis://127.0.0.1:1/0")
            .create_pool(Some(deadpool_redis::Runtime::Tokio1))
            .unwrap()
    }

    fn new_id() -> String {
        format!("{}00000000", uuid::Uuid::new_v4().simple())
    }

    #[tokio::test]
    async fn mark_and_publish_round_trip() {
        let Some(archive) = pool(0).await else { return };
        let lock = RedisTestLock::acquire(&archive, LOCK).await;
        let a = new_id();
        mark_pending_in(&archive, &a).await.unwrap();
        assert!(is_pending_in(&archive, &a).await);
        assert!(pending_ids_in(&archive).await.contains(&a));
        publish_in(&archive, &a).await;
        assert!(!is_pending_in(&archive, &a).await);
        assert!(!pending_ids_in(&archive).await.contains(&a));
        lock.release().await;
    }

    #[tokio::test]
    async fn an_unreachable_redis_fails_closed() {
        let pool = unreachable_pool();
        assert!(mark_pending_in(&pool, &new_id()).await.is_err());
        assert!(is_pending_in(&pool, &new_id()).await);
        assert!(pending_ids_in(&pool).await.is_empty());
    }

    #[tokio::test]
    async fn startup_publishes_everything_and_drops_the_old_marker_keys() {
        let (Some(archive), Some(config)) = (pool(0).await, pool(2).await) else {
            return;
        };
        let lock = RedisTestLock::acquire(&archive, LOCK).await;
        let (a, b) = (new_id(), new_id());
        mark_pending_in(&archive, &a).await.unwrap();
        mark_pending_in(&archive, &b).await.unwrap();
        let legacy = format!("{LEGACY_PENDING_VISIBILITY_PREFIX}{}", new_id());
        for p in [&archive, &config] {
            let mut conn = p.get().await.unwrap();
            let _: () = conn.set(&legacy, "1").await.unwrap();
        }

        assert!(clear_all_pending_in(&archive, &config).await >= 2);

        assert!(pending_ids_in(&archive).await.is_empty());
        for p in [&archive, &config] {
            let mut conn = p.get().await.unwrap();
            let exists: bool = conn.exists(&legacy).await.unwrap();
            assert!(!exists, "legacy marker survived the sweep");
        }
        lock.release().await;
    }
}
