//! Duplicate detection and review (spec 024).
//!
//! Replaces legacy's cover-SHA-1 grouping with a perceptual hash plus metadata evidence, keeps the
//! legacy `LRR_DUPLICATE_GROUPS` hash in sync, and exposes the group/compare/decision endpoints the
//! new `/duplicates` page uses. The legacy read endpoint (`GET /database/duplicates`) is unchanged.

pub mod cover;
pub mod decisions;
pub mod detect;
pub mod model;
pub mod review;

use std::collections::{HashMap, HashSet};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;

use crate::common::error;
use crate::AppState;
use model::{ArchiveMeta, DecisionRecord, GroupRecord};

const GROUPS_KEY: &str = "LANRURUGI_DUP_GROUPS";
const LEGACY_GROUPS_KEY: &str = "LRR_DUPLICATE_GROUPS";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/database/duplicates",
            get(get_duplicates).delete(clear_duplicates),
        )
        .route("/database/duplicates/scan", post(scan_duplicates))
        .route("/database/duplicates/groups", get(list_groups))
        .route("/database/duplicates/groups/{key}", get(group_detail))
        .route("/database/duplicates/compare", get(compare_pair))
        .route(
            "/database/duplicates/decisions",
            get(get_decisions).post(put_decisions),
        )
}

#[derive(Debug, Deserialize, Default)]
struct ScanParams {
    threshold: Option<u32>,
}

async fn scan_duplicates(
    State(state): State<AppState>,
    Query(params): Query<ScanParams>,
) -> Response {
    let threshold = params.threshold.unwrap_or(detect::DEFAULT_THRESHOLD);
    if threshold > 16 {
        return error(
            StatusCode::BAD_REQUEST,
            "find_duplicates",
            "threshold must be between 0 and 16",
        );
    }
    if state
        .jobs
        .by_name("find_duplicates")
        .await
        .iter()
        .any(|job| !job.state.is_terminal())
    {
        return error(
            StatusCode::CONFLICT,
            "find_duplicates",
            "a duplicate scan is already running",
        );
    }

    let job_id = state.jobs.create("find_duplicates").await;
    let state_for_task = state.clone();
    let job_for_task = job_id.clone();
    tokio::spawn(async move {
        state_for_task.jobs.mark_active(&job_for_task).await;
        match run_scan(&state_for_task, threshold, &job_for_task).await {
            Ok(found) => {
                state_for_task
                    .jobs
                    .finish(&job_for_task, json!({ "groups_found": found }))
                    .await
            }
            Err(message) => state_for_task.jobs.fail(&job_for_task, message).await,
        }
    });

    axum::Json(json!({"operation": "find_duplicates", "success": 1, "job": job_id})).into_response()
}

async fn run_scan(state: &AppState, threshold: u32, job: &str) -> Result<usize, String> {
    let archives = state
        .repos
        .archives
        .list_all()
        .await
        .map_err(|e| e.to_string())?;
    // Only catalogued archives with a real file are candidates; an unfinished ingest has no cover.
    let metas: Vec<ArchiveMeta> = archives
        .iter()
        .filter(|a| !a.file.trim().is_empty() && a.pagecount > 0)
        .map(|a| ArchiveMeta {
            id: a.id.as_str().to_string(),
            title: a.title.clone(),
            tags: split_tags(&a.tags),
            pages: a.pagecount,
            size: a.arcsize,
        })
        .collect();
    state.jobs.set_progress(job, 0.05).await;

    let ids: Vec<String> = metas.iter().map(|m| m.id.clone()).collect();
    let covers = cover::collect(state, &ids).await;
    state.jobs.set_progress(job, 0.6).await;

    let decisions = decisions::load_all(state).await;
    let hashes = covers.hashes;
    let metas_for_task = metas.clone();
    let hashes_for_task = hashes.clone();
    let mut groups = lanrurugi_core::concurrency::run_blocking(move || {
        detect::build_groups(&metas_for_task, &hashes_for_task, threshold, now_secs())
    })
    .await
    .map_err(|e| e.to_string())?;

    // A pair the user marked "not duplicate" never becomes a group edge again, even when both
    // files are still within the threshold.
    for group in &mut groups {
        group.pairs.retain(|p| {
            !decisions
                .get(&group_key(&group.members))
                .map(|d| d.rejects(&p.a, &p.b))
                .unwrap_or(false)
        });
    }
    let hashes_for_regroup = hashes.clone();
    groups = groups
        .iter()
        .flat_map(|group| detect::regroup_by_pairs(group, &hashes_for_regroup))
        .collect();

    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let _: Result<(), _> = conn.del(GROUPS_KEY).await;
    let _: Result<(), _> = conn.del(LEGACY_GROUPS_KEY).await;
    for group in &groups {
        let key = group_key(&group.members);
        let value = serde_json::to_string(group).unwrap_or_default();
        let _: Result<(), _> = conn.hset(GROUPS_KEY, &key, value).await;
        let _: Result<(), _> = conn
            .hset(
                LEGACY_GROUPS_KEY,
                format!("dupgp_{}", legacy_composite(&group.members)),
                serde_json::to_string(&group.members).unwrap_or_default(),
            )
            .await;
    }
    // Semantic review: one model call per group without a cached verdict. A fallback verdict is
    // returned to the UI but never stored — it is a local guess, and caching it would make the
    // guess permanent the moment a provider appears.
    for group in &groups {
        let verdict = review::ensure(state, group, &metas).await;
        if verdict.fallback {
            continue;
        }
        if let Ok(mut conn) = state.redis.config.get().await {
            let value = serde_json::to_string(&verdict).unwrap_or_default();
            let _: Result<(), _> = conn
                .hset(review::REVIEW_KEY, group_key(&group.members), value)
                .await;
        }
    }

    state.jobs.set_progress(job, 1.0).await;
    Ok(groups.len())
}

