//! Redis persistence for the reader recommendation engine's precomputed embedding vectors and
//! per-archive Top-N similar-archive lists (issue #70's performance fix — the on-demand ONNX
//! embedding + O(n) cosine ranking previously done inside the request path, at 3s/1k-archives up
//! to a multi-minute timeout at 100k, is replaced by writing these at catalogue/title-change time
//! instead).
//!
//! A new, purely additive Redis namespace on the **`config`** logical DB — no legacy key shape
//! touched, and deliberately *not* the `archive` DB: that DB is glob-scanned by both this crate's
//! own `ARCHIVE_KEY_GLOB` (`repository.rs`, 40 lowercase-hex chars) and legacy's own `$redis->keys`
//! call, so a key on DB 0 whose suffix happened to look archive-id-shaped would be silently fed
//! into archive deserialization as a bogus record. These keys live on `config` instead, alongside
//! the other non-legacy additive namespaces (`plugin_options`, `download_queue`).
//!
//! Vectors are stored as raw little-endian `f32` bytes, not JSON — a JSON number array runs
//! ~10-12 bytes/float (`0.05283419,`) vs. 4 raw bytes, the difference between ~430MB and ~154MB
//! at 100k archives for a 384-dim embedding. Top-N lists store archive-id strings only (no
//! score) — rank is expressed by list order, halving storage again vs. a scored format, since
//! nothing downstream needs the raw cosine value once the list is built (the LLM rerank step
//! computes its own ordering from title text, not from a persisted score).

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecommendCacheError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed vector bytes for key {0:?}: {1}")]
    Codec(String, String),
    #[error("malformed Top-N JSON in key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, RecommendCacheError>;

fn vector_key(archive_id: &str) -> String {
    format!("LANRURUGI_RECOMMEND_VECTOR_{archive_id}")
}

fn topn_key(archive_id: &str) -> String {
    format!("LANRURUGI_RECOMMEND_TOPN_{archive_id}")
}

/// Single fixed key (not per-archive) — no legacy counterpart, holds precompute bookkeeping:
/// the active precision tier, a generation counter bumped on every tier change (so a full
/// rebuild-in-progress can tell "already-current-generation" Top-N entries from stale ones left
/// over from the previous tier, making the rebuild resumable across a restart), and the one-time
/// post-upgrade backfill's completed-version marker.
const META_KEY: &str = "LANRURUGI_RECOMMEND_META";

/// Hash of `archive_id -> rebuild_generation (as a string)` — separate from the Top-N value
/// itself (which stays a plain ID-list JSON array, [`RecommendCacheRepository::get_topn`]'s
/// existing contract that `lanrurugi-api`'s incremental `precompute_one` path relies on) so the
/// full-rebuild job's resumability check (`lanrurugi-api::recommend_precompute::
/// spawn_full_precompute_job`) is one extra `HGET` rather than a value-shape change threading a
/// generation number through every Top-N read/write call site.
const TOPN_GENERATION_KEY: &str = "LANRURUGI_RECOMMEND_TOPN_GENERATION";

