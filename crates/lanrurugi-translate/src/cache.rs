//! The server-composited Translation Cache Entry (data-model.md, research.md §16/§18).
//!
//! **This is a re-derivable performance cache, not the authoritative record.** The authoritative
//! translation result is the persisted `DetectedTextRegion` set (text + position + resolved style),
//! which is orders of magnitude smaller than a rendered page image. An entry here may be evicted at
//! any time with no data loss: regenerating it re-runs *compositing only* — never OCR, never a
//! billed LLM call.
//!
//! **Quota (research.md §18)**: entries live under the reader's existing resize-page cache tree and
//! are therefore swept by its existing `tempmaxsize` mechanism
//! (`lanrurugi_api::download_manager::ingest::sweep_resize_cache_size`). No new setting, no second
//! sweep. Notably NOT the thumbnail cache, which has no quota or eviction at all — an earlier draft
//! of research.md said otherwise and was corrected once actually checked against the code.

use std::path::{Path, PathBuf};

use lanrurugi_core::ids::ArchiveId;
use lanrurugi_ocr::entities::PageNumber;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CacheError {
    #[error("failed to read/write the translation cache: {0}")]
    Io(#[from] std::io::Error),
}

/// Identifies one cached rendering. Every component matters: a change to any of them MUST NOT reuse
/// an entry keyed under a different combination (FR-016).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TranslationCacheKey {
    pub archive_id: ArchiveId,
    pub page_number: PageNumber,
    pub target_language: String,
    pub provider: String,
}

impl TranslationCacheKey {
    pub fn new(
        archive_id: ArchiveId,
        page_number: PageNumber,
        target_language: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            archive_id,
            page_number,
            target_language: target_language.into(),
            provider: provider.into(),
        }
    }

    /// Filename component encoding everything but the archive id.
    ///
    /// Language and provider are sanitized because both are user/config-supplied and would
    /// otherwise be able to escape the cache directory via `..` or a path separator.
    fn file_stem(&self) -> String {
        format!(
            "translated_{}_{}_{}",
            self.page_number.get(),
            sanitize(&self.target_language),
            sanitize(&self.provider),
        )
    }
}

/// Reduces an arbitrary string to a filename-safe token.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unknown".to_string()
    } else {
        cleaned
    }
}

/// Filesystem-backed cache for composited pages.
#[derive(Clone)]
pub struct TranslationImageCache {
    /// The reader's own resize-page cache root, so entries fall under the same `tempmaxsize` sweep.
    resize_cache_dir: PathBuf,
}

impl TranslationImageCache {
    /// `temp_dir` is the same `state.library.temp_dir` the reader's resize cache uses.
    pub fn new(temp_dir: &Path) -> Self {
        Self {
            resize_cache_dir: temp_dir.join("resize_page"),
        }
    }

    /// Path for a key. Mirrors the reader's `resize_page/<archive_id>/<file>.webp` layout so the
    /// existing sweep — which walks exactly that shape and matches `.webp` — picks these up with
    /// no changes of its own.
    pub fn path_for(&self, key: &TranslationCacheKey) -> PathBuf {
        self.resize_cache_dir
            .join(sanitize(key.archive_id.as_str()))
            .join(format!("{}.webp", key.file_stem()))
    }

    /// Reads a cached rendering, or `None` on a miss. A miss is always safe — the caller
    /// re-composites from the persisted regions.
    pub async fn get(&self, key: &TranslationCacheKey) -> Option<Vec<u8>> {
        tokio::fs::read(self.path_for(key)).await.ok()
    }

    pub async fn put(&self, key: &TranslationCacheKey, bytes: &[u8]) -> Result<(), CacheError> {
        let path = self.path_for(key);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        // Write-then-rename so a concurrent reader never observes a half-written image.
        let tmp = path.with_extension("webp.part");
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, &path).await?;
        Ok(())
    }

    /// Drops every cached rendering for one page across all languages/providers — used when a
    /// page's regions are re-translated and previous renderings are stale.
    pub async fn invalidate_page(&self, archive_id: &ArchiveId, page_number: PageNumber) {
        let dir = self.resize_cache_dir.join(sanitize(archive_id.as_str()));
        let prefix = format!("translated_{}_", page_number.get());

        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            return;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name.ends_with(".webp") {
                let _ = tokio::fs::remove_file(entry.path()).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(page: u32, lang: &str, provider: &str) -> TranslationCacheKey {
        TranslationCacheKey::new(ArchiveId::from("abc123"), PageNumber(page), lang, provider)
    }

    #[test]
    fn each_key_component_produces_a_distinct_path() {
        let cache = TranslationImageCache::new(Path::new("/tmp/lrr"));
        let base = cache.path_for(&key(1, "en", "deepseek"));

        // FR-016: changing any component must not reuse another combination's entry.
        assert_ne!(base, cache.path_for(&key(2, "en", "deepseek")));
        assert_ne!(base, cache.path_for(&key(1, "fr", "deepseek")));
        assert_ne!(base, cache.path_for(&key(1, "en", "anthropic")));
    }

    #[test]
    fn entries_live_under_the_resize_cache_so_the_existing_sweep_finds_them() {
        let cache = TranslationImageCache::new(Path::new("/tmp/lrr"));
        let path = cache.path_for(&key(1, "en", "deepseek"));

        assert!(path.starts_with("/tmp/lrr/resize_page"));
        assert_eq!(
            path.extension().and_then(|e| e.to_str()),
            Some("webp"),
            "the sweep only matches .webp files"
        );
    }

    #[test]
    fn path_traversal_in_a_key_component_is_neutralized() {
        let cache = TranslationImageCache::new(Path::new("/tmp/lrr"));
        let evil = TranslationCacheKey::new(
            ArchiveId::from("../../etc"),
            PageNumber(1),
            "../../../etc/passwd",
            "p",
        );
        let path = cache.path_for(&evil);
        assert!(path.starts_with("/tmp/lrr/resize_page"));
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn empty_key_components_still_produce_a_usable_path() {
        let cache = TranslationImageCache::new(Path::new("/tmp/lrr"));
        let path = cache.path_for(&TranslationCacheKey::new(
            ArchiveId::from("abc"),
            PageNumber(1),
            "",
            "",
        ));
        assert!(path.to_string_lossy().contains("unknown"));
    }

    #[tokio::test]
    async fn a_miss_is_not_an_error() {
        let cache = TranslationImageCache::new(&std::env::temp_dir().join("lrr-cache-miss-test"));
        assert!(cache.get(&key(99, "en", "deepseek")).await.is_none());
    }

    #[tokio::test]
    async fn stored_bytes_round_trip() {
        let dir = std::env::temp_dir().join("lrr-cache-roundtrip-test");
        let cache = TranslationImageCache::new(&dir);
        let k = key(1, "en", "deepseek");

        cache.put(&k, b"fake-webp-bytes").await.unwrap();
        assert_eq!(
            cache.get(&k).await.as_deref(),
            Some(&b"fake-webp-bytes"[..])
        );

        cache.invalidate_page(&k.archive_id, k.page_number).await;
        assert!(
            cache.get(&k).await.is_none(),
            "invalidation must remove the entry"
        );

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
