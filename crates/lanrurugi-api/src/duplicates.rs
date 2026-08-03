//! `duplicates` endpoint group — additive, no OpenAPI contract equivalent (legacy's
//! `/duplicates` page is server-rendered HTML, `Controller/Duplicates.pm`).
//!
//! **Verified against source**: legacy's "duplicate detection" compares SHA-1 hashes of each
//! archive's *cover* image (`Utils/Archive.pm::extract_thumbnail`: `shasum_str($arcimg, 1)`,
//! stored as the `thumbhash` field — see `lanrurugi_scanner::thumbnail::generate`), not a true
//! perceptual/image hash. Groups are formed by connected components under a Hamming-distance
//! threshold over that hex string (`Utils/Minion.pm`'s `find_duplicates` task: BFS/union over
//! all pairs within `threshold`, default `5`), persisted to the same `LRR_DUPLICATE_GROUPS` hash
//! on the config DB legacy itself writes, keyed `dupgp_{composite}` where composite is the
//! concatenation of each sorted group member's first 10 ID characters.

use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;

use crate::common::error;
use crate::AppState;

const GROUPS_KEY: &str = "LRR_DUPLICATE_GROUPS";
const DEFAULT_THRESHOLD: u32 = 5;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/database/duplicates",
            get(get_duplicates).delete(clear_duplicates),
        )
        .route("/database/duplicates/scan", post(scan_duplicates))
}

#[derive(Debug, Deserialize, Default)]
struct ScanParams {
    threshold: Option<u32>,
}

/// Queues the scan as a background job (matches legacy's `/api/minion/find_duplicates/queue`
/// being Minion-backed rather than synchronous) since hashing every archive pair is O(n²) and
/// shouldn't block the request.
async fn scan_duplicates(
    State(state): State<AppState>,
    Query(params): Query<ScanParams>,
) -> Response {
    let threshold = params.threshold.unwrap_or(DEFAULT_THRESHOLD);

    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "find_duplicates",
                e.to_string(),
            )
        }
    };

    let jobs = state.jobs.clone();
    let job_id = jobs.create("find_duplicates").await;
    let job_id_for_task = job_id.clone();
    let redis = state.redis.config.clone();

    tokio::spawn(async move {
        jobs.mark_active(&job_id_for_task).await;

        let thumbhashes: HashMap<String, String> = archives
            .into_iter()
            .filter_map(|a| a.thumbhash.map(|h| (a.id.into_string(), h)))
            .collect();
        let groups = group_by_hamming_distance(&thumbhashes, threshold);

        match redis.get().await {
            Ok(mut conn) => {
                for group in &groups {
                    let key = format!("dupgp_{}", composite_key(group));
                    let value = serde_json::to_string(group).unwrap_or_default();
                    let _: Result<(), _> = conn.hset(GROUPS_KEY, &key, value).await;
                }
                jobs.finish(&job_id_for_task, json!({ "groups_found": groups.len() }))
                    .await;
            }
            Err(e) => jobs.fail(&job_id_for_task, e.to_string()).await,
        }
    });

    axum::Json(json!({ "operation": "find_duplicates", "success": 1, "job": job_id }))
        .into_response()
}

/// Connected components over the "within `threshold` Hamming distance" relation — matches
/// legacy's stack-based DFS exactly (same grouping semantics, order doesn't matter since results
/// are sorted before storage).
fn group_by_hamming_distance(
    thumbhashes: &HashMap<String, String>,
    threshold: u32,
) -> Vec<Vec<String>> {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut groups = Vec::new();

    for start_id in thumbhashes.keys() {
        if visited.contains(start_id.as_str()) {
            continue;
        }
        let mut stack = vec![start_id.as_str()];
        let mut group = Vec::new();

        while let Some(node) = stack.pop() {
            if !visited.insert(node) {
                continue;
            }
            group.push(node.to_string());

            let node_hash = &thumbhashes[node];
            for (other_id, other_hash) in thumbhashes {
                if visited.contains(other_id.as_str()) {
                    continue;
                }
                if hamming_distance(node_hash, other_hash) <= threshold {
                    stack.push(other_id);
                }
            }
        }

        if group.len() >= 2 {
            group.sort();
            groups.push(group);
        }
    }

    groups
}

