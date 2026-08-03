//! `tankoubons` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Model/Tankoubon.pm` (constitution Principle II). As with `archives.rs`, `GET
//! /tankoubons/{id}/progress/{page}` doesn't exist in the legacy contract (PUT-only,
//! `update_tank_progress`) — implemented as PUT only.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::Router;
use lanrurugi_core::entities::Grouping;
use lanrurugi_core::ids::{ArchiveId, TankId};
use serde::Deserialize;
use serde_json::json;

use crate::archives::ArchiveMetadataJson;
use crate::common::{error, not_found};
use crate::AppState;

/// Matches legacy's default `archives_per_page` (verified: `ServerInfo` example in
/// `tools/openapi.yaml`).
const DEFAULT_PAGE_SIZE: usize = 100;

fn tankoubon_summary_json(g: &Grouping) -> serde_json::Value {
    json!({
        "id": g.tankid,
        "name": g.name,
        "summary": g.summary,
        "tags": g.tags,
        "archives": g.archives,
        "progress": g.progress,
    })
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/tankoubons",
            get(list_tankoubons).put(create_or_rename_tankoubon),
        )
        .route(
            "/tankoubons/{id}",
            get(get_tankoubon)
                .put(update_tankoubon)
                .delete(delete_tankoubon),
        )
        .route("/tankoubons/{id}/full", get(get_tankoubon_full))
        .route("/tankoubons/{id}/thumbnail", get(get_tankoubon_thumbnail))
        .route(
            "/tankoubons/{id}/progress/{page}",
            put(update_tankoubon_progress),
        )
        .route(
            "/tankoubons/{id}/{archive}",
            put(add_to_tankoubon).delete(remove_from_tankoubon),
        )
}

#[derive(Debug, Deserialize)]
pub struct PageParam {
    page: Option<i64>,
}

async fn list_tankoubons(State(state): State<AppState>, Query(q): Query<PageParam>) -> Response {
    match state.repos.groupings.list_all().await {
        Ok(mut groupings) => {
            groupings.sort_by(|a, b| a.tankid.cmp(&b.tankid));
            let total = groupings.len();
            let page = q.page.unwrap_or(0);
            let items: Vec<_> = if page < 0 {
                groupings.iter().map(tankoubon_summary_json).collect()
            } else {
                let start = (page as usize) * DEFAULT_PAGE_SIZE;
                groupings
                    .iter()
                    .skip(start)
                    .take(DEFAULT_PAGE_SIZE)
                    .map(tankoubon_summary_json)
                    .collect()
            };
            let filtered = items.len();
            axum::Json(json!({ "result": items, "total": total, "filtered": filtered }))
                .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_tankoubon_list",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateTankoubonParams {
    name: String,
    tankid: Option<String>,
}

async fn create_or_rename_tankoubon(
    State(state): State<AppState>,
    axum::Form(params): axum::Form<CreateTankoubonParams>,
) -> Response {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let (tankid, mut grouping) = match params.tankid {
        Some(id) => {
            let id = TankId(id);
            match state.repos.groupings.get(&id).await {
                Ok(Some(g)) => (id, g),
                Ok(None) => {
                    return not_found(
                        "create_tankoubon",
                        format!("{id} doesn't exist in the database!"),
                    )
                }
                Err(e) => {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "create_tankoubon",
                        e.to_string(),
                    )
                }
            }
        }
        None => {
            let mut candidate = TankId(format!("TANK_{now}"));
            let mut attempt = now;
            while state
                .repos
                .groupings
                .get(&candidate)
                .await
                .ok()
                .flatten()
                .is_some()
            {
                attempt += 1;
                candidate = TankId(format!("TANK_{attempt}"));
            }
            (
                candidate.clone(),
                Grouping {
                    tankid: candidate,
                    name: String::new(),
                    summary: String::new(),
                    tags: String::new(),
                    progress: 0,
                    archives: Vec::new(),
                },
            )
        }
    };

    grouping.name = params.name;
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => axum::Json(json!({
            "operation": "create_tankoubon",
            "tankoubon_id": tankid,
            "success": 1,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "create_tankoubon",
            e.to_string(),
        ),
    }
}

async fn get_tankoubon(State(state): State<AppState>, Path(id): Path<TankId>) -> Response {
    match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => axum::Json(tankoubon_summary_json(&g)).into_response(),
        Ok(None) => not_found(
            "get_tankoubon",
            format!("{id} doesn't exist in the database!"),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_tankoubon",
            e.to_string(),
        ),
    }
}

async fn get_tankoubon_full(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
    Query(q): Query<PageParam>,
) -> Response {
    let grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "get_tankoubon_full",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_tankoubon_full",
                e.to_string(),
            )
        }
    };

    let total = grouping.archives.len();
    let page = q.page.unwrap_or(-1);
    let page_ids: Vec<ArchiveId> = if page < 0 {
        grouping.archives.clone()
    } else {
        let start = (page as usize) * DEFAULT_PAGE_SIZE;
        grouping
            .archives
            .iter()
            .skip(start)
            .take(DEFAULT_PAGE_SIZE)
            .cloned()
            .collect()
    };

    let mut full_data = Vec::with_capacity(page_ids.len());
    for id in &page_ids {
        if let Ok(Some(a)) = state.repos.archives.get(id).await {
            full_data.push(ArchiveMetadataJson::from(&a));
        }
    }
    let filtered = page_ids.len();

    axum::Json(json!({
        "result": {
            "id": grouping.tankid,
            "name": grouping.name,
            "summary": grouping.summary,
            "tags": grouping.tags,
            "progress": grouping.progress,
            "archives": page_ids,
            "full_data": full_data,
        },
        "total": total,
        "filtered": filtered,
    }))
    .into_response()
}

