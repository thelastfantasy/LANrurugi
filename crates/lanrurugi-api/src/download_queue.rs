//! `/download_queue*` — the Upload page's persistent, plugin-grouped download queue (additive,
//! no legacy equivalent). Backed by [`lanrurugi_storage::download_queue::DownloadQueueRepository`]
//! so a queued/in-progress item survives a page refresh or a different browser tab; starting an
//! item reuses `plugins::start_download`'s exact dispatch/execute/ingest sequence via a
//! `pub(crate)` re-export, keeping this module's own responsibility to CRUD + orchestrating which
//! items to start.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::Router;
use futures_util::StreamExt;
use lanrurugi_storage::download_queue::{DownloadQueueState, QueueItemOrigin};
use serde::Deserialize;
use serde_json::json;
use tokio_stream::wrappers::BroadcastStream;

use crate::common::{error, not_found, ok};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/download_queue", get(list_queue).post(add_to_queue))
        .route("/download_queue/stream", get(queue_stream))
        .route(
            "/download_queue/{id}",
            patch(update_queue_item).delete(delete_queue_item),
        )
        .route(
            "/download_queue/{id}/fetch-metadata",
            post(fetch_queue_item_metadata),
        )
        .route("/download_queue/{id}/start", post(start_queue_item))
        .route("/download_queue/{id}/stop", post(stop_queue_item))
        .route("/download_queue/{id}/overwrite", post(overwrite_queue_item))
        .route("/download_queue/{id}/rename", post(rename_queue_item))
        .route("/download_queue/{id}/compare", post(compare_queue_item))
        .route(
            "/download_queue/{id}/compare/stream",
            get(compare_queue_item_stream),
        )
        .route(
            "/download_queue/{id}/compare/page",
            get(compare_queue_item_page),
        )
        .route(
            "/download_queue/{id}/compare/pages",
            get(list_compare_pages),
        )
        .route(
            "/download_queue/{id}/compare/export-patch",
            post(export_compare_patch),
        )
        .route("/download_queue/{id}/compare/keep-b", post(keep_side_b))
        .route("/download_queue/start_all", post(start_all))
        .route("/download_queue/start_selected", post(start_selected))
        .route("/download_queue/delete_selected", post(delete_selected))
        .route("/download_queue/clear_completed", post(clear_completed))
}

/// `GET /download_queue/stream` — SSE endpoint that pushes incremental queue-item state changes
/// broadcast by `update_queue_item_state` after each successful Redis write. The first event is
/// `full` with the current complete list so the client can bootstrap without a separate HTTP GET.
async fn queue_stream(State(state): State<AppState>) -> Response {
    let Some(tx) = &state.download_queue_tx else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "queue_stream",
            "SSE not available",
        );
    };
    // Subscribe BEFORE reading the initial list — a delta broadcast in the gap between
    // `list_all` and `subscribe` would otherwise be missed by this client forever (until
    // the next delta/lag). With subscribe-first, that delta just sits buffered in `rx`
    // and is delivered right after the `full` event, on top of an already-consistent list.
    let rx = tx.subscribe();
    let initial = match state.download_queue.list_all().await {
        Ok(items) => serde_json::json!({ "type": "full", "items": items }),
        Err(_) => serde_json::json!({ "type": "full", "items": [] }),
    };
    let initial_event = Event::default()
        .event("full")
        .data(serde_json::to_string(&initial).unwrap_or_default());

    let queue_repo = state.download_queue.clone();
    let delta_stream = BroadcastStream::new(rx).filter_map(move |result| {
        let queue_repo = queue_repo.clone();
        async move {
            match result {
                Ok(event) => {
                    let sse_event: Event = Event::default().event("delta").data(
                        serde_json::to_string(
                            &serde_json::json!({ "type": "delta", "item": event }),
                        )
                        .unwrap_or_default(),
                    );
                    Some(Ok::<_, std::convert::Infallible>(sse_event))
                }
                // Client fell behind the broadcast buffer — resend the full list instead of
                // silently skipping, so the client can't end up with stale items forever.
                // (`Closed` isn't a variant — the stream just ends once the sender drops.)
                Err(tokio_stream::wrappers::errors::BroadcastStreamRecvError::Lagged(_)) => {
                    let full = match queue_repo.list_all().await {
                        Ok(items) => serde_json::json!({ "type": "full", "items": items }),
                        Err(_) => serde_json::json!({ "type": "full", "items": [] }),
                    };
                    let sse_event: Event = Event::default()
                        .event("full")
                        .data(serde_json::to_string(&full).unwrap_or_default());
                    Some(Ok::<_, std::convert::Infallible>(sse_event))
                }
            }
        }
    });

    let stream = futures_util::stream::once(async move { Ok(initial_event) }).chain(delta_stream);
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// `POST /download_queue/{id}/fetch-metadata` — the "Fetch Metadata" button's backend path. Runs
/// the same plugin-`execMetadata` + `metadata_preview`-cache logic the post-download auto-fetch
/// uses (`plugins::ensure_metadata_cached`), so a manual preview and the automatic one share one
/// code path (and one 10-min TTL) instead of the frontend calling `POST /plugins/use` itself.
async fn fetch_queue_item_metadata(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    let mut item = match state.download_queue.get(&id).await {
        Ok(Some(item)) => item,
        Ok(None) => return not_found("fetch_queue_item_metadata", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "fetch_queue_item_metadata",
                e.to_string(),
            )
        }
    };
    match crate::plugins::ensure_metadata_cached(&state, &mut item).await {
        Some(preview) => {
            axum::Json(json!({ "success": 1, "metadata_preview": preview })).into_response()
        }
        None => axum::Json(json!({ "success": 0, "error": "no matching metadata plugin found" }))
            .into_response(),
    }
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
                .add(lanrurugi_storage::download_queue::NewQueueItem {
                    origin: lanrurugi_storage::download_queue::QueueItemOrigin::Download,
                    url: item.url.clone(),
                    plugin_namespace: item.plugin_namespace.clone(),
                    file_size: None,
                    category: item.category.clone(),
                    auto_fetch_metadata: item.auto_fetch_metadata,
                    overwrite_on_duplicate: item.overwrite_on_duplicate,
                    state: DownloadQueueState::Queued,
                })
                .await
            {
                Ok(saved) => {
                    if let Some(tx) = &state.download_queue_tx {
                        let _ = tx.send(serde_json::json!({ "kind": "add", "item": saved }));
                    }
                    added.push(saved);
                }
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
        Ok(()) => {
            if let Some(tx) = &state.download_queue_tx {
                let _ = tx.send(serde_json::json!({
                    "kind": "update",
                    "id": &item.id,
                    "state": &item.state,
                    "job_id": &item.job_id,
                    "archive_ids": &item.archive_ids,
                    "title": &item.title,
                    "metadata_preview": &item.metadata_preview,
                    "error": &item.error,
                }));
            }
            axum::Json(json!({ "item": item })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_download_queue_item",
            e.to_string(),
        ),
    }
}

