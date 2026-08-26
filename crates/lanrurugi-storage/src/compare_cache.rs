//! Redis persistence for `lanrurugi_imgcompare::ComparisonResult` (issue #77's AI quality-
//! comparison flow) — reported live: re-opening the same conflict's comparison re-ran the whole
//! perceptual-hash/DP-alignment pipeline from scratch every time ("每次打开都要等待"), even though
//! nothing about the two files being compared had changed since the last run.
//!
//! Also caches the *sample* pages' own raw image bytes ([`put_image`]/[`get_image`]), not just the
//! result JSON — the sample pages were originally re-read from each side's own on-disk file
//! (`temp_dir` for the staged download, the library file for the existing archive) on every view,
//! which meant a cache *hit* on the result could still show broken images if the staged file had
//! since been cleaned up (a real, reported gap: "现在这样的方式好吗？" — a dev-container rebuild
//! wipes `temp_dir` entirely, so the result cache alone made a stale-but-still-"successful"-looking
//! comparison look broken instead of just being a clean cache miss). Caching the actual sample
//! bytes alongside the result means a cache hit is always a *complete*, consistent view — never a
//! result with dead image links.
//!
//! Values are stored as opaque JSON strings/raw bytes, not `lanrurugi_imgcompare`'s own concrete
//! types — this crate sits *below* `lanrurugi-scanner` in the dependency graph, and
//! `lanrurugi-imgcompare` (which defines `ComparisonResult`) depends on `lanrurugi-scanner`, so a
//! direct type dependency here would be a cycle. `lanrurugi-api` (which depends on both) does the
//! typed serialize/deserialize at the call site, the same "this repository only knows bytes/JSON,
//! not the caller's own struct" shape `recommend_cache.rs`'s own Top-N storage already uses.
//!
//! On the **`config`** logical DB, same placement as `recommend_cache`/`plugin_options`/
//! `download_queue` — a new, purely additive namespace, not part of the legacy key surface.
//!
//! Capacity is explicitly bounded (confirmed design: "最多保存10组对比缓存，慢了后再添加就去旧留
//! 新") — a sorted set (`ORDER_KEY`) tracks insertion order by score, so `put` can cheaply evict
//! the single oldest entry (its result JSON *and* its images hash) once the count exceeds
//! [`MAX_ENTRIES`] without a full table scan. This is a *second*, independent safety net on top of
//! the primary cleanup path (deleting a cache entry when its queue item's conflict actually gets
//! resolved, in `lanrurugi-api`) — the cap matters if that primary cleanup is ever missed on some
//! code path, so the cache still can't grow unbounded even then. Only a handful of sample images
//! (3-5, per issue #77's own confirmed sample size) are ever cached per entry, each already a
//! decoded-and-recompressed manga page rather than a raw multi-MB archive, so the 10-entry cap
//! keeps total image storage bounded to a low tens-of-MB order of magnitude even in the worst case.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CompareCacheError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, CompareCacheError>;

/// Oldest-entries-evicted-first cap (confirmed design: "最多保存10组对比缓存") — small on purpose:
/// this exists to make re-opening a comparison the user *just* ran instant, not to be a general-
/// purpose long-term store (the primary lifecycle-based cleanup already handles the common case of
/// a resolved conflict; this cap only matters for the unusual case of many simultaneously-unresolved
/// conflicts, or a missed cleanup call).
const MAX_ENTRIES: isize = 10;

fn result_key(queue_item_id: &str) -> String {
    format!("LANRURUGI_COMPARE_RESULT_{queue_item_id}")
}

/// Hash of `"{side}_{page_index}" -> raw image bytes` for one queue item's cached sample pages —
/// a hash rather than one key per `(side, index)` pair so evicting/deleting a whole entry (`del`
/// below) is one Redis command instead of enumerating every sample's own key first.
fn images_key(queue_item_id: &str) -> String {
    format!("LANRURUGI_COMPARE_IMAGES_{queue_item_id}")
}

fn image_field(side: &str, page_index: usize) -> String {
    format!("{side}_{page_index}")
}

/// Sorted set of queue item ids, scored by insertion time (Unix seconds) — lets [`put`] find and
/// evict the single oldest entry in O(log n) rather than scanning every `LANRURUGI_COMPARE_RESULT_*`
/// key's own TTL/write-time.
const ORDER_KEY: &str = "LANRURUGI_COMPARE_RESULT_ORDER";

#[derive(Clone)]
pub struct CompareCacheRepository {
    pool: Pool,
}

