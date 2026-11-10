//! Per-group user decisions: which copies to keep, which pairs are not duplicates.

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;

use super::model::DecisionRecord;
use crate::AppState;

pub const DECISIONS_KEY: &str = "LANRURUGI_DUP_DECISIONS";

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
