//! `/download_queue*` — the Upload page's persistent, plugin-grouped download queue (additive,
//! no legacy equivalent). Backed by [`lanrurugi_storage::download_queue::DownloadQueueRepository`]
//! so a queued/in-progress item survives a page refresh or a different browser tab; starting an
//! item reuses `plugins::start_download`'s exact dispatch/execute/ingest sequence via a
//! `pub(crate)` re-export, keeping this module's own responsibility to CRUD + orchestrating which
//! items to start.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::Router;
use lanrurugi_storage::download_queue::DownloadQueueState;
use serde::Deserialize;
use serde_json::json;

use crate::common::{error, not_found, ok};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/download_queue", get(list_queue).post(add_to_queue))
        .route(
            "/download_queue/{id}",
            patch(update_queue_item).delete(delete_queue_item),
        )
        .route("/download_queue/{id}/start", post(start_queue_item))
        .route("/download_queue/start_all", post(start_all))
        .route("/download_queue/start_selected", post(start_selected))
        .route("/download_queue/clear_completed", post(clear_completed))
}

async fn list_queue(State(state): State<AppState>) -> Response {
    match state.download_queue.list_all().await {
        Ok(items) => axum::Json(json!({ "items": items })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_download_queue",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct AddQueueItem {
    url: String,
    plugin_namespace: String,
    category: Option<String>,
    auto_fetch_metadata: bool,
    overwrite_on_duplicate: bool,
}

#[derive(Debug, Deserialize)]
struct AddQueueBody {
    items: Vec<AddQueueItem>,
}

/// `POST /download_queue` — bulk add. The client has already resolved which download plugin's
/// `url_pattern` matched each URL and computed the two checkbox defaults; this handler's only
/// validation is that `plugin_namespace` actually resolves to an installed plugin — invalid
/// entries are rejected individually (`rejected: [{url, reason}]`) rather than failing the whole
/// batch, so one bad URL in a pasted list doesn't block the rest.
async fn add_to_queue(State(state): State<AppState>, body: axum::Json<AddQueueBody>) -> Response {
    let mut added = Vec::new();
    let mut rejected = Vec::new();

    for item in &body.items {
        match state.plugins.plugin_info(&item.plugin_namespace).await {
            Ok(_) => match state
                .download_queue
                .add(
                    item.url.clone(),
                    item.plugin_namespace.clone(),
                    item.category.clone(),
                    item.auto_fetch_metadata,
                    item.overwrite_on_duplicate,
                )
                .await
            {
                Ok(saved) => added.push(saved),
                Err(e) => rejected.push(json!({ "url": item.url, "reason": e.to_string() })),
            },
            Err(_) => rejected.push(json!({
                "url": item.url,
                "reason": format!("no installed plugin under namespace {:?}", item.plugin_namespace),
            })),
        }
    }

    axum::Json(json!({ "added": added, "rejected": rejected })).into_response()
}

#[derive(Debug, Deserialize)]
struct UpdateQueueItemBody {
    title: Option<String>,
    /// The metadata plugin's full `execMetadata` response — see
    /// `DownloadQueueItem::metadata_preview`'s own docs for why this is untyped JSON.
    metadata_preview: Option<serde_json::Value>,
    auto_fetch_metadata: Option<bool>,
    overwrite_on_duplicate: Option<bool>,
}

/// `PATCH /download_queue/{id}` — partial update; a field omitted from the request body leaves
/// that stored value untouched (same convention as `PUT /plugins/options`).
async fn update_queue_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: axum::Json<UpdateQueueItemBody>,
) -> Response {
    let mut item = match state.download_queue.get(&id).await {
        Ok(Some(item)) => item,
        Ok(None) => {
            return not_found(
                "update_download_queue_item",
                format!("Item {id} not found."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_download_queue_item",
                e.to_string(),
            )
        }
    };

    if let Some(title) = &body.title {
        item.title = Some(title.clone());
    }
    if let Some(metadata_preview) = &body.metadata_preview {
        item.metadata_preview = Some(metadata_preview.clone());
    }
    if let Some(auto_fetch_metadata) = body.auto_fetch_metadata {
        item.auto_fetch_metadata = auto_fetch_metadata;
    }
    if let Some(overwrite_on_duplicate) = body.overwrite_on_duplicate {
        item.overwrite_on_duplicate = overwrite_on_duplicate;
    }

    match state.download_queue.update(&item).await {
        Ok(()) => axum::Json(json!({ "item": item })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_download_queue_item",
            e.to_string(),
        ),
    }
}

async fn delete_queue_item(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.download_queue.delete(&id).await {
        Ok(()) => ok("delete_download_queue_item", []),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_download_queue_item",
            e.to_string(),
        ),
    }
}

/// `POST /download_queue/{id}/start` — resolves the item's own (already-fixed, chosen at add
/// time) `plugin_namespace`, marks it `Starting`, and launches the real download via
/// `plugins::start_download` with `queue_link` set so the background task keeps this item's
/// `state`/`job_id`/`error` current as the download proceeds.
async fn start_queue_item(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match start_one(&state, &id).await {
        Ok(job_id) => axum::Json(
            json!({ "operation": "start_download_queue_item", "success": 1, "job": job_id }),
        )
        .into_response(),
        Err(StartError::NotFound) => {
            not_found("start_download_queue_item", format!("Item {id} not found."))
        }
        Err(StartError::NotQueued) => error(
            StatusCode::CONFLICT,
            "start_download_queue_item",
            format!("Item {id} is not in the Queued state."),
        ),
        Err(StartError::PluginMissing) => error(
            StatusCode::BAD_REQUEST,
            "start_download_queue_item",
            "The plugin this item was queued under is no longer installed.",
        ),
        Err(StartError::Storage(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "start_download_queue_item",
            e,
        ),
    }
}

/// `POST /download_queue/start_all` — every `Queued`-state item, fired concurrently (each spawns
/// its own independent background task already; per-domain concurrency/rate-limiting is enforced
/// downstream by the existing `DownloadManager`, so no additional throttling is needed here).
async fn start_all(State(state): State<AppState>) -> Response {
    let items = match state.download_queue.list_all().await {
        Ok(items) => items,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "start_all_download_queue",
                e.to_string(),
            )
        }
    };
    let ids: Vec<String> = items
        .into_iter()
        .filter(|i| i.state == DownloadQueueState::Queued)
        .map(|i| i.id)
        .collect();
    start_many(&state, ids).await
}

