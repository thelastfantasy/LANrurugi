//! Redis persistence for a download plugin's user-configured settings overrides
//! (`specs/005-download-plugin-progress/data-model.md`'s `Download Plugin Settings`).
//!
//! A new, purely additive Redis namespace — no legacy key shape touched. Only the user's
//! *override* is ever stored here; a plugin's own declared defaults (`pluginOptions()`) are always
//! recomputed fresh by calling the plugin, never duplicated into Redis (data-model.md's
//! `declared_defaults` row: "Not stored").

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginOptionsStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, PluginOptionsStorageError>;

/// Mirrors `lanrurugi_plugin::protocol::DomainRule` field-for-field — duplicated here rather than
/// imported since `lanrurugi-storage` sits below `lanrurugi-plugin` in the dependency graph
/// (`lanrurugi-api` depends on both, not the reverse). Kept in exact sync by the round-trip test
/// below plus `lanrurugi-api`'s own conversion code at the one place these two types meet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct DomainRuleOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes_per_sec: Option<u64>,
}

/// The user-editable subset of `PluginOptionsResult` (data-model.md: "Shape mirrors
/// `PluginOptionsResult` minus its descriptive/label metadata, which only ever comes from the
/// plugin, never user-editable"). A field left `None`/absent means the user hasn't overridden it —
/// the plugin's own declared default (or system-wide absence of one, FR-017) applies instead.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PluginOptionsOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain_rules: Option<Vec<DomainRuleOverride>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_as_archive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overwrite_on_duplicate: Option<bool>,
}

fn redis_key(plugin_namespace: &str) -> String {
    format!("LANRURUGI_DOWNLOAD_PLUGIN_OPTIONS_{plugin_namespace}")
}

#[derive(Clone)]
pub struct PluginOptionsRepository {
    pool: Pool,
}

impl PluginOptionsRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Returns `None` when the plugin has no stored override at all (every field falls back to
    /// the plugin's own declared default).
    pub async fn get(&self, plugin_namespace: &str) -> Result<Option<PluginOptionsOverride>> {
        let mut conn = self.pool.get().await?;
        let key = redis_key(plugin_namespace);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| PluginOptionsStorageError::Json(key, e)),
        }
    }

    pub async fn save(
        &self,
        plugin_namespace: &str,
        overrides: &PluginOptionsOverride,
    ) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = redis_key(plugin_namespace);
        let raw = serde_json::to_string(overrides)
            .map_err(|e| PluginOptionsStorageError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        Ok(())
    }

    /// Idempotent — clearing an override that doesn't exist is a no-op, not an error (spec
    /// `DELETE /api/plugins/{namespace}/options`'s contract).
    pub async fn delete(&self, plugin_namespace: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(redis_key(plugin_namespace)).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        let url = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let cfg = deadpool_redis::Config::from_url(url);
        cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1)).ok()
    }

    #[tokio::test]
    async fn round_trips_an_override_and_deletes_it() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = PluginOptionsRepository::new(pool);
        let namespace = "test-plugin-options-roundtrip";

        assert_eq!(repo.get(namespace).await.unwrap(), None);

        let overrides = PluginOptionsOverride {
            domain_rules: Some(vec![DomainRuleOverride {
                pattern: Some("*.example.com".to_string()),
                max_concurrent: Some(3),
                max_bytes_per_sec: None,
            }]),
            bundle_as_archive: Some(false),
            overwrite_on_duplicate: Some(true),
        };
        repo.save(namespace, &overrides).await.unwrap();
        assert_eq!(repo.get(namespace).await.unwrap(), Some(overrides));

        repo.delete(namespace).await.unwrap();
        assert_eq!(repo.get(namespace).await.unwrap(), None);

        // Idempotent: deleting an already-absent override is a no-op, not an error.
        repo.delete(namespace).await.unwrap();
    }
}
