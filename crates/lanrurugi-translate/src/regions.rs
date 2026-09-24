//! Redis persistence for `DetectedTextRegion` — the **authoritative** translation record
//! (research.md §16, data-model.md).
//!
//! This reverses the original design, where the rendered image was durable and this data
//! disposable. A JSON record of text + position + style is orders of magnitude smaller than a
//! rendered page image, so it is the cheaper thing to keep indefinitely — and the more valuable:
//! losing the rendered image costs one compositing pass, while losing this would mean re-running
//! OCR and paying for a fresh LLM translation.
//!
//! Keyed by (archive, page, target language, provider) mirroring `TranslationCacheKey`, because a
//! different language or backend genuinely produces different `translated_text` for the same
//! detected geometry.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use lanrurugi_core::ids::ArchiveId;
use lanrurugi_ocr::entities::{DetectedTextRegion, PageNumber};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RegionStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, RegionStorageError>;

/// Regions before translation — detection output is language/provider independent, so it is stored
/// once and reused across every language and backend rather than being re-run per combination.
fn detection_key(archive_id: &ArchiveId, page: PageNumber) -> String {
    format!(
        "LRR_TRANSLATION_REGIONS_{}_{}",
        archive_id.as_str(),
        page.get()
    )
}

/// Regions after translation, per (language, provider).
fn translation_key(
    archive_id: &ArchiveId,
    page: PageNumber,
    target_language: &str,
    provider: &str,
) -> String {
    // BCP-47 tags are case-insensitive, but Redis keys are not. The SPA sends `zh-CN` while the
    // backend/tests have historically used `zh-cn`, which split one translation record into two
    // (`..._zh-CN_...` held a stale A candidate; `..._zh-cn_...` held the corrected B candidate)
    // while the image-cache filename's own `sanitize()` already lowercases — so whichever request
    // arrived first rendered the shared file and the other case could never refresh it. Canonicalize
    // the language component here, at the single persistence boundary every reader/writer crosses.
    // Provider ids are already canonical lowercase enums; leaving them untouched keeps the key
    // migration surface limited to the actual bug.
    let target_language = target_language.trim().to_ascii_lowercase();
    format!(
        "LRR_TRANSLATION_TEXT_{}_{}_{}_{}",
        archive_id.as_str(),
        page.get(),
        target_language,
        provider
    )
}

#[derive(Clone)]
pub struct RegionRepository {
    pool: Pool,
}

impl RegionRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Cached OCR output for a page, if it has been detected before.
    pub async fn get_detected(
        &self,
        archive_id: &ArchiveId,
        page: PageNumber,
    ) -> Result<Option<Vec<DetectedTextRegion>>> {
        let key = detection_key(archive_id, page);
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.get(&key).await?;

        raw.map(|json| {
            serde_json::from_str(&json).map_err(|e| RegionStorageError::Json(key.clone(), e))
        })
        .transpose()
    }

    pub async fn save_detected(
        &self,
        archive_id: &ArchiveId,
        page: PageNumber,
        regions: &[DetectedTextRegion],
    ) -> Result<()> {
        let key = detection_key(archive_id, page);
        let json =
            serde_json::to_string(regions).map_err(|e| RegionStorageError::Json(key.clone(), e))?;
        let mut conn = self.pool.get().await?;
        let _: () = conn.set(&key, json).await?;
        Ok(())
    }

    /// Translated regions for a specific (language, provider) — the authoritative result.
    pub async fn get_translated(
        &self,
        archive_id: &ArchiveId,
        page: PageNumber,
        target_language: &str,
        provider: &str,
    ) -> Result<Option<Vec<DetectedTextRegion>>> {
        let key = translation_key(archive_id, page, target_language, provider);
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.get(&key).await?;

        raw.map(|json| {
            serde_json::from_str(&json).map_err(|e| RegionStorageError::Json(key.clone(), e))
        })
        .transpose()
    }

    pub async fn save_translated(
        &self,
        archive_id: &ArchiveId,
        page: PageNumber,
        target_language: &str,
        provider: &str,
        regions: &[DetectedTextRegion],
    ) -> Result<()> {
        let key = translation_key(archive_id, page, target_language, provider);
        let json =
            serde_json::to_string(regions).map_err(|e| RegionStorageError::Json(key.clone(), e))?;
        let mut conn = self.pool.get().await?;
        let _: () = conn.set(&key, json).await?;
        Ok(())
    }

    /// Every translated region set — used by backup/export (FR-022).
    pub async fn list_all_translated(&self) -> Result<Vec<(String, Vec<DetectedTextRegion>)>> {
        let mut conn = self.pool.get().await?;
        let keys: Vec<String> = conn.keys("LRR_TRANSLATION_TEXT_*").await?;

        let mut out = Vec::with_capacity(keys.len());
        for key in keys {
            let raw: Option<String> = conn.get(&key).await?;
            if let Some(json) = raw {
                match serde_json::from_str(&json) {
                    Ok(regions) => out.push((key, regions)),
                    Err(e) => {
                        tracing::warn!(key = %key, error = %e, "skipping malformed region record")
                    }
                }
            }
        }
        Ok(out)
    }

    /// Restores a region set from a backup (FR-022), by its original key.
    pub async fn restore_raw(&self, key: &str, regions: &[DetectedTextRegion]) -> Result<()> {
        let json =
            serde_json::to_string(regions).map_err(|e| RegionStorageError::Json(key.into(), e))?;
        let mut conn = self.pool.get().await?;
        let _: () = conn.set(key, json).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_is_keyed_independently_of_language_and_provider() {
        // Detection geometry doesn't change with language, so it must not be re-run per language.
        let a = ArchiveId::from("abc");
        assert_eq!(
            detection_key(&a, PageNumber(1)),
            detection_key(&a, PageNumber(1))
        );
        assert_ne!(
            detection_key(&a, PageNumber(1)),
            detection_key(&a, PageNumber(2))
        );
    }

    #[test]
    fn translations_are_keyed_by_language_and_provider() {
        let a = ArchiveId::from("abc");
        let base = translation_key(&a, PageNumber(1), "en", "deepseek");
        assert_ne!(base, translation_key(&a, PageNumber(1), "fr", "deepseek"));
        assert_ne!(base, translation_key(&a, PageNumber(1), "en", "anthropic"));
    }

    #[test]
    fn language_tags_are_keyed_case_insensitively() {
        // Regression: frontend `zh-CN` vs backend `zh-cn` must address the same persisted record,
        // otherwise one case can silently keep serving a stale OCR/translation candidate forever.
        let a = ArchiveId::from("abc");
        assert_eq!(
            translation_key(&a, PageNumber(1), "zh-CN", "deepseek"),
            translation_key(&a, PageNumber(1), "zh-cn", "deepseek")
        );
        assert_eq!(
            translation_key(&a, PageNumber(1), " zh-CN ", "deepseek"),
            translation_key(&a, PageNumber(1), "zh-cn", "deepseek")
        );
    }

    #[test]
    fn keys_use_this_features_own_namespace() {
        let a = ArchiveId::from("abc");
        assert!(detection_key(&a, PageNumber(1)).starts_with("LRR_TRANSLATION_"));
        assert!(translation_key(&a, PageNumber(1), "en", "x").starts_with("LRR_TRANSLATION_"));
    }
}