#[derive(Debug, Deserialize)]
struct StartSelectedBody {
    ids: Vec<String>,
}

/// `POST /download_queue/start_selected` — like `start_all`, filtered to the given IDs; starting
/// an ID that's already in-flight/done/not `Queued` is a silent no-op, not an error, so the
/// frontend can pass a selection without first checking each item's current state.
async fn start_selected(
    State(state): State<AppState>,
    body: axum::Json<StartSelectedBody>,
) -> Response {
    start_many(&state, body.ids.clone()).await
}

async fn start_many(state: &AppState, ids: Vec<String>) -> Response {
    let mut started = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        match start_one(state, &id).await {
            Ok(job_id) => started.push(json!({ "id": id, "job": job_id })),
            Err(StartError::NotQueued) => {} // silent no-op, per contract
            Err(e) => skipped.push(json!({ "id": id, "reason": start_error_message(&e) })),
        }
    }
    axum::Json(json!({ "started": started, "skipped": skipped })).into_response()
}

enum StartError {
    NotFound,
    NotQueued,
    PluginMissing,
    Storage(String),
}

fn start_error_message(e: &StartError) -> String {
    match e {
        StartError::NotFound => "item not found".to_string(),
        StartError::NotQueued => "item is not in the Queued state".to_string(),
        StartError::PluginMissing => "plugin no longer installed".to_string(),
        StartError::Storage(msg) => msg.clone(),
    }
}

async fn start_one(state: &AppState, id: &str) -> Result<String, StartError> {
    let mut item = state
        .download_queue
        .get(id)
        .await
        .map_err(|e| StartError::Storage(e.to_string()))?
        .ok_or(StartError::NotFound)?;

    if item.state != DownloadQueueState::Queued {
        return Err(StartError::NotQueued);
    }

    let info = state
        .plugins
        .plugin_info(&item.plugin_namespace)
        .await
        .map_err(|_| StartError::PluginMissing)?;

    item.state = DownloadQueueState::Starting;
    state
        .download_queue
        .update(&item)
        .await
        .map_err(|e| StartError::Storage(e.to_string()))?;

    let job_id = crate::plugins::start_download(
        state.clone(),
        item.plugin_namespace.clone(),
        info,
        item.url.clone(),
        item.category.clone(),
        item.overwrite_on_duplicate,
        Some((state.download_queue.clone(), item.id.clone())),
    )
    .await;

    Ok(job_id)
}

/// `POST /download_queue/clear_completed` — deletes every `Done`/`Error` item, returns the count
/// removed.
async fn clear_completed(State(state): State<AppState>) -> Response {
    let items = match state.download_queue.list_all().await {
        Ok(items) => items,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "clear_completed_download_queue",
                e.to_string(),
            )
        }
    };
    let ids: Vec<String> = items
        .into_iter()
        .filter(|i| {
            matches!(
                i.state,
                DownloadQueueState::Done | DownloadQueueState::Error
            )
        })
        .map(|i| i.id)
        .collect();
    let cleared = ids.len();
    if let Err(e) = state.download_queue.delete_many(&ids).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "clear_completed_download_queue",
            e.to_string(),
        );
    }
    ok(
        "clear_completed_download_queue",
        [("cleared", json!(cleared))],
    )
}
