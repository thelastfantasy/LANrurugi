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
        .route("/download_queue/{id}/stop", post(stop_queue_item))
        .route("/download_queue/{id}/overwrite", post(overwrite_queue_item))
        .route("/download_queue/{id}/rename", post(rename_queue_item))
        .route("/download_queue/start_all", post(start_all))
        .route("/download_queue/start_selected", post(start_selected))
        .route("/download_queue/delete_selected", post(delete_selected))
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
        }
    }

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
async fn overwrite_queue_item(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match resolve_conflict(&state, &id, ResolveAction::Overwrite).await {
        Ok(archive_id) => axum::Json(
            json!({ "operation": "overwrite_download_queue_item", "success": 1, "archive_id": archive_id }),
        )
        .into_response(),
        Err(e) => resolve_error_response("overwrite_download_queue_item", &id, e),
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

    let result = match &action {
        ResolveAction::Overwrite => {
            crate::download_manager::ingest::resolve_overwrite(state, &conflict, &item.url).await
        }
        ResolveAction::Rename(new_filename) => {
            crate::download_manager::ingest::resolve_rename(
                state,
                &conflict,
                new_filename,
                &item.url,
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
                fresh.state = DownloadQueueState::Done;
                fresh.error = None;
                fresh.pending_filename_conflict = None;
                if let Err(e) = state.download_queue.update(&fresh).await {
                    tracing::warn!(%id, error = %e, "failed to update download-queue item after resolving filename conflict");
                }
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
            url: url.to_string(),
            plugin_namespace: "download/ehentai".to_string(),
            category: None,
            auto_fetch_metadata: false,
            overwrite_on_duplicate: false,
            state,
            job_id: None,
            title: None,
            metadata_preview: None,
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