pub fn group_key(members: &[String]) -> String {
    let mut sorted = members.to_vec();
    sorted.sort();
    sorted.join(",")
}

fn legacy_composite(members: &[String]) -> String {
    let mut sorted = members.to_vec();
    sorted.sort();
    sorted.iter().map(|id| &id[..10.min(id.len())]).collect()
}

async fn load_groups(state: &AppState) -> Vec<GroupRecord> {
    let Ok(mut conn) = state.redis.config.get().await else {
        return Vec::new();
    };
    let raw: HashMap<String, String> = conn.hgetall(GROUPS_KEY).await.unwrap_or_default();
    let mut groups: Vec<GroupRecord> = raw
        .into_values()
        .filter_map(|v| serde_json::from_str(&v).ok())
        .collect();
    groups.sort_by(|a, b| a.members.first().cmp(&b.members.first()));
    groups
}

async fn list_groups(State(state): State<AppState>) -> Response {
    let groups = load_groups(&state).await;
    let decisions = decisions::load_all(&state).await;
    let reviews = review::load_all(&state).await;
    let mut views = Vec::with_capacity(groups.len());
    for group in &groups {
        let key = group_key(&group.members);
        let decision = decisions.get(&key).cloned().unwrap_or_default();
        let review = reviews.get(&key).cloned();
        views.push(group_view(&state, group, &key, &decision, review).await);
    }
    axum::Json(json!({
        "scanned_at": groups.first().map(|g| g.built_at).unwrap_or(0),
        "algorithm": detect::ALGORITHM,
        "chained_groups": groups.iter().filter(|g| g.chained).count(),
        "groups": views,
    }))
    .into_response()
}

async fn group_view(
    state: &AppState,
    group: &GroupRecord,
    key: &str,
    decision: &DecisionRecord,
    review: Option<model::ReviewVerdict>,
) -> serde_json::Value {
    let mut members = Vec::with_capacity(group.members.len());
    for id in &group.members {
        let archive = state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
            .await
            .ok()
            .flatten();
        let Some(archive) = archive else { continue };
        let recommended = review
            .as_ref()
            .map(|r| r.keep.iter().any(|k| k == id))
            .unwrap_or(false);
        members.push(json!({
            "id": id,
            "title": archive.title,
            "size": archive.arcsize,
            "pages": archive.pagecount,
            "cover": format!("/api/archives/{id}/thumbnail"),
            "decision": decision.state(id),
            "recommended": recommended,
        }));
    }
    json!({
        "key": key,
        "chained": group.chained,
        "middle": group.middle,
        "members": members,
        "pairs": group.pairs.iter().map(|p| json!({
            "a": p.a, "b": p.b, "similarity": p.similarity,
            "distance": p.distance, "title_similarity": p.title_similarity,
        })).collect::<Vec<_>>(),
        "review": review,
    })
}

async fn group_detail(State(state): State<AppState>, Path(key): Path<String>) -> Response {
    let groups = load_groups(&state).await;
    let Some(group) = groups.into_iter().find(|g| group_key(&g.members) == key) else {
        return error(StatusCode::NOT_FOUND, "duplicates", "group not found");
    };
    let decision = decisions::load_all(&state)
        .await
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let review = review::load_all(&state).await.get(&key).cloned();
    let mut view = group_view(&state, &group, &key, &decision, review).await;

    let matrix: Vec<Vec<f64>> = group
        .members
        .iter()
        .map(|a| {
            group
                .members
                .iter()
                .map(|b| {
                    if a == b {
                        1.0
                    } else {
                        group
                            .pairs
                            .iter()
                            .find(|p| (&p.a == a && &p.b == b) || (&p.a == b && &p.b == a))
                            .map(|p| p.similarity)
                            .unwrap_or(0.0)
                    }
                })
                .collect()
        })
        .collect();
    view["matrix"] = json!(matrix);
    axum::Json(view).into_response()
}

