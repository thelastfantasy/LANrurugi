//! `stamps` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml` and
//! `Model/Stamp.pm` (constitution Principle II) — closes the Stamp-entity gap `data-model.md`
//! flagged (present in legacy data but not named in the feature spec's Key Entities).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::common::{error, not_found};
use crate::AppState;

#[derive(Debug, Serialize)]
pub struct StampJson {
    pub id: String,
    pub position: String,
    pub content: String,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/archives/{id}/stamps", get(stamped_pages))
        .route(
            "/archives/{id}/stamps/{index}",
            get(stamps_by_page).put(add_stamp),
        )
        .route(
            "/stamps/{id}",
            get(get_stamp).put(update_stamp).delete(delete_stamp),
        )
}

/// `GET /archives/{id}/stamps` — pages that have at least one stamp (legacy `get_stamped_pages`).
async fn stamped_pages(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "get_stamped_pages",
                format!("{id} does not exist in the database."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_stamped_pages",
                e.to_string(),
            )
        }
    };

    let mut pages = std::collections::BTreeSet::new();
    for stamp_id in &archive.stamp_ids {
        if let Some(page) = page_of(stamp_id) {
            pages.insert(page);
        }
    }
    let result: Vec<String> = pages.into_iter().map(|p| p.to_string()).collect();
    axum::Json(json!({ "result": result })).into_response()
}

fn page_of(stamp_id: &str) -> Option<u32> {
    stamp_id
        .strip_prefix("STAMPS_")
        .and_then(|rest| rest.split('_').next())
        .and_then(|p| p.parse().ok())
}

async fn stamps_by_page(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, u32)>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "get_stamps_by_page",
                format!("{id} does not exist in the database."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_stamps_by_page",
                e.to_string(),
            )
        }
    };

    let mut result = Vec::new();
    for stamp_id in &archive.stamp_ids {
        if page_of(stamp_id) != Some(index) {
            continue;
        }
        if let Ok(Some(s)) = state.repos.stamps.get(stamp_id).await {
            result.push(StampJson {
                id: s.stamp_id,
                position: s.position,
                content: s.content,
            });
        }
    }
    axum::Json(json!({ "result": result })).into_response()
}

#[derive(Debug, Deserialize, Default)]
pub struct StampParams {
    content: Option<String>,
    position: Option<String>,
}

async fn add_stamp(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, u32)>,
    Query(params): Query<StampParams>,
) -> Response {
    let archive = match state.repos.archives.get(&id).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            return error(
                StatusCode::BAD_REQUEST,
                "add_stamp",
                format!("{id} does not exist in the database."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "add_stamp",
                e.to_string(),
            )
        }
    };
    if index == 0 || index > archive.pagecount {
        return error(
            StatusCode::BAD_REQUEST,
            "add_stamp",
            format!("Page {index} out of range."),
        );
    }

    let now_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    match state
        .repos
        .stamps
        .create(
            &id,
            index,
            params.content.as_deref().unwrap_or_default(),
            params.position.as_deref().unwrap_or_default(),
            now_millis,
        )
        .await
    {
        Ok(stamp_id) => axum::Json(json!({
            "operation": "add_stamp",
            "stamp_id": stamp_id,
            "success": 1,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "add_stamp",
            e.to_string(),
        ),
    }
}

async fn get_stamp(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.repos.stamps.get(&id).await {
        Ok(Some(s)) => axum::Json(StampJson {
            id: s.stamp_id,
            position: s.position,
            content: s.content,
        })
        .into_response(),
        Ok(None) => not_found("get_stamp", format!("{id} doesn't exist in the database!")),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_stamp",
            e.to_string(),
        ),
    }
}

async fn update_stamp(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<StampParams>,
) -> Response {
    match state
        .repos
        .stamps
        .update(&id, params.content.as_deref(), params.position.as_deref())
        .await
    {
        Ok(()) => axum::Json(json!({ "operation": "update_stamp", "success": 1 })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_stamp",
            e.to_string(),
        ),
    }
}

async fn delete_stamp(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.repos.stamps.delete(&id).await {
        Ok(()) => axum::Json(json!({ "operation": "delete_stamp", "success": 1 })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_stamp",
            e.to_string(),
        ),
    }
}