impl CompareCacheRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, queue_item_id: &str) -> Result<Option<String>> {
        let mut conn = self.pool.get().await?;
        Ok(conn.get(result_key(queue_item_id)).await?)
    }

    /// One sample page's own raw image bytes, if cached — `None` on a plain cache miss (this
    /// queue item's comparison was never cached, or was already evicted/deleted), the same
    /// "degrade to re-reading from disk" signal [`get`]'s own caller already handles for the
    /// result JSON.
    pub async fn get_image(
        &self,
        queue_item_id: &str,
        side: &str,
        page_index: usize,
    ) -> Result<Option<Vec<u8>>> {
        let mut conn = self.pool.get().await?;
        Ok(conn
            .hget(images_key(queue_item_id), image_field(side, page_index))
            .await?)
    }

    /// Caches one sample page's own raw image bytes — called alongside [`put`] for every sample
    /// `ComparisonResult.samples` carries, so a cached result is always backed by a *complete* set
    /// of viewable images, never a result whose sample images silently 404 if the on-disk source
    /// file they'd otherwise be re-read from has since been cleaned up (see this module's own
    /// docs for the real gap this closes).
    pub async fn put_image(
        &self,
        queue_item_id: &str,
        side: &str,
        page_index: usize,
        bytes: &[u8],
    ) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn
            .hset(
                images_key(queue_item_id),
                image_field(side, page_index),
                bytes,
            )
            .await?;
        Ok(())
    }

    /// Stores `result_json` for `queue_item_id`, then evicts the single oldest entry if this put
    /// pushed the total past [`MAX_ENTRIES`]. Re-`put`-ting an id already present refreshes its
    /// position to "most recently inserted" (via `ZADD`'s own overwrite-on-existing-member
    /// behavior), so a cache that's merely being re-confirmed (not genuinely new) doesn't count as
    /// aging out anything else.
    pub async fn put(&self, queue_item_id: &str, result_json: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _: () = conn.set(result_key(queue_item_id), result_json).await?;
        let _: () = conn.zadd(ORDER_KEY, queue_item_id, now).await?;

        let count: isize = conn.zcard(ORDER_KEY).await?;
        if count > MAX_ENTRIES {
            let oldest: Vec<String> = conn.zrange(ORDER_KEY, 0, count - MAX_ENTRIES - 1).await?;
            for id in &oldest {
                let _: () = conn.del(result_key(id)).await?;
            }
            if !oldest.is_empty() {
                let _: () = conn.zrem(ORDER_KEY, &oldest).await?;
            }
        }
        Ok(())
    }

    /// Removes one entry outright — called when its queue item's conflict is actually resolved
    /// (overwrite/keep-b/rename) or the queue item itself is deleted, since a cached comparison
    /// for a conflict that no longer exists has nothing left to be re-opened against.
    pub async fn delete(&self, queue_item_id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(result_key(queue_item_id)).await?;
        let _: () = conn.zrem(ORDER_KEY, queue_item_id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        crate::test_support::test_pool().await
    }

    #[tokio::test]
    async fn round_trips_and_deletes_an_entry() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = CompareCacheRepository::new(pool);
        let id = "test-compare-cache-roundtrip";
        repo.delete(id).await.unwrap();

        assert_eq!(repo.get(id).await.unwrap(), None);
        repo.put(id, r#"{"aligned_pairs":3}"#).await.unwrap();
        assert_eq!(
            repo.get(id).await.unwrap(),
            Some(r#"{"aligned_pairs":3}"#.to_string())
        );

        repo.delete(id).await.unwrap();
        assert_eq!(repo.get(id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn evicts_the_oldest_entry_once_past_the_cap() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = CompareCacheRepository::new(pool);
        let ids: Vec<String> = (0..12)
            .map(|i| format!("test-compare-cache-eviction-{i}"))
            .collect();
        for id in &ids {
            repo.delete(id).await.unwrap();
        }

        for id in &ids {
            repo.put(id, "{}").await.unwrap();
        }

        // The first two inserted (oldest) should have been evicted once the 11th/12th pushed the
        // count past MAX_ENTRIES; the most recent MAX_ENTRIES should all still be present.
        assert_eq!(repo.get(&ids[0]).await.unwrap(), None);
        assert_eq!(repo.get(&ids[1]).await.unwrap(), None);
        for id in &ids[2..] {
            assert!(
                repo.get(id).await.unwrap().is_some(),
                "{id} should still be cached"
            );
        }

        for id in &ids {
            repo.delete(id).await.unwrap();
        }
    }
}
