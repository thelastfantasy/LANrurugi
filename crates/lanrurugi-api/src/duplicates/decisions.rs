//! Per-group user decisions: which copies to keep, which pairs are not duplicates.

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;

use super::model::DecisionRecord;
use crate::AppState;

pub const DECISIONS_KEY: &str = "LANRURUGI_DUP_DECISIONS";

/// Global "these two are not duplicates" set, independent of the group they were seen in. Stored
/// as a Redis set of `a|b` (sorted ids): a group key changes whenever a member is added, so a
/// decision under the group key would silently stop applying the next time the user downloads
/// another copy of the same work.
pub const NOT_DUPLICATE_KEY: &str = "LANRURUGI_DUP_NOT_DUPLICATE";

pub async fn load_rejects(state: &AppState) -> std::collections::HashSet<(String, String)> {
    let Ok(mut conn) = state.redis.config.get().await else {
        return std::collections::HashSet::new();
    };
    let raw: Vec<String> = conn.smembers(NOT_DUPLICATE_KEY).await.unwrap_or_default();
    raw.into_iter()
        .filter_map(|entry| {
            let (a, b) = entry.split_once('|')?;
            if a.is_empty() || b.is_empty() {
                return None;
            }
            Some(if a <= b {
                (a.to_string(), b.to_string())
            } else {
                (b.to_string(), a.to_string())
            })
        })
        .collect()
}

pub async fn add_rejects(state: &AppState, pairs: &[[String; 2]]) -> Result<(), String> {
    if pairs.is_empty() {
        return Ok(());
    }
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let members: Vec<String> = pairs
        .iter()
        .map(|[a, b]| {
            if a <= b {
                format!("{a}|{b}")
            } else {
                format!("{b}|{a}")
            }
        })
        .collect();
    let _: () = conn
        .sadd(NOT_DUPLICATE_KEY, members)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn load_all(state: &AppState) -> HashMap<String, DecisionRecord> {
    let Ok(mut conn) = state.redis.config.get().await else {
        return HashMap::new();
    };
    let raw: HashMap<String, String> = conn.hgetall(DECISIONS_KEY).await.unwrap_or_default();
    raw.into_iter()
        .filter_map(|(key, value)| serde_json::from_str(&value).ok().map(|v| (key, v)))
        .collect()
}

pub async fn save(state: &AppState, key: &str, record: &DecisionRecord) -> Result<(), String> {
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let value = serde_json::to_string(record).map_err(|e| e.to_string())?;
    let _: () = conn
        .hset(DECISIONS_KEY, key, value)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