const PLACEHOLDER_THUMBNAIL: &[u8] = include_bytes!("../assets/no_thumb.png");

/// A legitimate tankoubon ID is always `TANK_` followed by exactly 10 ASCII digits
/// (`lanrurugi_storage::repository::GroupingRepository`, verified via its own `TANK_??????????`
/// Redis key-scan glob) — anything else must never be used to build a filesystem path.
fn is_valid_tankoubon_id(id: &str) -> bool {
    id.strip_prefix("TANK_")
        .is_some_and(|rest| rest.len() == 10 && rest.bytes().all(|b| b.is_ascii_digit()))
}

/// Read-only: serves an existing tank cover thumbnail (legacy path `<thumb_dir>/TA/<id>.<ext>`),
/// falling back to a placeholder. Generation (`update_tankoubon_thumbnail`) needs page extraction
/// from a member archive and isn't implemented yet (belongs with User Story 2's thumbnailing).
async fn get_tankoubon_thumbnail(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
) -> Response {
    // See `is_valid_tankoubon_id`'s docs: this endpoint (like `archives::get_archive_thumbnail`)
    // builds a filesystem path directly from `id` without a repository lookup first, so an
    // unvalidated value containing `/` or `..` would otherwise escape `thumb_dir`.
    if !is_valid_tankoubon_id(id.as_str()) {
        return ([(header::CONTENT_TYPE, "image/png")], PLACEHOLDER_THUMBNAIL).into_response();
    }
    for format in lanrurugi_scanner::thumbnail::ThumbFormat::ALL {
        let path = state
            .library
            .thumb_dir
            .join("TA")
            .join(format!("{id}.{}", format.extension()));
        if let Ok(bytes) = tokio::fs::read(&path).await {
            return ([(header::CONTENT_TYPE, format.content_type())], bytes).into_response();
        }
    }
    ([(header::CONTENT_TYPE, "image/png")], PLACEHOLDER_THUMBNAIL).into_response()
}

async fn update_tankoubon_progress(
    State(state): State<AppState>,
    Path((id, page)): Path<(TankId, u32)>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "update_tank_progress",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_tank_progress",
                e.to_string(),
            )
        }
    };
    grouping.progress = page;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => axum::Json(json!({
            "id": id,
            "operation": "update_tank_progress",
            "page": page,
            "lastreadtime": now,
            "success": 1,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_tank_progress",
            e.to_string(),
        ),
    }
}

async fn update_tankoubon(
    State(state): State<AppState>,
    Path(id): Path<TankId>,
    axum::Json(body): axum::Json<UpdateTankoubonBody>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "update_tankoubon",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_tankoubon",
                e.to_string(),
            )
        }
    };

    if let Some(metadata) = body.metadata {
        if let Some(name) = metadata.name {
            grouping.name = name;
        }
        if let Some(summary) = metadata.summary {
            grouping.summary = summary;
        }
        if let Some(tags) = metadata.tags {
            grouping.tags = if metadata.append.unwrap_or(false) && !grouping.tags.is_empty() {
                format!("{},{}", grouping.tags, tags)
            } else {
                tags
            };
        }
    }
    if let Some(archives) = body.archives {
        grouping.archives = archives.into_iter().map(ArchiveId).collect();
    }

    match state.repos.groupings.save(&grouping).await {
        Ok(()) => {
            axum::Json(json!({ "operation": "update_tankoubon", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_tankoubon",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateTankoubonBody {
    archives: Option<Vec<String>>,
    metadata: Option<UpdateTankoubonMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateTankoubonMetadata {
    name: Option<String>,
    summary: Option<String>,
    tags: Option<String>,
    append: Option<bool>,
}

async fn delete_tankoubon(State(state): State<AppState>, Path(id): Path<TankId>) -> Response {
    match state.repos.groupings.delete(&id).await {
        Ok(()) => {
            axum::Json(json!({ "operation": "delete_tankoubon", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_tankoubon",
            e.to_string(),
        ),
    }
}

async fn add_to_tankoubon(
    State(state): State<AppState>,
    Path((id, archive)): Path<(TankId, ArchiveId)>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "add_to_tankoubon",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "add_to_tankoubon",
                e.to_string(),
            )
        }
    };
    if state
        .repos
        .archives
        .get(&archive)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "add_to_tankoubon",
            format!("{archive} does not exist in the database."),
        );
    }
    if !grouping.archives.contains(&archive) {
        grouping.archives.push(archive);
    }
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => {
            axum::Json(json!({ "operation": "add_to_tankoubon", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "add_to_tankoubon",
            e.to_string(),
        ),
    }
}

async fn remove_from_tankoubon(
    State(state): State<AppState>,
    Path((id, archive)): Path<(TankId, ArchiveId)>,
) -> Response {
    let mut grouping = match state.repos.groupings.get(&id).await {
        Ok(Some(g)) => g,
        Ok(None) => {
            return not_found(
                "remove_from_tankoubon",
                format!("{id} doesn't exist in the database!"),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "remove_from_tankoubon",
                e.to_string(),
            )
        }
    };
    grouping.archives.retain(|a| a != &archive);
    match state.repos.groupings.save(&grouping).await {
        Ok(()) => axum::Json(json!({ "operation": "remove_from_tankoubon", "success": 1 }))
            .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "remove_from_tankoubon",
            e.to_string(),
        ),
    }
}
