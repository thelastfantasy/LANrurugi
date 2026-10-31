//! Redis persistence for Archive split suggestions.
//!
//! This stores the LLM-generated split/repackage recommendation for a single archive. It is
//! deliberately **not** automatically executed: the suggestion is saved for the reader page to
//! show, and the user explicitly starts the background split job.
//!
//! Storage is on the `config` logical DB, same placement as other additive non-legacy stores
//! (`plugin_options`, `download_queue`, `compare_cache`, ...). Keys are `archive_id -> JSON`.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ArchiveSplitSuggestionsError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, ArchiveSplitSuggestionsError>;

const HASH_KEY: &str = "LANRURUGI_ARCHIVE_SPLIT_SUGGESTIONS";

/// One output ZIP group.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SplitGroupSuggestion {
    /// Suggested filename, e.g. `"Series_Vol_1.zip"`. The executor may append a CRC32 before the
    /// extension if a same-directory collision occurs.
    pub zip_name: String,
    pub description: String,
    /// Directory paths inside the original archive whose entire non-directory subtree should go
    /// into this ZIP.
    #[serde(default)]
    pub source_dirs: Vec<String>,
    /// Individual files inside the original archive (usually top-level files not under any
    /// assigned directory) that should go into this ZIP.
    #[serde(default)]
    pub source_files: Vec<String>,
}

/// The complete saved split suggestion for one archive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArchiveSplitSuggestion {
    pub archive_id: String,
    pub suggestion_version: u32,
    pub created_at: u64,
    pub split_groups: Vec<SplitGroupSuggestion>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

#[derive(Clone)]
pub struct ArchiveSplitSuggestionsRepository {
    pool: Pool,
}

impl ArchiveSplitSuggestionsRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn save(&self, suggestion: &ArchiveSplitSuggestion) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let raw = serde_json::to_string(suggestion)
            .expect("ArchiveSplitSuggestion is always serializable");
        let _: () = conn.hset(HASH_KEY, &suggestion.archive_id, raw).await?;
        Ok(())
    }

    pub async fn get(&self, archive_id: &str) -> Result<Option<ArchiveSplitSuggestion>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.hget(HASH_KEY, archive_id).await?;
        Ok(raw.and_then(|v| serde_json::from_str(&v).ok()))
    }

    pub async fn delete(&self, archive_id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hdel(HASH_KEY, archive_id).await?;
        Ok(())
    }

    pub async fn list_all(&self) -> Result<Vec<ArchiveSplitSuggestion>> {
        let mut conn = self.pool.get().await?;
        let raw: HashMap<String, String> = conn.hgetall(HASH_KEY).await?;
        Ok(raw
            .into_values()
            .filter_map(|v| serde_json::from_str(&v).ok())
            .collect())
    }
}

type HashMap<K, V> = std::collections::HashMap<K, V>;