/// `DELETE /download_queue/{id}` — rejects an in-flight (`Starting`/`Downloading`) item: deleting
/// its Redis entry out from under the still-running background task would leave that task's later
/// `update_queue_item_state` calls silently no-op-ing (the item it's trying to update is just
/// gone), with no queue row left to show progress or a Stop button to actually interrupt the
/// transfer. The frontend's own delete button already disables for these states — this is the
/// server-side backstop for any caller that bypasses that (e.g. a direct API call).
async fn delete_queue_item(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let item = match state.download_queue.get(&id).await {
        Ok(Some(item)) if is_in_flight(item.state) => {
            return error(
                StatusCode::CONFLICT,
                "delete_download_queue_item",
                format!("Item {id} is currently downloading; stop it before deleting."),
            )
        }
        Ok(item) => item,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_download_queue_item",
                e.to_string(),
            )
        }
    };

    // A `pending_filename_conflict`'s staged bytes (`temp_dir/temp_{crc32}_{filename}`, see that
    // type's own docs) belong to this queue item alone — nothing else references them, and once
    // this item's own Redis record is gone there's no other path back to them short of the 24h
    // stale-file sweep eventually catching it. Deleting the item is the user's explicit call that
    // they're done with this conflict entirely, so it cleans up its own staged file immediately
    // rather than leaving an orphan for the periodic sweep to find later.
    if let Some(item) = &item {
        if let Some(conflict) = &item.pending_filename_conflict {
            if let Err(e) = tokio::fs::remove_file(&conflict.temp_path).await {
                tracing::warn!(%id, temp_path = %conflict.temp_path, error = %e, "failed to remove pending-rename temp file on queue item delete");
            }
            if let Err(e) = state.compare_cache.delete(&id).await {
                tracing::warn!(%id, error = %e, "failed to clear cached comparison on queue item delete");
            }
        }
    }

    match state.download_queue.delete(&id).await {
        Ok(()) => {
            if let Some(tx) = &state.download_queue_tx {
                let _ = tx.send(serde_json::json!({ "kind": "remove", "id": &id }));
            }
            ok("delete_download_queue_item", [])
        }
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
        Err(StartError::DuplicateInFlight) => error(
            StatusCode::CONFLICT,
            "start_download_queue_item",
            format!("Another item for the same URL as {id} is already downloading."),
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

#[derive(Debug, Deserialize)]
struct DeleteSelectedBody {
    ids: Vec<String>,
}

/// `POST /download_queue/delete_selected` — bulk delete by ID, mirroring `start_selected`'s own
/// shape/semantics (a missing/already-deleted ID is a silent no-op, not an error, so the frontend
/// can pass a stale selection without first re-checking it against the current list). An in-flight
/// (`Starting`/`Downloading`) ID is likewise a silent no-op here — the frontend's own selection UI
/// already excludes these (see `delete_queue_item`'s own docs on why deleting one out from under
/// its running download task is unsafe), so this only matters for a caller that bypasses it.
async fn delete_selected(
    State(state): State<AppState>,
    body: axum::Json<DeleteSelectedBody>,
) -> Response {
    let mut deleted = Vec::new();
    for id in &body.ids {
        let item = match state.download_queue.get(id).await {
            Ok(item) => item,
            Err(_) => continue,
        };
        if item.as_ref().is_some_and(|i| is_in_flight(i.state)) {
            continue;
        }
        // See `delete_queue_item`'s own docs on why this cleans up the staged file itself, not
        // just the Redis record.
        if let Some(conflict) = item
            .as_ref()
            .and_then(|i| i.pending_filename_conflict.as_ref())
        {
            if let Err(e) = tokio::fs::remove_file(&conflict.temp_path).await {
                tracing::warn!(%id, temp_path = %conflict.temp_path, error = %e, "failed to remove pending-rename temp file on queue item delete");
            }
        }
        if state.download_queue.delete(id).await.is_ok() {
            if let Some(tx) = &state.download_queue_tx {
                let _ = tx.send(serde_json::json!({ "kind": "remove", "id": id }));
            }
            deleted.push(id.clone());
        }
    }
    axum::Json(json!({ "deleted": deleted })).into_response()
}

enum StartError {
    NotFound,
    NotQueued,
    PluginMissing,
    Storage(String),
    DuplicateInFlight,
}

fn start_error_message(e: &StartError) -> String {
    match e {
        StartError::NotFound => "item not found".to_string(),
        StartError::NotQueued => "item is not in the Queued state".to_string(),
        StartError::PluginMissing => "plugin no longer installed".to_string(),
        StartError::Storage(msg) => msg.clone(),
        StartError::DuplicateInFlight => {
            "another item for this same URL is already downloading".to_string()
        }
    }
}

async fn start_one(state: &AppState, id: &str) -> Result<String, StartError> {
    let mut item = state
        .download_queue
        .get(id)
        .await
        .map_err(|e| StartError::Storage(e.to_string()))?
        .ok_or(StartError::NotFound)?;

    if !is_startable(item.state) {
        return Err(StartError::NotQueued);
    }

    // Dedup on the item's own fixed source `url` (the one the user actually added, chosen at
    // add-to-queue time) — deliberately NOT on whatever per-run resolved link a plugin's
    // `execDownload` happens to return this time (e.g. a CDN URL with a fresh signature/token),
    // since two legitimate, non-duplicate runs for the very same source can resolve to different
    // download links. Two separate queue items can share the same source `url` (e.g. added twice
    // by mistake, or a retry-after-error alongside a fresh add) — this only blocks starting a
    // second one while an earlier one for that same URL is actually in flight, not from existing
    // side by side in the queue.
    let all_items = state
        .download_queue
        .list_all()
        .await
        .map_err(|e| StartError::Storage(e.to_string()))?;
    if has_running_duplicate(&item, &all_items) {
        return Err(StartError::DuplicateInFlight);
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

/// `Queued`, `Error`, and `Cancelled` are all valid starting states — a failed download (e.g.
/// missing login credentials at the time it first ran) or a stopped one must be retryable once
/// the underlying problem is fixed / the user wants to try again, or the item would be
/// permanently stuck short of deleting and re-adding it from scratch.
fn is_startable(state: DownloadQueueState) -> bool {
    matches!(
        state,
        DownloadQueueState::Queued | DownloadQueueState::Error | DownloadQueueState::Cancelled
    )
}

/// True while a download-queue item has a live background task actually transferring bytes for
/// it — see `delete_queue_item`'s own docs for why these two states must not be deletable.
fn is_in_flight(state: DownloadQueueState) -> bool {
    matches!(
        state,
        DownloadQueueState::Starting | DownloadQueueState::Downloading
    )
}

/// True when some *other* item sharing `item`'s own source `url` is currently `Starting` or
/// `Downloading` — see `start_one`'s own docs for why this dedups on `url` specifically rather
/// than a plugin's per-run resolved download link.
fn has_running_duplicate(
    item: &lanrurugi_storage::download_queue::DownloadQueueItem,
    all_items: &[lanrurugi_storage::download_queue::DownloadQueueItem],
) -> bool {
    all_items
        .iter()
        .any(|other| other.id != item.id && other.url == item.url && is_in_flight(other.state))
}

/// `POST /download_queue/{id}/stop` — cancels an in-flight (`Starting`/`Downloading`) item's
/// download and reverts it to `Queued` so it can be restarted. Cooperative, not forceful: this
/// only signals `AppState::download_cancellations`' token for the item — the background task
/// itself notices at its next `tokio::select!` poll (see `download_manager::stream::download_one`)
/// and does its own partial-file cleanup before actually finishing, at which point `start_download`
/// reverts the queue item's state (see the `cancel_for_task.is_cancelled()` branch there). No
/// token present means either the item was never actually downloading (nothing to stop) or it
/// already finished on its own just before this request landed — both are `NotRunning`, not an
/// error worth surfacing loudly, since the desired end state ("this item isn't downloading") is
/// already true either way.
async fn stop_queue_item(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match stop_one(&state, &id).await {
        Ok(()) => axum::Json(json!({ "operation": "stop_download_queue_item", "success": 1 }))
            .into_response(),
        Err(StopError::NotFound) => {
            not_found("stop_download_queue_item", format!("Item {id} not found."))
        }
        Err(StopError::NotRunning) => error(
            StatusCode::CONFLICT,
            "stop_download_queue_item",
            format!("Item {id} is not currently downloading."),
        ),
        Err(StopError::Storage(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "stop_download_queue_item",
            e,
        ),
    }
}

enum StopError {
    NotFound,
    NotRunning,
    Storage(String),
}

async fn stop_one(state: &AppState, id: &str) -> Result<(), StopError> {
    let item = state
        .download_queue
        .get(id)
        .await
        .map_err(|e| StopError::Storage(e.to_string()))?
        .ok_or(StopError::NotFound)?;

    if !is_in_flight(item.state) {
        return Err(StopError::NotRunning);
    }

    let cancel = state.download_cancellations.lock().await.get(id).cloned();
    match cancel {
        Some(token) => {
            token.cancel();
            Ok(())
        }
        None => Err(StopError::NotRunning),
    }
}

/// `POST /download_queue/{id}/overwrite` — resolves a `PendingFilenameConflict` (see that type's
/// own docs) by deleting the archive that already owns the colliding filename and cataloguing the
/// staged download under its originally-intended filename.
#[derive(Debug, Deserialize)]
struct OverwriteQueueItemBody {
    /// B's own unique pages (issue #77's own follow-on design — `ComparisonResult
    /// .b_unmatched_pages`) the user chose to keep even though A is replacing B overall. Optional
    /// — omitted (or empty) for every ordinary overwrite that never went through the AI
    /// comparison flow at all, which is the overwhelming majority of calls to this endpoint.
    #[serde(default)]
    insertions: Vec<ExportPatchInsertion>,
}

/// `POST /download_queue/{id}/overwrite` — resolves a `PendingFilenameConflict` by deleting the
/// existing archive (B) and cataloguing the staged download (A) under its originally-intended
/// filename.
///
/// `body.insertions`, when given, packages B's own unique pages into a `.patch.zip` for the *new*
/// A archive (issue #77's own follow-on design: even when A wins overall, some of B's pages might
/// still be worth keeping) — this has to read B's pages and resolve B's own category/tag/progress
/// metadata *before* calling `resolve_conflict`, since that call deletes B's file and record as
/// part of cataloguing A (`pipeline.rs::delete_existing_archive`) — there's nothing left to read
/// from afterward.
async fn overwrite_queue_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: axum::Json<OverwriteQueueItemBody>,
) -> Response {
    let existing_before = match state.download_queue.get(&id).await {
        Ok(Some(item)) => match &item.pending_filename_conflict {
            Some(conflict) => state
                .repos
                .archives
                .get(&lanrurugi_core::ids::ArchiveId(
                    conflict.existing_id.clone(),
                ))
                .await
                .ok()
                .flatten()
                .map(|archive| (conflict.temp_path.clone(), archive)),
            None => None,
        },
        _ => None,
    };

    // Built from B's own pages BEFORE `resolve_conflict` runs — that call deletes B's file as
    // part of cataloguing A (`pipeline.rs::delete_existing_archive`), so B's pages must be read
    // and bundled into patch bytes now, held in memory, and only written to disk (under A's new
    // path) after A exists. Reading B's pages after the fact isn't an option: there is no "after
    // the fact" for B's own file.
    //
    // The patch's own `targetCrc32` is computed from the STAGED file (`a_temp_path`), not from
    // whatever A's file path ends up being post-catalogue — cataloguing moves/renames the file but
    // never transforms its bytes, so the staged file's crc32 already equals the final A's crc32.
    // Computing it any other way would need A to already exist, which — same ordering problem as
    // the page reads above — it doesn't yet at this point in the flow.
    let pre_built_patch = match &existing_before {
        Some((a_temp_path, b_before)) if !body.insertions.is_empty() => {
            let a_temp_path = std::path::PathBuf::from(a_temp_path);
            let b_path = std::path::PathBuf::from(&b_before.file);
            let insertions = body.insertions.clone();
            match tokio::task::spawn_blocking(move || {
                build_patch_zip_bytes(&a_temp_path, &b_path, insertions)
            })
            .await
            {
                Ok(Ok(bytes)) => Some(bytes),
                Ok(Err(e)) => {
                    tracing::warn!(%id, error = %e, "failed to build post-overwrite patch from B's pages");
                    None
                }
                Err(e) => {
                    tracing::warn!(%id, error = %e, "failed to build post-overwrite patch from B's pages");
                    None
                }
            }
        }
        _ => None,
    };

    match resolve_conflict(&state, &id, ResolveAction::Overwrite).await {
        Ok(archive_id) => {
            if let Some((_, b_before)) = &existing_before {
                migrate_archive_metadata(&state, b_before, &archive_id).await;
            }
            if let Some(bytes) = pre_built_patch {
                write_prebuilt_patch(&state, &archive_id, bytes).await;
            }
            axum::Json(
                json!({ "operation": "overwrite_download_queue_item", "success": 1, "archive_id": archive_id }),
            )
            .into_response()
        }
        Err(e) => resolve_error_response("overwrite_download_queue_item", &id, e),
    }
}

/// Extracts just the `rating:`-namespaced tag(s) from `tags` (this app's flat-tag convention has
/// no dedicated rating field — see [`migrate_archive_metadata`]'s own docs for why that tag alone
/// still needs to survive an overwrite even though the rest of `tags` deliberately doesn't).
fn extract_rating_tags(tags: &str) -> impl Iterator<Item = &str> {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter(|t| t.split(':').next().unwrap_or(t) == "rating")
}

/// Whether `tags` has any real content tag — i.e. excluding `date_added:`, which every freshly
/// catalogued archive carries regardless of whether any metadata plugin ever actually ran on it
/// successfully (see `pipeline.rs::catalogue_new_archive`), so its presence alone can't be used to
/// tell "A has real metadata" apart from "A is tag-less". Used by
/// [`migrate_archive_metadata`]'s own no-content-tags fallback.
fn has_any_content_tag(tags: &str) -> bool {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .any(|t| t.split(':').next().unwrap_or(t) != "date_added")
}

/// `from`'s tags with `source:`/`uploader:` stripped — used by [`migrate_archive_metadata`]'s
/// no-content-tags fallback: those two namespaces describe *where B itself was fetched from*, and
/// A (being kept) has its own real provenance, so falling back to B's content tags as better than
/// nothing must not also make A's record claim B's source/uploader as its own.
fn strip_source_and_uploader_tags(tags: &str) -> String {
    tags.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .filter(|t| {
            let namespace = t.split(':').next().unwrap_or(t);
            namespace != "source" && namespace != "uploader"
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Copies B's own user data onto the newly-catalogued A that replaced it — issue #77's own
/// follow-on design ("category或评分数据阅读进度呢？" — confirmed: migrate, don't lose it). Tags,
/// rating, and category membership are deliberately three SEPARATE decisions, not one blanket
/// "copy B's metadata" — confirmed live after reproducing a real regression: content tags
/// (Artist/Female/Male/Source/... — everything except `rating:`) must NOT be copied from B, since
/// the archive being KEPT (A, `to`) is the one whose own tags should win, whether freshly
/// re-scraped by `run_enabled_metadata_plugins_on_archive` (already applied by the time this runs
/// — see `overwrite_queue_item`'s own docs on call order) or otherwise already correct; blindly
/// stamping B's tags back on silently discarded whatever A's own metadata actually said. The one
/// exception: if A ended up with NO real content tags at all (metadata plugins disabled/failed —
/// see [`has_any_content_tag`]), B's content tags are used as a fallback (better than nothing) —
/// but with `source:`/`uploader:` stripped first ([`strip_source_and_uploader_tags`]), since those
/// two describe *where B itself came from*, not A's own real provenance. `rating:` is the one
/// tag-shaped exception even outside the fallback case — it's the user's own manual rating of
/// "this work" as a whole, not something either version's own metadata scrape produces, so it's
/// always merged in on top of A's own tags (never overwriting a rating A might already carry).
/// `summary` is likewise NOT copied, for the same reason as content tags (it's metadata about the
/// work, from the same plugin-or-otherwise source as tags, not user data). `lastreadpage`/
/// `lastreadtime`/`isnew` ARE copied straight — reading progress/new-status describe the reader's
/// own relationship to "this work", independent of which version's bytes are kept. `toc`/
/// `stamp_ids` are deliberately NOT copied — both are keyed by page position/entry name specific
/// to B's own page layout, which the insertions this same overwrite is applying may have just
/// changed, so carrying them over verbatim could point at the wrong pages in the new A;
/// regenerating/re-adding either is a separate, explicit user action, not an automatic one this
/// migration should silently guess at. Static category membership is migrated too (a category
/// search over `list_all()` is the only way to discover which categories held B — no reverse
/// index); a *dynamic* category needs no migration at all, since its own membership is
/// search-derived and will naturally re-include A once A's real tags/title exist. Best-effort
/// throughout (logged, never fails the overwrite itself) — matches every other post-resolve side
/// effect in this file (the category-add above, the search-index cleanup in
/// `archives.rs::delete_archive`).
async fn migrate_archive_metadata(
    state: &AppState,
    from: &lanrurugi_core::entities::Archive,
    to_id: &str,
) {
    let to_archive_id = lanrurugi_core::ids::ArchiveId(to_id.to_string());
    if let Ok(Some(mut to)) = state.repos.archives.get(&to_archive_id).await {
        if !has_any_content_tag(&to.tags) {
            let fallback = strip_source_and_uploader_tags(&from.tags);
            if !fallback.is_empty() {
                to.tags = if to.tags.is_empty() {
                    fallback
                } else {
                    format!("{},{fallback}", to.tags)
                };
            }
        }
        // Same shape as the category migration below: B's own rating tag(s), if any, are merged
        // into A's — deduped, not blindly appended, since a fresh A could in principle already
        // carry the same rating (e.g. re-scraped from a source that also encodes one).
        let mut merged: Vec<String> = to
            .tags
            .split(',')
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .collect();
        for rating in extract_rating_tags(&from.tags) {
            if !merged.iter().any(|t| t == rating) {
                merged.push(rating.to_string());
            }
        }
        to.tags = merged.join(",");
        to.lastreadpage = from.lastreadpage;
        to.lastreadtime = from.lastreadtime;
        to.isnew = from.isnew;
        if let Err(e) = state.repos.archives.save(&to).await {
            tracing::warn!(from = %from.id, to = %to_id, error = %e, "failed to migrate archive metadata after overwrite");
        }
    }

    match state.repos.categories.list_all().await {
        Ok(categories) => {
            for mut category in categories {
                if category.search.is_some() {
                    continue;
                }
                if category
                    .archives
                    .iter()
                    .any(|a| a.as_str() == from.id.as_str())
                {
                    category.archives.push(to_archive_id.clone());
                    if let Err(e) = state.repos.categories.save(&category).await {
                        tracing::warn!(from = %from.id, to = %to_id, category = %category.catid, error = %e, "failed to migrate category membership after overwrite");
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(from = %from.id, to = %to_id, error = %e, "failed to list categories while migrating membership after overwrite");
        }
    }
}

/// Writes already-built patch bytes (from [`overwrite_queue_item`]'s own pre-`resolve_conflict`
/// build — see that function's docs for why the bytes have to be built before this point rather
/// than read fresh here) to the newly-catalogued A's own sidecar `.patch.zip` path. Resolves A's
/// real file path fresh via a repository lookup rather than assuming anything about where
/// `resolve_conflict` put it.
async fn write_prebuilt_patch(state: &AppState, to_id: &str, bytes: Vec<u8>) {
    let to_archive_id = lanrurugi_core::ids::ArchiveId(to_id.to_string());
    let Ok(Some(mut to)) = state.repos.archives.get(&to_archive_id).await else {
        tracing::warn!(to = %to_id, "cannot write post-overwrite patch: new archive record not found");
        return;
    };
    let patch_path = lanrurugi_scanner::patch::patch_path_for(std::path::Path::new(&to.file));
    if let Err(e) = tokio::fs::write(&patch_path, &bytes).await {
        tracing::warn!(to = %to_id, error = %e, "failed to write post-overwrite patch");
        return;
    }
    // Keeps the library grid's own patch badge (issue #77's own follow-on design) in sync — best-
    // effort, matching every other side effect in this file; a failure here just means the badge
    // lags until something else next re-saves this archive's record.
    to.has_patch = true;
    if let Err(e) = state.repos.archives.save(&to).await {
        tracing::warn!(to = %to_id, error = %e, "failed to mark archive has_patch after writing post-overwrite patch");
    }
}

#[derive(Debug, Deserialize)]
struct RenameQueueItemBody {
    filename: String,
}

/// `POST /download_queue/{id}/rename` — resolves a `PendingFilenameConflict` by cataloguing the
/// staged download under `body.filename` instead, leaving the existing archive that owns the
/// original filename untouched (a fresh, separate, coexisting archive). If `filename` itself also
/// collides with some other archive, the item ends up with a *new* `PendingFilenameConflict`
/// (still resolvable, just against a different existing archive/filename) rather than losing the
/// already-downloaded bytes — see `ingest::resolve_rename`'s own docs.
async fn rename_queue_item(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: axum::Json<RenameQueueItemBody>,
) -> Response {
    match resolve_conflict(&state, &id, ResolveAction::Rename(body.filename.clone())).await {
        Ok(archive_id) => axum::Json(
            json!({ "operation": "rename_download_queue_item", "success": 1, "archive_id": archive_id }),
        )
        .into_response(),
        Err(e) => resolve_error_response("rename_download_queue_item", &id, e),
    }
}

/// The real on-disk paths + display metadata behind a `PendingFilenameConflict`'s two sides —
/// shared by [`compare_queue_item`] and [`compare_queue_item_stream`] so the lookup/validation
/// logic (queue item exists, has a pending conflict, the existing archive it conflicts with still
/// exists, the staged download is still on disk) lives in exactly one place.
struct ResolvedCompareConflict {
    a_path: std::path::PathBuf,
    b_path: std::path::PathBuf,
    a_filename: String,
    a_file_size: u64,
    b_filename: String,
    b_file_size: u64,
}

/// Either a resolved conflict ready to compare, or a terminal [`Response`] (a normal JSON error —
/// callers turn this into the right shape for their own transport, an SSE handler wrapping it in
/// one `error` event before closing the stream, same as [`compare_queue_item`]'s own plain-JSON
/// error responses).
enum ResolveOutcome {
    Ready(ResolvedCompareConflict),
    Err(Response),
}

async fn resolve_compare_conflict(state: &AppState, id: &str) -> ResolveOutcome {
    let item = match state.download_queue.get(id).await {
        Ok(Some(item)) => item,
        Ok(None) => {
            return ResolveOutcome::Err(not_found(
                "compare_download_queue_item",
                format!("{id} does not exist."),
            ))
        }
        Err(e) => {
            return ResolveOutcome::Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compare_download_queue_item",
                e.to_string(),
            ))
        }
    };
    let Some(conflict) = item.pending_filename_conflict else {
        return ResolveOutcome::Err(error(
            StatusCode::BAD_REQUEST,
            "compare_download_queue_item",
            "This queue item has no pending filename conflict to compare.",
        ));
    };

    let existing_archive = match state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(
            conflict.existing_id.clone(),
        ))
        .await
    {
        Ok(Some(archive)) => archive,
        Ok(None) => {
            return ResolveOutcome::Err(not_found(
                "compare_download_queue_item",
                format!(
                    "{} (the existing archive this conflict is against) no longer exists.",
                    conflict.existing_id
                ),
            ))
        }
        Err(e) => {
            return ResolveOutcome::Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compare_download_queue_item",
                e.to_string(),
            ))
        }
    };

    let a_path = std::path::PathBuf::from(&conflict.temp_path);
    let b_path = std::path::PathBuf::from(&existing_archive.file);
    let a_filename = conflict.original_filename.clone();
    // The existing archive's own on-disk filename (with extension), not `Archive.name` (that
    // field is explicitly the decoded name *without* extension, and would read oddly next to
    // `a_filename`, which is a real filename including one) — matches what the user actually
    // sees when browsing their filesystem.
    let b_filename = std::path::Path::new(&existing_archive.file)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| existing_archive.name.clone());
    let b_file_size = existing_archive.arcsize;
    let a_file_size = match tokio::fs::metadata(&a_path).await {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // A real, distinguishable case (not just "some I/O error") — the staged download this
            // conflict points at is gone from disk (e.g. the periodic temp-file sweep reclaimed
            // it, or — in a dev container — the whole writable layer got wiped on a rebuild/
            // recreate). Reported live: the previous generic 500 surfaced as an undifferentiated
            // "Comparison failed" toast with no way to tell this apart from a real bug. A distinct
            // `error` string here lets the frontend show "re-download and try again" instead.
            return ResolveOutcome::Err(error(
                StatusCode::BAD_REQUEST,
                "compare_download_queue_item",
                format!(
                    "The staged download for this conflict no longer exists on disk ({}) — it may have been cleared by cleanup. Try downloading again to compare.",
                    a_path.display()
                ),
            ));
        }
        Err(e) => {
            return ResolveOutcome::Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compare_download_queue_item",
                e.to_string(),
            ))
        }
    };

    ResolveOutcome::Ready(ResolvedCompareConflict {
        a_path,
        b_path,
        a_filename,
        a_file_size,
        b_filename,
        b_file_size,
    })
}

/// `POST /download_queue/{id}/compare` — issue #77's AI quality-comparison judgment for a
/// `PendingFilenameConflict`: page-aligns and sharpness-compares the freshly-staged download
/// (`temp_path`, exposed to the response as [`imgcompare::Side::A`]) against the already-cataloged
/// archive that owns the colliding filename (`existing_id`'s own file, exposed as
/// [`imgcompare::Side::B`]) — read-only, resolves nothing on its own; the frontend still calls
/// `.../overwrite` or `.../rename` afterward based on what the user (with or without this
/// comparison's help) decides. `A`/`B` are deliberately not labeled "new"/"old" in the response
/// itself (see [`lanrurugi_imgcompare::Side`]'s own docs) — this handler is what actually assigns
/// that meaning for this specific caller.
///
/// Kept alongside the streaming [`compare_queue_item_stream`] below (not replaced by it) as a
/// plain request/response fallback for any caller that doesn't want SSE — the frontend's own
/// `ComparisonResultModal` flow now goes through the stream endpoint instead (see that handler's
/// own docs for why).
async fn compare_queue_item(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    // Re-opening a comparison the user already ran should be instant, not re-run the whole
    // perceptual-hash/DP-alignment pipeline from scratch — reported live: "每次打开都要等待". The
    // cached value is invalidated (deleted) the moment this conflict is actually resolved
    // (`overwrite_queue_item`/`keep_side_b`/`rename_queue_item`) or the queue item itself is
    // deleted, so a hit here is always for the *same* still-pending conflict, never stale data.
    if let Ok(Some(cached_json)) = state.compare_cache.get(&id).await {
        if let Ok(result) =
            serde_json::from_str::<lanrurugi_imgcompare::ComparisonResult>(&cached_json)
        {
            return axum::Json(json!({ "result": result })).into_response();
        }
    }

    let resolved = match resolve_compare_conflict(&state, &id).await {
        ResolveOutcome::Ready(resolved) => resolved,
        ResolveOutcome::Err(response) => return response,
    };

    // CPU/IO-bound (archive decompression + per-page image decode + perceptual hashing) — run off
    // the async reactor, same reasoning `tankoubon_grouping.rs`'s own `spawn_blocking` use for its
    // batch embedding work.
    let result = match tokio::task::spawn_blocking(move || {
        lanrurugi_imgcompare::compare_archives(
            &resolved.a_path,
            &resolved.b_path,
            resolved.a_filename,
            resolved.a_file_size,
            resolved.b_filename,
            resolved.b_file_size,
        )
    })
    .await
    {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compare_download_queue_item",
                e.to_string(),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compare_download_queue_item",
                e.to_string(),
            )
        }
    };

    if let Ok(result_json) = serde_json::to_string(&result) {
        if let Err(e) = state.compare_cache.put(&id, &result_json).await {
            tracing::warn!(%id, error = %e, "failed to cache comparison result");
        }
    }

    axum::Json(json!({ "result": result })).into_response()
}