/// `<u32 LE: title byte length><title utf8 bytes><embedding f32s, 4 bytes each, LE>`. The title
/// is embedded alongside the vector (not a separate key) so a "did this archive's title change
/// since we last embedded it" check never needs a second round trip.
fn encode_vector(title: &str, vector: &[f32]) -> Vec<u8> {
    let title_bytes = title.as_bytes();
    let mut out = Vec::with_capacity(4 + title_bytes.len() + vector.len() * 4);
    out.extend_from_slice(&(title_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(title_bytes);
    for f in vector {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

fn decode_vector(key: &str, bytes: &[u8]) -> Result<(String, Vec<f32>)> {
    if bytes.len() < 4 {
        return Err(RecommendCacheError::Codec(
            key.to_string(),
            "buffer shorter than the 4-byte title-length header".to_string(),
        ));
    }
    let title_len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
    let title_end = 4 + title_len;
    if bytes.len() < title_end {
        return Err(RecommendCacheError::Codec(
            key.to_string(),
            format!("title length {title_len} exceeds buffer"),
        ));
    }
    let title = std::str::from_utf8(&bytes[4..title_end])
        .map_err(|e| RecommendCacheError::Codec(key.to_string(), e.to_string()))?
        .to_string();
    let vector_bytes = &bytes[title_end..];
    if !vector_bytes.len().is_multiple_of(4) {
        return Err(RecommendCacheError::Codec(
            key.to_string(),
            format!(
                "trailing vector byte length {} is not a multiple of 4",
                vector_bytes.len()
            ),
        ));
    }
    let vector: Vec<f32> = vector_bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Ok((title, vector))
}

#[derive(Clone)]
pub struct RecommendCacheRepository {
    pool: Pool,
}

impl RecommendCacheRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get_vector(&self, archive_id: &str) -> Result<Option<(String, Vec<f32>)>> {
        let mut conn = self.pool.get().await?;
        let key = vector_key(archive_id);
        let raw: Option<Vec<u8>> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(bytes) => decode_vector(&key, &bytes).map(Some),
        }
    }

    pub async fn put_vector(&self, archive_id: &str, title: &str, vector: &[f32]) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = vector_key(archive_id);
        let bytes = encode_vector(title, vector);
        let _: () = conn.set(&key, bytes).await?;
        Ok(())
    }

    /// Bulk-loads every persisted vector. Uses `SCAN` (paginated, `COUNT 1000` per round trip),
    /// not `KEYS` — unlike `repository.rs`'s `list_all_by_glob` (which scans the legacy-shaped
    /// `archive` DB glob with `KEYS`, matching legacy's own blocking behavior there), this is a
    /// from-scratch namespace with no legacy precedent to match, and a `KEYS
    /// LANRURUGI_RECOMMEND_VECTOR_*` over 100k entries would block Redis's single command thread
    /// for a user-visible window. Don't "fix" this back to `KEYS` to match `list_all_by_glob`'s
    /// style — the two have different blocking-cost profiles at this table's actual scale.
    pub async fn get_vectors_all(&self) -> Result<Vec<(String, String, Vec<f32>)>> {
        let mut conn = self.pool.get().await?;
        let prefix = "LANRURUGI_RECOMMEND_VECTOR_";
        let mut cursor: u64 = 0;
        let mut keys: Vec<String> = Vec::new();
        loop {
            let (next_cursor, batch): (u64, Vec<String>) = deadpool_redis::redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(format!("{prefix}*"))
                .arg("COUNT")
                .arg(1000)
                .query_async(&mut conn)
                .await?;
            keys.extend(batch);
            cursor = next_cursor;
            if cursor == 0 {
                break;
            }
        }

        let mut out = Vec::with_capacity(keys.len());
        for chunk in keys.chunks(500) {
            let values: Vec<Option<Vec<u8>>> = conn.mget(chunk).await?;
            for (key, raw) in chunk.iter().zip(values) {
                let Some(bytes) = raw else { continue };
                let (title, vector) = decode_vector(key, &bytes)?;
                let archive_id = key.strip_prefix(prefix).unwrap_or(key.as_str()).to_string();
                out.push((archive_id, title, vector));
            }
        }
        Ok(out)
    }

    pub async fn get_topn(&self, archive_id: &str) -> Result<Option<Vec<String>>> {
        let mut conn = self.pool.get().await?;
        let key = topn_key(archive_id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| RecommendCacheError::Json(key, e)),
        }
    }

    pub async fn put_topn(&self, archive_id: &str, ids: &[String]) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = topn_key(archive_id);
        let raw =
            serde_json::to_string(ids).map_err(|e| RecommendCacheError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        Ok(())
    }

    /// Removes both the vector and the Top-N list for a deleted archive. Deliberately does
    /// **not** scrub this id out of every *other* archive's cached Top-N list — those become
    /// dangling references, handled cheaply on the read side instead (`recommend.rs`'s
    /// hydration step already does an `archives.get()` per cached id, so a dangling id simply
    /// yields `None` and gets filtered out there, at zero extra cost on the write side).
    pub async fn delete_for(&self, archive_id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(vector_key(archive_id)).await?;
        let _: () = conn.del(topn_key(archive_id)).await?;
        let _: () = conn.hdel(TOPN_GENERATION_KEY, archive_id).await?;
        Ok(())
    }

    pub async fn get_meta_tier(&self) -> Result<Option<String>> {
        let mut conn = self.pool.get().await?;
        Ok(conn.hget(META_KEY, "tier").await?)
    }

    pub async fn put_meta_tier(&self, tier: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hset(META_KEY, "tier", tier).await?;
        Ok(())
    }

    /// Which `rebuild_generation` this archive's currently-cached Top-N list was computed under —
    /// `None` if never recorded (an archive whose Top-N was only ever written by the incremental
    /// `precompute_one` path, which doesn't tag a generation at all).
    pub async fn get_topn_generation(&self, archive_id: &str) -> Result<Option<u64>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.hget(TOPN_GENERATION_KEY, archive_id).await?;
        Ok(raw.and_then(|s| s.parse().ok()))
    }

    pub async fn put_topn_generation(&self, archive_id: &str, generation: u64) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn
            .hset(TOPN_GENERATION_KEY, archive_id, generation.to_string())
            .await?;
        Ok(())
    }

    pub async fn get_rebuild_generation(&self) -> Result<u64> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.hget(META_KEY, "rebuild_generation").await?;
        Ok(raw.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    pub async fn bump_rebuild_generation(&self) -> Result<u64> {
        let mut conn = self.pool.get().await?;
        let next: u64 = conn.hincr(META_KEY, "rebuild_generation", 1).await?;
        Ok(next)
    }

    pub async fn get_backfill_version(&self) -> Result<u32> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.hget(META_KEY, "backfill_version").await?;
        Ok(raw.and_then(|s| s.parse().ok()).unwrap_or(0))
    }

    pub async fn put_backfill_version(&self, version: u32) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hset(META_KEY, "backfill_version", version).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_codec_round_trips_bit_exact_with_multibyte_title() {
        let title = "架空アンソロジー 銀花猫 café";
        let vector: Vec<f32> = (0..384).map(|i| (i as f32) * 0.01234 - 1.5).collect();
        let encoded = encode_vector(title, &vector);
        let (decoded_title, decoded_vector) = decode_vector("test-key", &encoded).unwrap();
        assert_eq!(decoded_title, title);
        assert_eq!(decoded_vector, vector);
    }

    #[test]
    fn vector_codec_rejects_truncated_buffer() {
        let err = decode_vector("test-key", &[1, 2]).unwrap_err();
        assert!(matches!(err, RecommendCacheError::Codec(_, _)));
    }

    #[test]
    fn vector_codec_rejects_misaligned_trailing_bytes() {
        let mut encoded = encode_vector("x", &[1.0, 2.0]);
        encoded.push(0); // one stray byte breaks the 4-byte float alignment
        let err = decode_vector("test-key", &encoded).unwrap_err();
        assert!(matches!(err, RecommendCacheError::Codec(_, _)));
    }

    async fn test_pool() -> Option<Pool> {
        let url = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let cfg = deadpool_redis::Config::from_url(url);
        cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1)).ok()
    }

    #[tokio::test]
    async fn round_trips_a_vector_and_a_topn_list() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RecommendCacheRepository::new(pool);
        let archive_id = "test-recommend-cache-roundtrip";

        assert_eq!(repo.get_vector(archive_id).await.unwrap(), None);
        assert_eq!(repo.get_topn(archive_id).await.unwrap(), None);

        let vector: Vec<f32> = (0..384).map(|i| i as f32).collect();
        repo.put_vector(archive_id, "Test Title", &vector)
            .await
            .unwrap();
        assert_eq!(
            repo.get_vector(archive_id).await.unwrap(),
            Some(("Test Title".to_string(), vector))
        );

        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        repo.put_topn(archive_id, &ids).await.unwrap();
        assert_eq!(repo.get_topn(archive_id).await.unwrap(), Some(ids));

        repo.delete_for(archive_id).await.unwrap();
        assert_eq!(repo.get_vector(archive_id).await.unwrap(), None);
        assert_eq!(repo.get_topn(archive_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn topn_generation_round_trips_and_is_cleared_by_delete_for() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RecommendCacheRepository::new(pool);
        let archive_id = "test-recommend-cache-topn-generation";

        assert_eq!(repo.get_topn_generation(archive_id).await.unwrap(), None);
        repo.put_topn_generation(archive_id, 7).await.unwrap();
        assert_eq!(repo.get_topn_generation(archive_id).await.unwrap(), Some(7));

        repo.delete_for(archive_id).await.unwrap();
        assert_eq!(repo.get_topn_generation(archive_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn meta_tier_and_generation_and_backfill_version_round_trip() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RecommendCacheRepository::new(pool);

        repo.put_meta_tier("high").await.unwrap();
        assert_eq!(
            repo.get_meta_tier().await.unwrap(),
            Some("high".to_string())
        );

        let gen_before = repo.get_rebuild_generation().await.unwrap();
        let gen_after = repo.bump_rebuild_generation().await.unwrap();
        assert_eq!(gen_after, gen_before + 1);

        repo.put_backfill_version(3).await.unwrap();
        assert_eq!(repo.get_backfill_version().await.unwrap(), 3);
    }
}