fn hamming_distance(a: &str, b: &str) -> u32 {
    a.chars()
        .zip(b.chars())
        .filter(|(x, y)| x != y)
        .count()
        .max(a.len().abs_diff(b.len())) as u32
}

fn composite_key(group: &[String]) -> String {
    group.iter().map(|id| &id[0..10.min(id.len())]).collect()
}

/// Reads every stored group, resolving IDs to current archive metadata and pruning entries that
/// have vanished since the scan (deleted archives) — matches legacy's self-healing behavior in
/// `Controller/Duplicates.pm::index` (drops the whole group if it'd shrink below 2 members,
/// otherwise just removes the vanished ID and rewrites the group).
async fn get_duplicates(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_duplicates",
                e.to_string(),
            )
        }
    };
    let raw_groups: HashMap<String, String> = conn.hgetall(GROUPS_KEY).await.unwrap_or_default();

    let mut result = Vec::new();
    for (key, json_ids) in raw_groups {
        let Ok(ids) = serde_json::from_str::<Vec<String>>(&json_ids) else {
            continue;
        };

        let mut surviving = Vec::new();
        let mut archives = Vec::new();
        for id in &ids {
            match state
                .repos
                .archives
                .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
                .await
            {
                Ok(Some(archive)) => {
                    surviving.push(id.clone());
                    archives.push(json!({
                        "arcid": archive.id,
                        "title": archive.title,
                        "tags": archive.tags,
                        "size": archive.arcsize,
                        "group_key": key,
                    }));
                }
                _ => continue,
            }
        }

        if surviving.len() < 2 {
            let _: Result<(), _> = conn.hdel(GROUPS_KEY, &key).await;
            continue;
        }
        if surviving.len() != ids.len() {
            let value = serde_json::to_string(&surviving).unwrap_or_default();
            let _: Result<(), _> = conn.hset(GROUPS_KEY, &key, value).await;
        }

        result.push(archives);
    }

    axum::Json(result).into_response()
}

/// `DELETE /database/duplicates` — matches legacy's `?delete=1` param on the same page route.
async fn clear_duplicates(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "clear_duplicates",
                e.to_string(),
            )
        }
    };
    let _: Result<(), _> = conn.del(GROUPS_KEY).await;
    axum::Json(json!({ "operation": "clear_duplicates", "success": 1 })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_hashes_form_a_group() {
        let mut hashes = HashMap::new();
        hashes.insert("a".to_string(), "1111".to_string());
        hashes.insert("b".to_string(), "1111".to_string());
        let groups = group_by_hamming_distance(&hashes, 0);
        assert_eq!(groups, vec![vec!["a".to_string(), "b".to_string()]]);
    }

    #[test]
    fn distance_beyond_threshold_does_not_group() {
        let mut hashes = HashMap::new();
        hashes.insert("a".to_string(), "0000".to_string());
        hashes.insert("b".to_string(), "1111".to_string());
        let groups = group_by_hamming_distance(&hashes, 2);
        assert!(groups.is_empty());
    }

    #[test]
    fn distance_within_threshold_groups_transitively() {
        let mut hashes = HashMap::new();
        hashes.insert("a".to_string(), "0000".to_string());
        hashes.insert("b".to_string(), "0001".to_string());
        hashes.insert("c".to_string(), "0011".to_string());
        // a<->b distance 1, b<->c distance 1, a<->c distance 2 — all should join one group via
        // the transitive BFS/DFS walk even though a<->c alone would exceed a threshold of 1.
        let groups = group_by_hamming_distance(&hashes, 1);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 3);
    }

    #[test]
    fn singletons_are_not_returned_as_groups() {
        let mut hashes = HashMap::new();
        hashes.insert("a".to_string(), "0000".to_string());
        hashes.insert("b".to_string(), "ffff".to_string());
        let groups = group_by_hamming_distance(&hashes, 0);
        assert!(groups.is_empty());
    }

    #[test]
    fn composite_key_concatenates_first_ten_chars_of_each_sorted_id() {
        let group = vec!["a".repeat(12), "b".repeat(12)];
        assert_eq!(
            composite_key(&group),
            format!("{}{}", "a".repeat(10), "b".repeat(10))
        );
    }
}