/// `GET /download_queue/{id}/compare/stream` — the SSE-streaming twin of [`compare_queue_item`]
/// above, per issue #77's own confirmed two-phase design (see
/// `lanrurugi_imgcompare::compare_archives_streaming`'s own docs for the full phase-1/phase-2
/// breakdown): opens an `EventSource`-compatible stream (GET, not POST — `EventSource` can't send
/// a request body or use any other HTTP method) that emits one `event: sample` per
/// `CompareEvent::Sample` (coarse first, precise replacements streamed in afterward) and a single
/// terminal `event: done` carrying the rest of `ComparisonResult`'s own fields — never relies on
/// connection-close/timeout as an implicit end signal ("并且sse要有结束标记"). The frontend opens
/// its result view the moment the FIRST `sample` event arrives rather than waiting for the whole
/// comparison, which for a ~200-page archive pair is otherwise several seconds of dead time
/// ("假设有6对结果（粗加工），可以用sse分6次发，发第一对的时候就开始显示modal，这样是一个很大的提速").
///
/// A cache hit (same policy as `compare_queue_item`'s own) is replayed as a single burst of
/// `sample` events (`phase: coarse` for all of them — a cached result has already gone through
/// both phases, and the frontend only cares about "is this a display-final value", which `Coarse`
/// vs `Precise` doesn't actually change once the value itself is final) immediately followed by
/// `done`, so re-opening an already-computed comparison is still effectively instant.
async fn compare_queue_item_stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures_util::stream;

    // A complete cache hit replays straight from the cached result — no real work to redo. An
    // `incomplete` one (a prior run that got cancelled mid-stream) has no `Done` summary to replay,
    // so it only seeds `resume_samples` below and the real pipeline still runs — see this
    // function's own `resume_samples` handling for why that's still a net win (the frontend sees
    // the old samples immediately, then the real run's own fresh events replace them in place).
    let mut resume_samples: Vec<axum::response::sse::Event> = Vec::new();
    if let Ok(Some(cached_json)) = state.compare_cache.get(&id).await {
        if let Ok(result) =
            serde_json::from_str::<lanrurugi_imgcompare::ComparisonResult>(&cached_json)
        {
            if result.incomplete {
                resume_samples = sample_events(&result.samples);
            } else {
                let events = cached_result_to_events(result);
                let stream =
                    stream::iter(events.into_iter().map(Ok::<_, std::convert::Infallible>));
                return Sse::new(stream)
                    .keep_alive(KeepAlive::default())
                    .into_response();
            }
        }
    }

    let resolved = match resolve_compare_conflict(&state, &id).await {
        ResolveOutcome::Ready(resolved) => resolved,
        ResolveOutcome::Err(response) => return response,
    };

    // `compare_archives_streaming` is synchronous and calls `on_event` from whatever thread runs
    // it — bridged to the async SSE stream via an unbounded channel + `spawn_blocking`, same
    // reasoning `compare_queue_item`'s own `spawn_blocking` use has (CPU/IO-bound work must not
    // run on the async reactor), just with incremental sends instead of one final value.
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    // Seed the resumed samples from an `incomplete` cache hit (see above) before the real pipeline
    // below sends anything — the frontend opens its modal on the first `sample` event, so this
    // shows the last known result immediately while the real run recomputes it in the background.
    for event in resume_samples {
        let _ = tx.send(event);
    }
    let cache = state.compare_cache.clone();
    // Cloned before `compare_archives_streaming` moves the originals — needed for the placeholder
    // fields of an `incomplete` cache entry if the run gets cancelled (see that branch below).
    let a_filename_for_cache = resolved.a_filename.clone();
    let b_filename_for_cache = resolved.b_filename.clone();
    let a_file_size_for_cache = resolved.a_file_size;
    let b_file_size_for_cache = resolved.b_file_size;
    tokio::task::spawn_blocking(move || {
        let mut collected_samples: Vec<Option<lanrurugi_imgcompare::PageComparison>> = Vec::new();
        let mut done_result: Option<lanrurugi_imgcompare::ComparisonResult> = None;
        let run_result = lanrurugi_imgcompare::compare_archives_streaming(
            &resolved.a_path,
            &resolved.b_path,
            resolved.a_filename,
            resolved.a_file_size,
            resolved.b_filename,
            resolved.b_file_size,
            |event| {
                // Mirror every event into `collected_samples`/`done_result` so the whole run can
                // still be assembled into one `ComparisonResult` for `compare_cache` at the end —
                // the SSE stream and the cache must never drift apart, or a cache hit on the next
                // open would silently serve a different (stale-shaped) result than what streaming
                // just showed.
                match &event {
                    lanrurugi_imgcompare::CompareEvent::Sample {
                        sample_index,
                        sample,
                        ..
                    } => {
                        if collected_samples.len() <= *sample_index {
                            collected_samples.resize(*sample_index + 1, None);
                        }
                        collected_samples[*sample_index] = Some(sample.clone());
                    }
                    lanrurugi_imgcompare::CompareEvent::Done {
                        aligned_pairs,
                        a_total_pages,
                        b_total_pages,
                        a_entries,
                        b_entries,
                        likely_different_language,
                        recommendation,
                        a_filename,
                        b_filename,
                        a_file_size,
                        b_file_size,
                        a_unmatched_pages,
                        b_unmatched_pages,
                    } => {
                        done_result = Some(lanrurugi_imgcompare::ComparisonResult {
                            aligned_pairs: *aligned_pairs,
                            a_total_pages: *a_total_pages,
                            b_total_pages: *b_total_pages,
                            a_entries: a_entries.clone(),
                            b_entries: b_entries.clone(),
                            likely_different_language: *likely_different_language,
                            recommendation: *recommendation,
                            samples: collected_samples.iter().flatten().cloned().collect(),
                            a_filename: a_filename.clone(),
                            b_filename: b_filename.clone(),
                            a_file_size: *a_file_size,
                            b_file_size: *b_file_size,
                            a_unmatched_pages: a_unmatched_pages.clone(),
                            b_unmatched_pages: b_unmatched_pages.clone(),
                            incomplete: false,
                        });
                    }
                }
                if let Ok(json) = serde_json::to_string(&event) {
                    let sse_event = match &event {
                        lanrurugi_imgcompare::CompareEvent::Sample { .. } => {
                            Event::default().event("sample").data(json)
                        }
                        lanrurugi_imgcompare::CompareEvent::Done { .. } => {
                            Event::default().event("done").data(json)
                        }
                    };
                    // `Err` means `rx`/`UnboundedReceiverStream` was already dropped — the client
                    // disconnected (e.g. the comparison modal was closed mid-stream) and no one is
                    // reading the channel anymore. Returning `false` here lets
                    // `compare_archives_streaming` stop sampling further events instead of running
                    // the rest of a multi-second CPU-bound comparison to completion for nobody —
                    // confirmed live: closing the modal mid-stream previously left this
                    // `spawn_blocking` task running unabated, tying up a blocking-pool thread.
                    tx.send(sse_event).is_ok()
                } else {
                    true
                }
            },
        );

        match run_result {
            Ok(()) => {
                if let Some(result) = done_result {
                    if let Ok(result_json) = serde_json::to_string(&result) {
                        // `compare_cache.put` is async; this closure runs on a blocking thread, so
                        // hand off to a `tokio::spawn`ed task rather than blocking here on an
                        // async call.
                        tokio::spawn(async move {
                            if let Err(e) = cache.put(&id, &result_json).await {
                                tracing::warn!(%id, error = %e, "failed to cache streamed comparison result");
                            }
                        });
                    }
                }
            }
            // The client disconnected mid-stream — cache whatever coarse/precise samples already
            // arrived (`incomplete: true`) so re-opening this comparison can show them immediately
            // instead of starting from a blank modal, even though the full pipeline still has to
            // re-run to produce a real `Done` summary (see this crate's own `incomplete` field docs
            // for why: the expensive stages run once over the whole page set regardless of sample
            // count, so there's no cheaper way to finish this run than to redo it).
            Err(lanrurugi_imgcompare::ImgCompareError::Cancelled) => {
                tracing::debug!(%id, "comparison stream cancelled — client disconnected");
                if collected_samples.iter().any(Option::is_some) {
                    let partial = lanrurugi_imgcompare::ComparisonResult {
                        aligned_pairs: 0,
                        a_total_pages: 0,
                        b_total_pages: 0,
                        a_entries: Vec::new(),
                        b_entries: Vec::new(),
                        likely_different_language: false,
                        recommendation: None,
                        samples: collected_samples.into_iter().flatten().collect(),
                        a_filename: a_filename_for_cache,
                        b_filename: b_filename_for_cache,
                        a_file_size: a_file_size_for_cache,
                        b_file_size: b_file_size_for_cache,
                        a_unmatched_pages: Vec::new(),
                        b_unmatched_pages: Vec::new(),
                        incomplete: true,
                    };
                    if let Ok(partial_json) = serde_json::to_string(&partial) {
                        tokio::spawn(async move {
                            if let Err(e) = cache.put(&id, &partial_json).await {
                                tracing::warn!(%id, error = %e, "failed to cache partial comparison result");
                            }
                        });
                    }
                }
            }
            Err(e) => {
                let _ = tx.send(Event::default().event("error").data(e.to_string()));
            }
        }
    });

    let stream = futures_util::StreamExt::map(
        tokio_stream::wrappers::UnboundedReceiverStream::new(rx),
        Ok::<_, std::convert::Infallible>,
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

/// One `sample` SSE event per entry, all tagged `Coarse` — a cached result (complete or partial)
/// has no meaningful coarse/precise distinction left, only "this is the last known value".
fn sample_events(
    samples: &[lanrurugi_imgcompare::PageComparison],
) -> Vec<axum::response::sse::Event> {
    use axum::response::sse::Event;

    let mut events = Vec::with_capacity(samples.len());
    for (sample_index, sample) in samples.iter().enumerate() {
        let event = lanrurugi_imgcompare::CompareEvent::Sample {
            sample_index,
            phase: lanrurugi_imgcompare::ComparePhase::Coarse,
            sample: sample.clone(),
        };
        if let Ok(json) = serde_json::to_string(&event) {
            events.push(Event::default().event("sample").data(json));
        }
    }
    events
}

/// Replays an already-cached [`lanrurugi_imgcompare::ComparisonResult`] as the same `sample`/`done`
/// SSE event sequence a fresh streaming run would produce.
fn cached_result_to_events(
    result: lanrurugi_imgcompare::ComparisonResult,
) -> Vec<axum::response::sse::Event> {
    use axum::response::sse::Event;

    let mut events = sample_events(&result.samples);
    let done_event = lanrurugi_imgcompare::CompareEvent::Done {
        aligned_pairs: result.aligned_pairs,
        a_total_pages: result.a_total_pages,
        b_total_pages: result.b_total_pages,
        a_entries: result.a_entries,
        b_entries: result.b_entries,
        likely_different_language: result.likely_different_language,
        recommendation: result.recommendation,
        a_filename: result.a_filename,
        b_filename: result.b_filename,
        a_file_size: result.a_file_size,
        b_file_size: result.b_file_size,
        a_unmatched_pages: result.a_unmatched_pages,
        b_unmatched_pages: result.b_unmatched_pages,
    };
    if let Ok(json) = serde_json::to_string(&done_event) {
        events.push(Event::default().event("done").data(json));
    }
    events
}

/// Resolves `side` ("a"/"b") to the real on-disk path it reads from — `"a"` is the still-
/// uncatalogued staged download (`conflict.temp_path`, no archive ID exists yet), `"b"` is the
/// existing library archive this conflict is against. Shared by every `.../compare/*` endpoint
/// that needs one side's own archive path (`compare_queue_item_page`, [`list_compare_pages`]),
/// pulled out once those grew to two identical copies of the same match arm.
async fn resolve_compare_side_path(
    state: &AppState,
    conflict: &lanrurugi_storage::download_queue::PendingFilenameConflict,
    side: &str,
) -> std::result::Result<std::path::PathBuf, Response> {
    match side {
        "a" => Ok(std::path::PathBuf::from(&conflict.temp_path)),
        "b" => match state
            .repos
            .archives
            .get(&lanrurugi_core::ids::ArchiveId(
                conflict.existing_id.clone(),
            ))
            .await
        {
            Ok(Some(archive)) => Ok(std::path::PathBuf::from(archive.file)),
            Ok(None) => Err(not_found(
                "resolve_compare_side_path",
                format!("{} no longer exists.", conflict.existing_id),
            )),
            Err(e) => Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "resolve_compare_side_path",
                e.to_string(),
            )),
        },
        other => Err(error(
            StatusCode::BAD_REQUEST,
            "resolve_compare_side_path",
            format!("side must be 'a' or 'b', got {other:?}"),
        )),
    }
}

