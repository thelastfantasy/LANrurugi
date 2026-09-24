//! Redis persistence for AI Tankoubon-grouping suggestions the user explicitly dismissed
//! ("Don't suggest this again") — `tankoubon_grouping.rs`'s `ai_group_suggestions` endpoint reads
//! this to skip a previously-ignored exact combination by default, and the frontend's own "Show
//! ignored combinations" checkbox surfaces them again on request.
//!
//! A new, purely additive Redis namespace on the **`config`** logical DB, alongside
//! `plugin_options`/`download_queue`/`recommend_cache` — see `recommend_cache.rs`'s own docs for
//! why `config` specifically (not `archive`, which is glob-scanned for archive records).
//!
//! Match granularity is intentionally exact, not fuzzy: an ignored entry is keyed by the *precise*
//! set of archive ids in that suggestion (plus, for an "add to existing Tankoubon" suggestion, that
//! Tankoubon's own id), sorted and hashed. If a future re-run's candidate set for the same anchor
//! differs by even one member (a new archive got ingested, an existing one got tagged
//! differently and dropped out), it's treated as a genuinely different suggestion and shown again —
//! deliberately conservative, so ignoring one combination can never silently suppress an
//! unrelated-but-overlapping future suggestion the user never actually saw and dismissed.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IgnoredGroupSuggestionsError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, IgnoredGroupSuggestionsError>;

const HASH_KEY: &str = "LANRURUGI_AI_GROUP_IGNORED";

/// Deterministic fingerprint for one suggestion's exact combination — sorted so member order
/// (which `ai_group_suggestions` doesn't guarantee is stable across requests) never affects the
/// key, and `existing_tankoubon_id` folded in so the same archive set suggested as a brand-new
/// group vs. as an addition to a specific existing Tankoubon are treated as different suggestions
/// (ignoring one must not silently suppress the other).
pub fn fingerprint(archive_ids: &[String], existing_tankoubon_id: Option<&str>) -> String {
    let mut sorted: Vec<&str> = archive_ids.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let mut hasher = DefaultHasher::new();
    existing_tankoubon_id.unwrap_or("").hash(&mut hasher);
    for id in &sorted {
        id.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// One ignored suggestion, as stored/returned — kept as the original (unsorted, human-readable)
/// `archive_ids` rather than re-deriving them from the fingerprint (a hash isn't reversible), so
/// the frontend's "Show ignored combinations" list can render real titles without a second lookup
/// against `ai_group_suggestions`' own current output (which may no longer even contain this exact
/// combination once ignored).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct IgnoredGroupSuggestion {
    pub archive_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub existing_tankoubon_id: Option<String>,
    pub ignored_at: u64,
}

#[derive(Clone)]
pub struct IgnoredGroupSuggestionsRepository {
    pool: Pool,
}

impl IgnoredGroupSuggestionsRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// All currently-ignored suggestions — malformed individual entries (should never happen
    /// outside manual Redis tampering) are skipped rather than failing the whole read, since one
    /// bad entry blocking every other ignored suggestion from ever being shown/un-ignored would be
    /// a worse failure mode than just dropping that one stale entry.
    pub async fn list_all(&self) -> Result<Vec<IgnoredGroupSuggestion>> {
        let mut conn = self.pool.get().await?;
        let raw: std::collections::HashMap<String, String> = conn.hgetall(HASH_KEY).await?;
        Ok(raw
            .into_values()
            .filter_map(|v| serde_json::from_str(&v).ok())
            .collect())
    }

    /// Just the fingerprint set, for `ai_group_suggestions`' own filtering pass — avoids
    /// deserializing every entry's full JSON body when all that's needed is a fast membership
    /// check per candidate group.
    pub async fn ignored_fingerprints(&self) -> Result<std::collections::HashSet<String>> {
        let mut conn = self.pool.get().await?;
        let keys: Vec<String> = conn.hkeys(HASH_KEY).await?;
        Ok(keys.into_iter().collect())
    }

    pub async fn ignore(
        &self,
        archive_ids: &[String],
        existing_tankoubon_id: Option<&str>,
        ignored_at: u64,
    ) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let fp = fingerprint(archive_ids, existing_tankoubon_id);
        let entry = IgnoredGroupSuggestion {
            archive_ids: archive_ids.to_vec(),
            existing_tankoubon_id: existing_tankoubon_id.map(str::to_string),
            ignored_at,
        };
        // Fields aren't `Serialize`-derived-error-prone here (plain strings/u64), so this can't
        // actually fail — `.expect` rather than threading a JSON error variant through for a call
        // that structurally cannot produce one.
        let raw = serde_json::to_string(&entry).expect("IgnoredGroupSuggestion always serializes");
        let _: () = conn.hset(HASH_KEY, fp, raw).await?;
        Ok(())
    }

    /// Idempotent — un-ignoring a combination that isn't currently ignored is a no-op, not an
    /// error (matches `PluginOptionsRepository::delete`'s own contract).
    pub async fn unignore(
        &self,
        archive_ids: &[String],
        existing_tankoubon_id: Option<&str>,
    ) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let fp = fingerprint(archive_ids, existing_tankoubon_id);
        let _: () = conn.hdel(HASH_KEY, fp).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_order_independent() {
        let a = fingerprint(&["b".to_string(), "a".to_string()], None);
        let b = fingerprint(&["a".to_string(), "b".to_string()], None);
        assert_eq!(a, b);
    }

    #[test]
    fn fingerprint_differs_by_existing_tankoubon_id() {
        let a = fingerprint(&["a".to_string()], None);
        let b = fingerprint(&["a".to_string()], Some("TANK_1"));
        let c = fingerprint(&["a".to_string()], Some("TANK_2"));
        assert_ne!(a, b);
        assert_ne!(b, c);
    }

    #[test]
    fn fingerprint_differs_when_membership_differs() {
        let a = fingerprint(&["a".to_string(), "b".to_string()], None);
        let b = fingerprint(&["a".to_string(), "c".to_string()], None);
        assert_ne!(a, b);
    }

    async fn test_pool() -> Option<Pool> {
        crate::test_support::test_pool().await
    }

    #[tokio::test]
    async fn round_trips_ignore_list_all_and_unignore() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = IgnoredGroupSuggestionsRepository::new(pool.clone());
        let mut conn = pool.get().await.unwrap();
        let _: () = deadpool_redis::redis::AsyncCommands::del::<_, ()>(&mut conn, HASH_KEY)
            .await
            .unwrap();

        let ids = vec!["a".to_string(), "b".to_string()];
        repo.ignore(&ids, None, 1_700_000_000).await.unwrap();

        let all = repo.list_all().await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].archive_ids, ids);
        assert_eq!(all[0].existing_tankoubon_id, None);

        let fps = repo.ignored_fingerprints().await.unwrap();
        assert_eq!(fps.len(), 1);
        assert!(fps.contains(&fingerprint(&ids, None)));

        repo.unignore(&ids, None).await.unwrap();
        assert!(repo.list_all().await.unwrap().is_empty());

        // Idempotent.
        repo.unignore(&ids, None).await.unwrap();
    }
}