#[derive(Debug, Deserialize)]
struct CompareParams {
    a: String,
    b: String,
}

async fn compare_pair(
    State(state): State<AppState>,
    Query(params): Query<CompareParams>,
) -> Response {
    match review::compare(&state, &params.a, &params.b).await {
        Ok(value) => axum::Json(value).into_response(),
        Err((status, message)) => error(status, "duplicates", message),
    }
}

#[derive(Debug, Deserialize)]
struct DecisionsQuery {
    group_key: String,
}

async fn get_decisions(
    State(state): State<AppState>,
    Query(query): Query<DecisionsQuery>,
) -> Response {
    let record = decisions::load_all(&state)
        .await
        .get(&query.group_key)
        .cloned()
        .unwrap_or_default();
    axum::Json(json!({
        "group_key": query.group_key,
        "decisions": record.decisions,
        "not_duplicate_pairs": record.not_duplicate,
        "updated_at": record.updated_at,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
struct DecisionsBody {
    group_key: String,
    #[serde(default)]
    decisions: HashMap<String, String>,
    #[serde(default)]
    not_duplicate_pairs: Vec<[String; 2]>,
}

async fn put_decisions(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<DecisionsBody>,
) -> Response {
    let groups = load_groups(&state).await;
    let Some(group) = groups
        .into_iter()
        .find(|g| group_key(&g.members) == body.group_key)
    else {
        return error(StatusCode::NOT_FOUND, "duplicates", "group not found");
    };
    let members: HashSet<&str> = group.members.iter().map(String::as_str).collect();
    for (id, state_value) in &body.decisions {
        if !members.contains(id.as_str()) {
            return error(
                StatusCode::BAD_REQUEST,
                "duplicates",
                "decision names an archive outside this group",
            );
        }
        if !matches!(state_value.as_str(), "keep" | "discard" | "undecided") {
            return error(
                StatusCode::BAD_REQUEST,
                "duplicates",
                "decision state must be keep, discard or undecided",
            );
        }
    }
    for [a, b] in &body.not_duplicate_pairs {
        if !members.contains(a.as_str()) || !members.contains(b.as_str()) {
            return error(
                StatusCode::BAD_REQUEST,
                "duplicates",
                "a not-duplicate pair names an archive outside this group",
            );
        }
    }
    let mut record = decisions::load_all(&state)
        .await
        .get(&body.group_key)
        .cloned()
        .unwrap_or_default();
    for (id, value) in body.decisions {
        if value == "undecided" {
            record.decisions.remove(&id);
        } else {
            record.decisions.insert(id, value);
        }
    }
    for pair in body.not_duplicate_pairs {
        if !record.rejects(&pair[0], &pair[1]) {
            record.not_duplicate.push(pair);
        }
    }
    record.updated_at = now_secs();
    if let Err(e) = decisions::save(&state, &body.group_key, &record).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "duplicates", e);
    }
    axum::Json(json!({"operation": "duplicate_decisions", "success": 1})).into_response()
}

/// Legacy read endpoint: unchanged shape, still fed by `LRR_DUPLICATE_GROUPS`.
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
    let raw_groups: HashMap<String, String> =
        conn.hgetall(LEGACY_GROUPS_KEY).await.unwrap_or_default();
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
                        "extension": archive.extension(),
                        "group_key": key,
                    }));
                }
                _ => continue,
            }
        }
        if surviving.len() < 2 {
            let _: Result<(), _> = conn.hdel(LEGACY_GROUPS_KEY, &key).await;
            continue;
        }
        if surviving.len() != ids.len() {
            let value = serde_json::to_string(&surviving).unwrap_or_default();
            let _: Result<(), _> = conn.hset(LEGACY_GROUPS_KEY, &key, value).await;
        }
        result.push(archives);
    }
    axum::Json(result).into_response()
}

async fn clear_duplicates(State(state): State<AppState>) -> Response {
    let Ok(mut conn) = state.redis.config.get().await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "clear_duplicates",
            "redis unavailable",
        );
    };
    for key in [
        GROUPS_KEY,
        LEGACY_GROUPS_KEY,
        cover::COVER_HASH_KEY,
        decisions::DECISIONS_KEY,
        review::REVIEW_KEY,
    ] {
        let _: Result<(), _> = conn.del(key).await;
    }
    axum::Json(json!({"operation": "clear_duplicates", "success": 1})).into_response()
}

fn split_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_key_is_sorted_and_stable() {
        let a = vec!["b".to_string(), "a".to_string()];
        let b = vec!["a".to_string(), "b".to_string()];
        assert_eq!(group_key(&a), "a,b");
        assert_eq!(group_key(&a), group_key(&b));
    }

    #[test]
    fn legacy_composite_uses_first_ten_chars() {
        let members = vec!["a".repeat(12), "b".repeat(12)];
        assert_eq!(
            legacy_composite(&members),
            format!("{}{}", "a".repeat(10), "b".repeat(10))
        );
    }
}