#[derive(Debug, Deserialize)]
struct CompareQueueItemPageParams {
    side: String,
    index: usize,
}

/// `GET /download_queue/{id}/compare/page?side=a|b&index=N` — the actual page image behind one
/// `PageComparison` sample from `.../compare`'s own response, for the frontend's
/// overlay/shift-to-toggle comparison view. `side=a` reads from the still-uncatalogued staged
/// download (`temp_path` — no archive ID exists yet, so this can't reuse `archives.rs`'s own
/// `GET /archives/{id}/page`); `side=b` reads from the existing archive's own file. `index` is a
/// natural-sort page position (matching `lanrurugi_imgcompare::PageComparison`'s own
/// `a_page_index`/`b_page_index`), resolved to an entry name via `list_pages` fresh on every call
/// rather than cached — this endpoint is only ever hit a handful of times per conflict (issue #77's
/// own confirmed sample size of 3-5), so re-listing isn't worth adding a cache for.
async fn compare_queue_item_page(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<CompareQueueItemPageParams>,
) -> Response {
    let item = match state.download_queue.get(&id).await {
        Ok(Some(item)) => item,
        Ok(None) => {
            return not_found(
                "compare_download_queue_item_page",
                format!("{id} does not exist."),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "compare_download_queue_item_page",
                e.to_string(),
            )
        }
    };
    let Some(conflict) = item.pending_filename_conflict else {
        return error(
            StatusCode::BAD_REQUEST,
            "compare_download_queue_item_page",
            "This queue item has no pending filename conflict to compare.",
        );
    };

    let archive_path = match resolve_compare_side_path(&state, &conflict, &params.side).await {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    let index = params.index;
    let result = tokio::task::spawn_blocking(move || {
        let pages = lanrurugi_scanner::archive_format::list_pages(&archive_path)?;
        let entry = pages.get(index).ok_or_else(|| {
            lanrurugi_scanner::archive_format::ArchiveFormatError::EntryNotFound(format!(
                "page index {index}"
            ))
        })?;
        lanrurugi_scanner::archive_format::read_entry(&archive_path, entry)
    })
    .await;

    match result {
        Ok(Ok(bytes)) => {
            let content_type = crate::archives::image_content_type(&bytes);
            // No `Cache-Control` at all meant the browser re-fetched this same page's bytes over
            // the network every single time anything (StrictMode double-invoke, a re-render that
            // recreates the preload `<img>`, a plain page reload) asked for it again — including,
            // confirmed live via a DevTools network capture, races with the padded-side preload
            // `<img>` in `OverlayPage.tsx` that made the user's first Shift press land while a
            // redundant fetch was still in flight, reading as UI jank even though the actual
            // render-side preload logic was already correct. `immutable` is safe: `index`/`side`
            // are stable coordinates into an already-downloaded/cataloged archive, and this
            // conflict's `compare_cache` entry (and the queue item itself) is invalidated the
            // moment the conflict is actually resolved — the URL itself goes away, it never starts
            // serving different bytes at the same URL.
            (
                [
                    (axum::http::header::CONTENT_TYPE, content_type),
                    (
                        axum::http::header::CACHE_CONTROL,
                        "private, max-age=86400, immutable",
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(Err(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "compare_download_queue_item_page",
            e.to_string(),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "compare_download_queue_item_page",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct ListComparePagesParams {
    side: String,
}

/// `GET /download_queue/{id}/compare/pages?side=a|b` — the real entry-name list for one side of
/// the comparison, in natural-sort (page) order. The frontend's own "pick where to insert an
/// unmatched A page" UI (issue #77's own follow-on design) needs B's real entry names to send as
/// `after_filename`/`before_filename` in `.../export-patch` — it otherwise only ever deals in
/// numeric page indices (matching `PageComparison.a_page_index`/`b_page_index`), which
/// `patch.rs`'s own JSON schema deliberately doesn't use (see that module's docs on why an entry
/// name is the stable anchor, not an index that can drift if the archive is re-scanned).
async fn list_compare_pages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<ListComparePagesParams>,
) -> Response {
    let item = match state.download_queue.get(&id).await {
        Ok(Some(item)) => item,
        Ok(None) => return not_found("list_compare_pages", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "list_compare_pages",
                e.to_string(),
            )
        }
    };
    let Some(conflict) = item.pending_filename_conflict else {
        return error(
            StatusCode::BAD_REQUEST,
            "list_compare_pages",
            "This queue item has no pending filename conflict to compare.",
        );
    };
    let archive_path = match resolve_compare_side_path(&state, &conflict, &params.side).await {
        Ok(path) => path,
        Err(resp) => return resp,
    };

    let result = tokio::task::spawn_blocking(move || {
        lanrurugi_scanner::archive_format::list_pages(&archive_path)
    })
    .await;

    match result {
        Ok(Ok(pages)) => axum::Json(json!({ "pages": pages })).into_response(),
        Ok(Err(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_compare_pages",
            e.to_string(),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_compare_pages",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct ExportPatchInsertion {
    #[serde(default)]
    after_filename: Option<String>,
    #[serde(default)]
    before_filename: Option<String>,
    /// The *source* side's own page indices (matching `PageComparison.a_page_index`/
    /// `b_page_index`, or `ComparisonResult.a_unmatched_pages`/`b_unmatched_pages`) to insert at
    /// this anchor, in the order the user wants them to appear. Which side is "source" depends on
    /// the caller — [`export_compare_patch`] always reads from A (its own doc comment covers why);
    /// [`keep_side_b`] reads from A too (A's unique pages get patched onto the B it's keeping).
    page_indices: Vec<usize>,
}

#[derive(Debug, Deserialize)]
struct ExportPatchRequest {
    insertions: Vec<ExportPatchInsertion>,
}

/// Builds a `.patch.zip`'s raw bytes for `target_path`, bundling pages read from `source_path` at
/// the indices/anchors `insertions` describe. Shared by [`export_compare_patch`] (returns the
/// bytes for the user to download and place themselves) and [`keep_side_b`] (writes the same
/// bytes straight to `target_path`'s own sidecar path) — both need the exact same
/// crc32-target + read-source-pages + `patch::build_patch_zip` sequence, just differing in what
/// happens to the result afterward.
fn build_patch_zip_bytes(
    target_path: &std::path::Path,
    source_path: &std::path::Path,
    insertions: Vec<ExportPatchInsertion>,
) -> std::result::Result<Vec<u8>, lanrurugi_scanner::archive_format::ArchiveFormatError> {
    let target_crc32 = lanrurugi_scanner::patch::crc32_of_file(target_path)
        .map_err(lanrurugi_scanner::archive_format::ArchiveFormatError::from)?;
    let source_crc32 = lanrurugi_scanner::patch::crc32_of_file(source_path).ok();
    let source_pages = lanrurugi_scanner::archive_format::list_pages(source_path)?;

    let mut bundled_pages: Vec<(String, Vec<u8>)> = Vec::new();
    let mut patch_insertions = Vec::new();
    for insertion in insertions {
        let mut files = Vec::new();
        for index in insertion.page_indices {
            let Some(entry_name) = source_pages.get(index) else {
                continue;
            };
            let bytes = lanrurugi_scanner::archive_format::read_entry(source_path, entry_name)?;
            // Bundled under a name distinct from the source's own entry name (issue #77's own
            // confirmed design: a patch's own image filenames are a separate namespace from
            // either source archive's — see `lanrurugi_scanner::patch`'s own module docs) so a
            // collision with an existing target entry name, or between two patches from different
            // sessions, can't happen.
            let patch_entry_name = format!("patch_{:04}_{}", bundled_pages.len() + 1, entry_name);
            files.push(patch_entry_name.clone());
            bundled_pages.push((patch_entry_name, bytes));
        }
        if files.is_empty() {
            continue;
        }
        patch_insertions.push(lanrurugi_scanner::patch::PatchInsertion {
            after_filename: insertion.after_filename,
            before_filename: insertion.before_filename,
            files,
        });
    }

    lanrurugi_scanner::patch::build_patch_zip(
        &target_crc32,
        source_crc32.as_deref(),
        &bundled_pages,
        patch_insertions,
    )
    .map_err(|e| lanrurugi_scanner::archive_format::ArchiveFormatError::Libarchive(e.to_string()))
}

/// `POST /download_queue/{id}/compare/export-patch` — builds a `.patch.zip` from a user-picked
/// selection of A's own unique pages (`ComparisonResult.a_unmatched_pages`, issue #77's own
/// follow-on design) and returns its raw bytes for download, ready to be dropped next to the
/// target archive on disk as `<archive filename>.patch.zip` (`lanrurugi_scanner::patch`) — this
/// endpoint doesn't write it there itself (confirmed design: "补丁不需要应用，只要放在漫画目录内"
/// — a user-placed file, not one this server installs unprompted). The target is always B (the
/// existing library archive this conflict is against) — a patch only ever adds A's own extra
/// pages on top of the version the user is keeping, never the other way around.
async fn export_compare_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<ExportPatchRequest>,
) -> Response {
    let item = match state.download_queue.get(&id).await {
        Ok(Some(item)) => item,
        Ok(None) => return not_found("export_compare_patch", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "export_compare_patch",
                e.to_string(),
            )
        }
    };
    let Some(conflict) = item.pending_filename_conflict else {
        return error(
            StatusCode::BAD_REQUEST,
            "export_compare_patch",
            "This queue item has no pending filename conflict to compare.",
        );
    };
    let existing_archive = match state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(
            conflict.existing_id.clone(),
        ))
        .await
    {
        Ok(Some(archive)) => archive,
        Ok(None) => {
            return not_found(
                "export_compare_patch",
                format!("{} no longer exists.", conflict.existing_id),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "export_compare_patch",
                e.to_string(),
            )
        }
    };

    let a_path = std::path::PathBuf::from(&conflict.temp_path);
    let b_path = std::path::PathBuf::from(&existing_archive.file);
    let result = tokio::task::spawn_blocking(move || {
        build_patch_zip_bytes(&b_path, &a_path, body.insertions)
    })
    .await;

    match result {
        Ok(Ok(bytes)) => {
            let filename = format!(
                "{}.patch.zip",
                std::path::Path::new(&existing_archive.file)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| existing_archive.name.clone())
            );
            (
                [
                    (
                        axum::http::header::CONTENT_TYPE,
                        "application/zip".to_string(),
                    ),
                    (
                        axum::http::header::CONTENT_DISPOSITION,
                        format!("attachment; filename=\"{filename}\""),
                    ),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(Err(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "export_compare_patch",
            e.to_string(),
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "export_compare_patch",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct KeepSideBRequest {
    #[serde(default)]
    insertions: Vec<ExportPatchInsertion>,
}

/// `POST /download_queue/{id}/compare/keep-b` — the "keep the existing library archive, discard
/// this download" resolution (issue #77's own follow-on design). Distinct from every other
/// resolve action in this file: those all catalogue the *staged* file (A) as some archive, either
/// replacing B (`overwrite_queue_item`) or coexisting with it (`rename_queue_item`) — this one
/// catalogues nothing at all. B is already in the library and stays exactly where it is; the only
/// two things that happen are (1) `insertions`, if any, get written straight to B's own sidecar
/// `.patch.zip` (not just returned for the user to place themselves, unlike
/// [`export_compare_patch`] — this flow's own frontend already walked the user through picking
/// insertion points as part of *choosing* to keep B, so there's no separate "now go place this
/// file" step left for them to do), and (2) the queue item is deleted outright, same as a plain
/// `DELETE /download_queue/{id}` — there's nothing left for it to track (confirmed design: a
/// patch's own existence lives on disk next to B, not as a queue-item flag).
async fn keep_side_b(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<KeepSideBRequest>,
) -> Response {
    let item = match state.download_queue.get(&id).await {
        Ok(Some(item)) => item,
        Ok(None) => return not_found("keep_side_b", format!("{id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "keep_side_b",
                e.to_string(),
            )
        }
    };
    let Some(conflict) = item.pending_filename_conflict else {
        return error(
            StatusCode::BAD_REQUEST,
            "keep_side_b",
            "This queue item has no pending filename conflict to resolve.",
        );
    };
    let existing_archive = match state
        .repos
        .archives
        .get(&lanrurugi_core::ids::ArchiveId(
            conflict.existing_id.clone(),
        ))
        .await
    {
        Ok(Some(archive)) => archive,
        Ok(None) => {
            return not_found(
                "keep_side_b",
                format!("{} no longer exists.", conflict.existing_id),
            )
        }
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "keep_side_b",
                e.to_string(),
            )
        }
    };

    if !body.insertions.is_empty() {
        let a_path = std::path::PathBuf::from(&conflict.temp_path);
        let b_path = std::path::PathBuf::from(&existing_archive.file);
        let patch_path = lanrurugi_scanner::patch::patch_path_for(&b_path);
        let write_result = tokio::task::spawn_blocking(move || {
            let bytes = build_patch_zip_bytes(&b_path, &a_path, body.insertions)?;
            std::fs::write(&patch_path, &bytes)
                .map_err(lanrurugi_scanner::archive_format::ArchiveFormatError::from)
        })
        .await;
        match write_result {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "keep_side_b",
                    e.to_string(),
                )
            }
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "keep_side_b",
                    e.to_string(),
                )
            }
        }
        // Keeps the library grid's own patch badge (issue #77's own follow-on design) in sync —
        // best-effort, same reasoning as `write_prebuilt_patch`'s own identical update.
        let mut updated = existing_archive.clone();
        updated.has_patch = true;
        if let Err(e) = state.repos.archives.save(&updated).await {
            tracing::warn!(%id, error = %e, "failed to mark archive has_patch after keep_side_b");
        }
    }

    // Discard the staged download — B is being kept as-is, so the temp file A sat in has nothing
    // left to become. Best-effort: a failure here shouldn't block the resolution itself (the
    // periodic temp-file sweep — same one `WatcherError`'s own module docs mention — will
    // eventually reclaim it if this leaves it behind).
    if let Err(e) = tokio::fs::remove_file(&conflict.temp_path).await {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(%id, path = %conflict.temp_path, error = %e, "failed to remove discarded staged download");
        }
    }
    if let Err(e) = state.compare_cache.delete(&id).await {
        tracing::warn!(%id, error = %e, "failed to clear cached comparison after keep_side_b");
    }

    match state.download_queue.delete(&id).await {
        Ok(()) => axum::Json(json!({ "operation": "keep_side_b", "success": 1 })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "keep_side_b",
            e.to_string(),
        ),
    }
}

enum ResolveAction {
    Overwrite,
    Rename(String),
}

enum ResolveConflictError {
    NotFound,
    NoConflict,
    Storage(String),
    /// The re-attempted ingest itself failed (e.g. `NoConflict`'s opposite — there *was* a
    /// conflict, but resolving it hit some other error) — plain-text summary only; the real
    /// structured detail is already persisted onto the item's own `error` field by
    /// `resolve_conflict` before this is returned, exactly like every other download-pipeline
    /// failure in this app (the frontend picks it up via its next `GET /download_queue` poll,
    /// not from this response body).
    Ingest(String),
}

/// Shared by both resolve endpoints — looks up the item, requires a live
/// `pending_filename_conflict`, delegates the actual (re-)cataloguing to `ingest::resolve_overwrite`/
/// `ingest::resolve_rename`, then updates the item to reflect the outcome: `Done` + cleared
/// `error`/`pending_filename_conflict` on success, or a refreshed `error` (state back to `Error`)
/// on failure — mirroring `plugins.rs::start_download`'s own generic error-recording convention,
/// just without a `JobRegistry` job involved (there's no new job for a resolve action; it reuses
/// the original download's already-staged bytes rather than re-downloading).
async fn resolve_conflict(
    state: &AppState,
    id: &str,
    action: ResolveAction,
) -> Result<String, ResolveConflictError> {
    let item = state
        .download_queue
        .get(id)
        .await
        .map_err(|e| ResolveConflictError::Storage(e.to_string()))?
        .ok_or(ResolveConflictError::NotFound)?;

    let conflict = item
        .pending_filename_conflict
        .clone()
        .ok_or(ResolveConflictError::NoConflict)?;

    // A `LocalUpload` item's `url` field is just the uploaded file's own name (see
    // `upload.rs::upload_archive`'s `NewQueueItem` construction), never a real external source —
    // stamping it into `source:` would be the same fabricated-data bug `ingest_downloaded_file`'s
    // own initial call already avoids for uploads (see that call site's docs).
    let source_url = match item.origin {
        QueueItemOrigin::Download => Some(item.url.as_str()),
        QueueItemOrigin::LocalUpload => None,
    };
    let result = match &action {
        ResolveAction::Overwrite => {
            crate::download_manager::ingest::resolve_overwrite(state, &conflict, source_url).await
        }
        ResolveAction::Rename(new_filename) => {
            crate::download_manager::ingest::resolve_rename(
                state,
                &conflict,
                new_filename,
                source_url,
                Some(id),
            )
            .await
        }
    };

    match result {
        Ok(ingested) => {
            if let Some(catid) = &item.category {
                let _ =
                    crate::categories::add_archive_to_category(state, catid, &ingested.archive_id)
                        .await;
            }
            if let Ok(Some(mut fresh)) = state.download_queue.get(id).await {
                // Cache metadata and apply tags before Done — this is the same path
                // `plugins.rs`'s regular download uses (see `ensure_metadata_cached`),
                // just reached through a different entry point (rename/overwrite vs.
                // start-pipeline). The archive already exists at this point (catalogue
                // succeeded above) so `archive_ids` is set first so the tag-applier
                // can find it.
                fresh.archive_ids = Some(vec![ingested.archive_id.clone()]);
                if fresh.auto_fetch_metadata || fresh.title.is_none() {
                    crate::plugins::ensure_metadata_cached(state, &mut fresh).await;
                }
                fresh.state = DownloadQueueState::Done;
                fresh.error = None;
                fresh.pending_filename_conflict = None;
                if let Err(e) = state.download_queue.update(&fresh).await {
                    tracing::warn!(%id, error = %e, "failed to update download-queue item after resolving filename conflict");
                } else if let Some(tx) = &state.download_queue_tx {
                    let _ = tx.send(serde_json::json!({
                        "kind": "update",
                        "id": id,
                        "state": "done",
                        "job_id": &fresh.job_id,
                        "archive_ids": &fresh.archive_ids,
                        "title": &fresh.title,
                        "metadata_preview": &fresh.metadata_preview,
                        "error": null,
                    }));
                }
            }
            // The conflict this cached comparison was for no longer exists (resolved either way —
            // Overwrite or Rename) — the cache entry has nothing left to be re-opened against.
            // Best-effort, matching every other cleanup side effect in this function.
            if let Err(e) = state.compare_cache.delete(id).await {
                tracing::warn!(%id, error = %e, "failed to clear cached comparison after resolving filename conflict");
            }
            Ok(ingested.archive_id)
        }
        Err(e) => {
            let queue_error = lanrurugi_core::queue_error::QueueError::from(&e);
            // A repeat `Filename` collision under `ResolveAction::Rename` already re-persisted a
            // *fresh* `pending_filename_conflict` onto the item itself (see
            // `ingest::stage_pending_rename`'s own docs) — re-fetch rather than reusing the
            // in-memory `item` from above so this only touches `state`/`error`/(conditionally)
            // `pending_filename_conflict`, never stomping that fresh conflict info with a stale
            // copy. Any *other* error — most importantly a `ContentHash` collision, which can
            // surface here if the staged bytes turn out to be byte-identical to some archive the
            // library already has under a different name (confirmed live: renaming around a
            // `Filename` collision doesn't change what the content itself hashes to) — is
            // unconditionally unresolvable by renaming again, so the stale `pending_filename_conflict`
            // from the *original* conflict must be cleared here too; leaving it would keep
            // offering "rename and catalog" for a conflict no filename could ever fix.
            let is_fresh_filename_conflict = matches!(
                queue_error,
                lanrurugi_core::queue_error::QueueError::DuplicateFilename { .. }
            );
            if let Ok(Some(mut fresh)) = state.download_queue.get(id).await {
                fresh.state = DownloadQueueState::Error;
                fresh.error = Some(queue_error.clone());
                if !is_fresh_filename_conflict {
                    fresh.pending_filename_conflict = None;
                }
                let _ = state.download_queue.update(&fresh).await;
            }
            Err(ResolveConflictError::Ingest(format!("{e}")))
        }
    }
}

fn resolve_error_response(operation: &str, id: &str, e: ResolveConflictError) -> Response {
    match e {
        ResolveConflictError::NotFound => not_found(operation, format!("Item {id} not found.")),
        ResolveConflictError::NoConflict => error(
            StatusCode::CONFLICT,
            operation,
            format!("Item {id} has no pending filename conflict to resolve."),
        ),
        ResolveConflictError::Storage(msg) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, operation, msg)
        }
        ResolveConflictError::Ingest(msg) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, operation, msg)
        }
    }
}

/// `POST /download_queue/clear_completed` — deletes every `Done` item, returns the count removed.
/// Deliberately excludes `Error` — unlike `Done`, an errored item is a restartable, still-
/// actionable state (grouped with `Queued`/`Cancelled` everywhere else in this module: selectable,
/// has a retry button, shows its own error text), not "completed" work a user is done looking at.
/// A user wanting to discard a specific failed item can still do so individually via
/// `DELETE /download_queue/{id}` (which does clean up a `pending_filename_conflict`'s own staged
/// temp file — `Done` items never carry one, so this handler has nothing analogous to worry
/// about).
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
        .filter(|i| i.state == DownloadQueueState::Done)
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
    if let Some(tx) = &state.download_queue_tx {
        for id in &ids {
            let _ = tx.send(serde_json::json!({ "kind": "remove", "id": id }));
        }
    }
    ok(
        "clear_completed_download_queue",
        [("cleared", json!(cleared))],
    )
}

#[cfg(test)]
mod tests {
    use lanrurugi_storage::download_queue::DownloadQueueItem;

    use super::*;

    fn item(id: &str, url: &str, state: DownloadQueueState) -> DownloadQueueItem {
        DownloadQueueItem {
            id: id.to_string(),
            origin: lanrurugi_storage::download_queue::QueueItemOrigin::Download,
            url: url.to_string(),
            plugin_namespace: "download/ehentai".to_string(),
            file_size: None,
            category: None,
            auto_fetch_metadata: false,
            overwrite_on_duplicate: false,
            state,
            job_id: None,
            archive_ids: None,
            title: None,
            metadata_preview: None,
            metadata_preview_at: None,
            error: None,
            pending_filename_conflict: None,
            created_at: 0,
        }
    }

    #[test]
    fn blocks_starting_when_another_item_with_the_same_url_is_downloading() {
        let target = item(
            "a",
            "https://example.com/gallery/1",
            DownloadQueueState::Queued,
        );
        let all = vec![
            target.clone(),
            item(
                "b",
                "https://example.com/gallery/1",
                DownloadQueueState::Downloading,
            ),
        ];
        assert!(has_running_duplicate(&target, &all));
    }

    #[test]
    fn blocks_starting_when_another_item_with_the_same_url_is_starting() {
        let target = item(
            "a",
            "https://example.com/gallery/1",
            DownloadQueueState::Queued,
        );
        let all = vec![
            target.clone(),
            item(
                "b",
                "https://example.com/gallery/1",
                DownloadQueueState::Starting,
            ),
        ];
        assert!(has_running_duplicate(&target, &all));
    }

    #[test]
    fn allows_starting_when_no_other_item_shares_the_url() {
        let target = item(
            "a",
            "https://example.com/gallery/1",
            DownloadQueueState::Queued,
        );
        let all = vec![
            target.clone(),
            item(
                "b",
                "https://example.com/gallery/2",
                DownloadQueueState::Downloading,
            ),
        ];
        assert!(!has_running_duplicate(&target, &all));
    }

    #[test]
    fn extract_rating_tags_returns_only_the_rating_namespace() {
        let got: Vec<&str> =
            extract_rating_tags("artist:someone, rating:4.5, category:manga").collect();
        assert_eq!(got, vec!["rating:4.5"]);
    }

    #[test]
    fn extract_rating_tags_is_empty_when_no_rating_tag_is_present() {
        let got: Vec<&str> = extract_rating_tags("artist:someone,category:manga").collect();
        assert!(got.is_empty());
    }

    #[test]
    fn extract_rating_tags_does_not_match_a_similarly_named_namespace() {
        // A namespace like `top_rating:` or a bare tag containing the substring "rating" must
        // NOT be treated as the rating tag — same edge case `tankoubons.rs::strip_rating_tag`'s
        // own tests already guard against for its (inverse) use case.
        let got: Vec<&str> = extract_rating_tags("top_rating:5,rating_note:high").collect();
        assert!(got.is_empty());
    }

    #[test]
    fn has_any_content_tag_is_false_for_empty_and_date_added_only_tags() {
        assert!(!has_any_content_tag(""));
        assert!(!has_any_content_tag("date_added:1738000000"));
    }

    #[test]
    fn has_any_content_tag_is_true_when_a_real_tag_is_present() {
        assert!(has_any_content_tag("date_added:1738000000,artist:someone"));
        assert!(has_any_content_tag("category:manga"));
    }

    #[test]
    fn strip_source_and_uploader_tags_removes_only_those_two_namespaces() {
        let got = strip_source_and_uploader_tags(
            "artist:someone,source:e-hentai.org/g/1/a,uploader:someone,category:manga",
        );
        assert_eq!(got, "artist:someone,category:manga");
    }

    #[test]
    fn strip_source_and_uploader_tags_is_a_noop_when_neither_is_present() {
        let got = strip_source_and_uploader_tags("artist:someone,category:manga");
        assert_eq!(got, "artist:someone,category:manga");
    }

    #[test]
    fn allows_starting_when_the_same_url_exists_but_is_not_actually_running() {
        let target = item(
            "a",
            "https://example.com/gallery/1",
            DownloadQueueState::Queued,
        );
        let all = vec![
            target.clone(),
            item(
                "b",
                "https://example.com/gallery/1",
                DownloadQueueState::Done,
            ),
            item(
                "c",
                "https://example.com/gallery/1",
                DownloadQueueState::Error,
            ),
            item(
                "d",
                "https://example.com/gallery/1",
                DownloadQueueState::Queued,
            ),
            item(
                "e",
                "https://example.com/gallery/1",
                DownloadQueueState::Cancelled,
            ),
        ];
        assert!(!has_running_duplicate(&target, &all));
    }

    #[test]
    fn does_not_count_the_item_itself_as_a_duplicate() {
        let target = item(
            "a",
            "https://example.com/gallery/1",
            DownloadQueueState::Starting,
        );
        let all = vec![target.clone()];
        assert!(!has_running_duplicate(&target, &all));
    }

    #[test]
    fn queued_error_and_cancelled_are_startable() {
        assert!(is_startable(DownloadQueueState::Queued));
        assert!(is_startable(DownloadQueueState::Error));
        assert!(is_startable(DownloadQueueState::Cancelled));
    }

    #[test]
    fn starting_downloading_and_done_are_not_startable() {
        assert!(!is_startable(DownloadQueueState::Starting));
        assert!(!is_startable(DownloadQueueState::Downloading));
        assert!(!is_startable(DownloadQueueState::Done));
    }

    #[test]
    fn starting_and_downloading_are_in_flight() {
        assert!(is_in_flight(DownloadQueueState::Starting));
        assert!(is_in_flight(DownloadQueueState::Downloading));
    }

    #[test]
    fn queued_error_cancelled_and_done_are_not_in_flight() {
        assert!(!is_in_flight(DownloadQueueState::Queued));
        assert!(!is_in_flight(DownloadQueueState::Error));
        assert!(!is_in_flight(DownloadQueueState::Cancelled));
        assert!(!is_in_flight(DownloadQueueState::Done));
    }
}
