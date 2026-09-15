//! Redis persistence for [`VolumeFontPattern`].
//!
//! Additive key namespace only (`LRR_TRANSLATION_FONTPATTERN_*`) — nothing here reads or writes a
//! Phase 1 archive/category/tankoubon key (constitution Principle I).

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_ocr::entities::VolumeId;
use thiserror::Error;

use crate::entities::{font_pattern_key, VolumeFontPattern};

#[derive(Debug, Error)]
pub enum FontPatternStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, FontPatternStorageError>;

#[derive(Clone)]
pub struct FontPatternRepository {
    pool: Pool,
}

impl FontPatternRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Loads a volume's pattern, or a fresh unlocked one if it has never been established.
    pub async fn get(&self, volume_id: &VolumeId) -> Result<VolumeFontPattern> {
        let key = font_pattern_key(volume_id);
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.get(&key).await?;

        match raw {
            Some(json) => serde_json::from_str(&json)
                .map_err(|e| FontPatternStorageError::Json(key.clone(), e)),
            None => Ok(VolumeFontPattern::new(volume_id)),
        }
    }

    pub async fn save(&self, pattern: &VolumeFontPattern) -> Result<()> {
        let key = font_pattern_key(&VolumeId::from(pattern.volume_id.clone()));
        let json = serde_json::to_string(pattern)
            .map_err(|e| FontPatternStorageError::Json(key.clone(), e))?;
        let mut conn = self.pool.get().await?;
        let _: () = conn.set(&key, json).await?;
        Ok(())
    }

    /// Clears a volume's pattern back to unlocked (FR-010).
    pub async fn reset(&self, volume_id: &VolumeId) -> Result<VolumeFontPattern> {
        let fresh = VolumeFontPattern::new(volume_id);
        self.save(&fresh).await?;
        Ok(fresh)
    }

    /// Every stored pattern — used by backup/export (FR-022).
    pub async fn list_all(&self) -> Result<Vec<VolumeFontPattern>> {
        let mut conn = self.pool.get().await?;
        let keys: Vec<String> = conn.keys("LRR_TRANSLATION_FONTPATTERN_*").await?;

        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let raw: Option<String> = conn.get(&key).await?;
            if let Some(json) = raw {
                match serde_json::from_str(&json) {
                    Ok(p) => out.push(p),
                    // A single malformed record must not abort a whole backup.
                    Err(e) => {
                        tracing::warn!(key = %key, error = %e, "skipping malformed font pattern")
                    }
                }
            }
        }
        Ok(out)
    }
}
