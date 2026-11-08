//! `/download_queue*` — the Upload page's persistent, plugin-grouped download queue (additive,
//! no legacy equivalent). Backed by [`lanrurugi_storage::download_queue::DownloadQueueRepository`]
//! so a queued/in-progress item survives a page refresh or a different browser tab; starting an
//! item reuses `plugins::start_download`'s exact dispatch/execute/ingest sequence via a
//! `pub(crate)` re-export, keeping this module's own responsibility to CRUD + orchestrating which
//! items to start.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::Router;
use futures_util::StreamExt;
use lanrurugi_plugin::protocol::PluginInfo;
use lanrurugi_storage::download_queue::DownloadQueueState;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found, ok};
use crate::plugins::{get_plugin_customargs, named_plugin_parameters, plugin_activity_fields};
use crate::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};

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
        .route("/download_queue/{id}/pause", post(pause_queue_item))
        .route(
            "/download_queue/{id}/confirm-older",
            post(confirm_older_revision),
        )
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
                Ok(mut event) => {
                    // `event` is already the flat `{ "kind": "update"/"add"/"remove", "id"/"item":
                    // ..., ... }` object every broadcast site (`update_queue_item_state`, the
                    // `add`/`remove` sites in this file and `upload.rs`) sends — the frontend's
                    // `DownloadQueueDelta` type reads `data.kind`/`data.item`/`data.id` at the
                    // *top* level (see `useDownloadQueue`'s SSE handler), so `type` must be merged
                    // into this same object, not wrapped around it in a new `item` key. Wrapping
                    // it (the previous shape here) put `kind` at `event.item.kind` instead of
                    // `event.kind`, so `switch (data.kind)` never matched any real case and every
                    // delta was silently dropped until the next `full` refetch — confirmed live: a
                    // metadata-preview update landed in Redis and broadcast correctly, but the
                    // Upload page's tooltip/title never updated without a manual page reload.
                    if let serde_json::Value::Object(ref mut map) = event {
                        map.insert("type".to_string(), serde_json::json!("delta"));
                    }
                    let sse_event: Event = Event::default()
                        .event("delta")
                        .data(serde_json::to_string(&event).unwrap_or_default());
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
    #[serde(default)]
    metadata_tags: Vec<String>,
    auto_fetch_metadata: bool,
    overwrite_on_duplicate: bool,
}

#[derive(Debug, Deserialize)]
struct AddQueueBody {
    items: Vec<AddQueueItem>,
}

/// Builds the `after` payload shared by `download_queue.add` and `download_queue.start` — the
/// download plugin that owns the URL plus every execution/queue setting actually chosen for it, so
/// the Activity detail view can show *which plugin with which settings* rather than a bare URL.
/// `plugin` is `None` for the start-request side (querying `plugin_info` there would spawn a
/// throwaway Deno subprocess just to enrich an audit entry — `plugins::start_download` patches the
/// resolved name/namespace/type onto the same entry moments later); `plugin_parameters` is likewise
/// `None` until those customargs have actually been resolved. Optional fields are omitted rather
/// than written as `null`, so the detail view can tell "not known yet" apart from "known empty".
#[allow(clippy::too_many_arguments)]
fn download_activity_payload(
    url: &str,
    plugin_namespace: &str,
    plugin: Option<&PluginInfo>,
    plugin_parameters: Option<Value>,
    category: Option<&str>,
    auto_fetch_metadata: bool,
    overwrite_on_duplicate: bool,
    title: Option<&str>,
    metadata: Option<&Value>,
) -> Value {
    let mut payload = serde_json::Map::new();
    payload.insert("url".to_string(), json!(url));
    payload.insert(
        "plugin".to_string(),
        plugin
            .map(plugin_activity_fields)
            .unwrap_or_else(|| json!({ "namespace": plugin_namespace })),
    );
    payload.insert("plugin_namespace".to_string(), json!(plugin_namespace));
    let mut parameters = serde_json::Map::new();
    parameters.insert("category".to_string(), json!(category));
    if let Some(plugin_parameters) = plugin_parameters {
        parameters.insert("plugin_parameters".to_string(), plugin_parameters);
    }
    payload.insert(
        "execution_parameters".to_string(),
        Value::Object(parameters),
    );
    payload.insert(
        "auto_fetch_metadata".to_string(),
        json!(auto_fetch_metadata),
    );
    payload.insert(
        "overwrite_on_duplicate".to_string(),
        json!(overwrite_on_duplicate),
    );
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        payload.insert("title".to_string(), json!(title));
    }
    if let Some(metadata) = metadata {
        payload.insert("metadata".to_string(), metadata.clone());
    }
    Value::Object(payload)
}

/// `POST /download_queue` — bulk add. The client has already resolved which download plugin's
/// `url_pattern` matched each URL and computed the two checkbox defaults; this handler's only
/// validation is that `plugin_namespace` actually resolves to an installed plugin — invalid
/// entries are rejected individually (`rejected: [{url, reason}]`) rather than failing the whole
/// batch, so one bad URL in a pasted list doesn't block the rest.
async fn add_to_queue(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    body: axum::Json<AddQueueBody>,
) -> Response {
    let mut added = Vec::new();
    let mut rejected = Vec::new();
    // Per *plugin*, not per URL. `plugin_info` spawns a throwaway Deno subprocess every call, so a
    // 310-URL paste used to start 310 of them (plus 310 Redis reads for the custom args) before the
    // first item was even queued — the server half of why that paste took so long. The client used
    // to amplify it further with three lookups per line; that half is fixed too.
    let mut resolved: HashMap<
        String,
        Option<(lanrurugi_plugin::protocol::PluginInfo, Vec<Value>)>,
    > = HashMap::new();

    for item in &body.items {
        let key = item.plugin_namespace.clone();
        if !resolved.contains_key(&key) {
            let entry = match state.plugins.plugin_info(&key).await {
                Ok(info) => {
                    let args = get_plugin_customargs(&state, &key, &info.parameters).await;
                    Some((info, args))
                }
                Err(_) => None,
            };
            resolved.insert(key.clone(), entry);
        }
        let Some(Some((info, plugin_parameters))) = resolved.get(&key).cloned() else {
            rejected.push(json!({
                "url": item.url,
                "reason": format!("no installed plugin under namespace {:?}", item.plugin_namespace),
            }));
            continue;
        };
        // Resolved once, up front, so both arms below record it: the plugin's declared parameter
        // names paired with the user's saved values are this download's real execution parameters,
        // and they're identical regardless of whether the Redis write that follows succeeds.
        let plugin_parameters = named_plugin_parameters(&info, &plugin_parameters);
        match state
            .download_queue
            .add(lanrurugi_storage::download_queue::NewQueueItem {
                // Started by the user directly, so it belongs to no subscription.
                subscription_id: None,
                origin: lanrurugi_storage::download_queue::QueueItemOrigin::Download,
                url: item.url.clone(),
                plugin_namespace: item.plugin_namespace.clone(),
                file_size: None,
                category: item.category.clone(),
                metadata_tags: item.metadata_tags.clone(),
                auto_fetch_metadata: item.auto_fetch_metadata,
                overwrite_on_duplicate: item.overwrite_on_duplicate,
                // A manual add answers its own conflicts in the UI, like it always has.
                conflict_policy: lanrurugi_storage::subscriptions::ConflictPolicy::Ask,
                state: DownloadQueueState::Queued,
            })
            .await
        {
            Ok(saved) => {
                if let Some(tx) = &state.download_queue_tx {
                    let _ = tx.send(serde_json::json!({ "kind": "add", "item": saved }));
                }
                // `title`/`metadata` are always absent this early (no metadata fetched yet at
                // add time), so `label` is just the url — but `after` still carries the plugin
                // and every execution/queue setting so the record is useful on its own, and the
                // frontend renders every `download_queue.*` entry the same way regardless of
                // which point in the item's lifecycle wrote it.
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::DOWNLOAD_QUEUE_ADD,
                    ActivityTarget {
                        id: Some(saved.id.clone()),
                        label: Some(saved.url.clone()),
                        kind: Some("download_queue_item".to_string()),
                    },
                    Outcome::Success,
                    None,
                    Some(download_activity_payload(
                        &saved.url,
                        &saved.plugin_namespace,
                        Some(&info),
                        Some(plugin_parameters.clone()),
                        saved.category.as_deref(),
                        saved.auto_fetch_metadata,
                        saved.overwrite_on_duplicate,
                        saved.title.as_deref(),
                        saved.metadata_preview.as_ref(),
                    )),
                )
                .await;
                added.push(saved);
            }
            Err(e) => {
                // A real attempted-and-failed add (Redis write error), not a precondition
                // rejection — the plugin_namespace already resolved fine above.
                record_manual(
                    &state,
                    auth.as_ref().map(|e| &e.0),
                    action_types::DOWNLOAD_QUEUE_ADD,
                    ActivityTarget {
                        id: None,
                        label: Some(item.url.clone()),
                        kind: Some("download_queue_item".to_string()),
                    },
                    Outcome::Failure {
                        reason: e.to_string(),
                    },
                    None,
                    Some(download_activity_payload(
                        &item.url,
                        &item.plugin_namespace,
                        Some(&info),
                        Some(plugin_parameters),
                        item.category.as_deref(),
                        item.auto_fetch_metadata,
                        item.overwrite_on_duplicate,
                        None,
                        None,
                    )),
                )
                .await;
                rejected.push(json!({ "url": item.url, "reason": e.to_string() }));
            }
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
async fn delete_queue_item(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
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
    // a stale-file sweep eventually catching it. Deleting the item is the user's explicit call
    // that they're done with this row entirely, so it removes its own staged file immediately.
    if let Some(item) = &item {
        if let Some(conflict) = &item.pending_filename_conflict {
            if let Err(e) = tokio::fs::remove_file(&conflict.temp_path).await {
                tracing::warn!(%id, temp_path = %conflict.temp_path, error = %e, "failed to remove pending-rename temp file on queue item delete");
            }
        }
    }

    match state.download_queue.delete(&id).await {
        Ok(()) => {
            if let Some(tx) = &state.download_queue_tx {
                let _ = tx.send(serde_json::json!({ "kind": "remove", "id": &id }));
            }
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::DOWNLOAD_QUEUE_DELETE,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: item.as_ref().map(|i| i.url.clone()),
                    kind: Some("download_queue_item".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            ok("delete_download_queue_item", [])
        }
        Err(e) => {
            // A real Redis delete failure on an item that passed the in-flight check above —
            // worth recording since the user's delete action genuinely didn't take effect.
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::DOWNLOAD_QUEUE_DELETE,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: item.as_ref().map(|i| i.url.clone()),
                    kind: Some("download_queue_item".to_string()),
                },
                Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_download_queue_item",
                e.to_string(),
            )
        }
    }
}

/// `POST /download_queue/{id}/start` — resolves the item's own (already-fixed, chosen at add
/// time) `plugin_namespace`, marks it `Starting`, and launches the real download via
/// `plugins::start_download` with `queue_link` set so the background task keeps this item's
/// `state`/`job_id`/`error` current as the download proceeds.
async fn start_queue_item(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    // Written *before* `start_one` (which launches the real background download/ingest task),
    // not after — `activity_entry_id` is threaded through `start_one`/`plugins::start_download`/
    // `run_managed_downloads` so that background task can patch this same entry's own `after`
    // once ingestion actually finishes (`activity::patch_after`), rather than this handler trying
    // to record the *result* of work that hasn't happened yet by the time it returns. Recording
    // this early means the entry exists even if `start_one` itself then fails (e.g. `NotQueued`)
    // — matches every other write-then-maybe-fail site in this file already accepting a record
    // for an attempt, not just a guaranteed success.
    //
    // `label` prefers the item's own already-fetched `title` (from an earlier metadata preview)
    // when one exists, falling back to the raw `url` otherwise — unlike an archive/tankoubon/
    // token there's no per-item detail page `targetLink` can send the user to
    // (`download_queue_item` links to the whole upload/queue page), so a title alone wouldn't let
    // two different downloads be told apart. `after.url` is set unconditionally alongside it so
    // the frontend can always show the real source link even when a title is also present
    // (`OperationDescription.tsx`'s own docs).
    let item = state.download_queue.get(&id).await.ok().flatten();
    let item_for_activity = item.clone();
    let label = item
        .as_ref()
        .map(|i| i.title.clone().unwrap_or_else(|| i.url.clone()));
    let activity_entry_id = record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DOWNLOAD_QUEUE_START,
        ActivityTarget {
            id: Some(id.clone()),
            label,
            kind: Some("download_queue_item".to_string()),
        },
        // Written before the attempt runs, same as `patch_after` can only enrich `after` later,
        // not flip this to `Failure` — the real download outcome (network/plugin/ingest failure
        // inside the background task `start_one` spawns) isn't observable from here yet. The
        // plugin's resolved name and actual execution parameters (customargs) are patched onto
        // this same entry by `plugins::start_download` once it has them; see
        // `download_activity_payload`'s own docs.
        Outcome::Success,
        None,
        item_for_activity.as_ref().map(|i| {
            // A retry (`state != Queued`) clears the previous run's title/metadata preview inside
            // `start_one` before this attempt begins — recording those stale values here would
            // say the new run is using metadata it has actually already discarded.
            let cached = i.state == DownloadQueueState::Queued;
            download_activity_payload(
                &i.url,
                &i.plugin_namespace,
                None,
                None,
                i.category.as_deref(),
                i.auto_fetch_metadata,
                i.overwrite_on_duplicate,
                if cached { i.title.as_deref() } else { None },
                if cached {
                    i.metadata_preview.as_ref()
                } else {
                    None
                },
            )
        }),
    )
    .await;
    match start_one(
        &state,
        auth.as_ref().map(|e| &e.0),
        &id,
        activity_entry_id,
        false,
    )
    .await
    {
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
        Err(StartError::Storage(e)) => {
            // `start_one` itself hit a real Redis failure marking the item `Starting` — the
            // download attempt never even got to spawn, worth recording since the earlier
            // `Success` entry above would otherwise misleadingly stand alone as the only record.
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::DOWNLOAD_QUEUE_START,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: None,
                    kind: Some("download_queue_item".to_string()),
                },
                Outcome::Failure { reason: e.clone() },
                None,
                None,
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "start_download_queue_item",
                e,
            )
        }
    }
}

/// Pushes one item's current state over the queue SSE stream, so an open Upload page reflects a
/// change made outside the normal in-flight update path without waiting for a refetch.
pub(crate) fn broadcast_item_update(
    state: &AppState,
    item: &lanrurugi_storage::download_queue::DownloadQueueItem,
) {
    let Some(tx) = &state.download_queue_tx else {
        return;
    };
    let _ = tx.send(json!({
        "kind": "update",
        "id": item.id,
        "state": item.state,
        "job_id": item.job_id,
        "error": item.error,
        "pending_revision_confirmation": item.pending_revision_confirmation,
        "title": item.title,
        "metadata_preview": item.metadata_preview,
    }));
}

#[derive(Debug, serde::Deserialize)]
pub struct ConfirmOlderRevisionBody {
    /// `true` = download it anyway, `false` = abandon it (issue #107). No default: this endpoint
    /// exists precisely to capture a decision, so a body that states none is a client bug, not a
    /// case to guess at.
    proceed: bool,
}

/// `POST /download_queue/{id}/confirm-older` — answers the
/// `AwaitingRevisionConfirmation` prompt raised when a download turned out to be an *older*
/// revision than an archive already in the library (issue #107's `warn_then_conflict_menu`).
///
/// `proceed: true` records the decision on the item and restarts it — the restart runs straight
/// through the same version-history check instead of parking again, because `revision_confirmed`
/// is now set. `proceed: false` moves the item to `Cancelled`, which is already the "user stopped
/// this on purpose, still restartable" state, so no new terminal state is needed.
///
/// Deliberately separate from `.../start`: `AwaitingRevisionConfirmation` is not `is_startable`,
/// so the ordinary Start button can't skip the question by accident.
async fn confirm_older_revision(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<ConfirmOlderRevisionBody>,
) -> Response {
    let mut item = match state.download_queue.get(&id).await {
        Ok(Some(item)) => item,
        Ok(None) => return not_found("confirm_older_revision", format!("Item {id} not found.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "confirm_older_revision",
                e.to_string(),
            )
        }
    };

    if item.state != DownloadQueueState::AwaitingRevisionConfirmation {
        return error(
            StatusCode::CONFLICT,
            "confirm_older_revision",
            format!("Item {id} is not awaiting revision confirmation."),
        );
    }

    let newer_archive_id = item
        .pending_revision_confirmation
        .as_ref()
        .map(|c| c.newer_archive_id.clone());
    item.pending_revision_confirmation = None;

    if !body.proceed {
        item.state = DownloadQueueState::Cancelled;
        item.revision_confirmed = false;
        if let Err(e) = state.download_queue.update(&item).await {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "confirm_older_revision",
                e.to_string(),
            );
        }
        broadcast_item_update(&state, &item);
        // Abandoning is a stop, not a start — the item ends in `Cancelled`, exactly where
        // `stop_one` leaves one.
        record_manual(
            &state,
            auth.as_ref().map(|e| &e.0),
            action_types::DOWNLOAD_QUEUE_STOP,
            ActivityTarget {
                id: Some(id.clone()),
                label: item.title.clone().or_else(|| Some(item.url.clone())),
                kind: Some("download_queue_item".to_string()),
            },
            Outcome::Success,
            None,
            Some(json!({ "revision_confirmation": "abandoned", "newer_archive_id": newer_archive_id })),
        )
        .await;
        return axum::Json(
            json!({ "operation": "confirm_older_revision", "success": 1, "proceeded": false }),
        )
        .into_response();
    }

    // Recorded before the restart, so the decision survives even if `start_one` then fails — the
    // same write-then-maybe-fail ordering `start_queue_item` already uses for its activity entry.
    item.revision_confirmed = true;
    item.state = DownloadQueueState::Queued;
    if let Err(e) = state.download_queue.update(&item).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "confirm_older_revision",
            e.to_string(),
        );
    }
    broadcast_item_update(&state, &item);

    let activity_entry_id = record_manual(
        &state,
        auth.as_ref().map(|e| &e.0),
        action_types::DOWNLOAD_QUEUE_START,
        ActivityTarget {
            id: Some(id.clone()),
            label: item.title.clone().or_else(|| Some(item.url.clone())),
            kind: Some("download_queue_item".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "revision_confirmation": "proceeded", "newer_archive_id": newer_archive_id })),
    )
    .await;

    match start_one(
        &state,
        auth.as_ref().map(|e| &e.0),
        &id,
        activity_entry_id,
        true,
    )
    .await
    {
        Ok(job_id) => axum::Json(json!({
            "operation": "confirm_older_revision",
            "success": 1,
            "proceeded": true,
            "job": job_id,
        }))
        .into_response(),
        Err(StartError::NotFound) => {
            not_found("confirm_older_revision", format!("Item {id} not found."))
        }
        Err(StartError::PluginMissing) => error(
            StatusCode::BAD_REQUEST,
            "confirm_older_revision",
            "The plugin this item was queued under is no longer installed.",
        ),
        Err(StartError::DuplicateInFlight) => error(
            StatusCode::CONFLICT,
            "confirm_older_revision",
            format!("Another item for the same URL as {id} is already downloading."),
        ),
        Err(StartError::NotQueued) => error(
            StatusCode::CONFLICT,
            "confirm_older_revision",
            format!("Item {id} is not in the Queued state."),
        ),
        Err(StartError::Storage(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "confirm_older_revision",
            e,
        ),
    }
}

/// `POST /download_queue/start_all` — every `Queued`-state item, fired concurrently (each spawns
/// its own independent background task already; per-domain concurrency/rate-limiting is enforced
/// downstream by the existing `DownloadManager`, so no additional throttling is needed here).
async fn start_all(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
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
        .filter(|i| is_bulk_startable(i.state))
        .map(|i| i.id)
        .collect();
    start_many(&state, auth.as_ref().map(|e| &e.0), ids).await
}

/// Which states "Start All" sweeps up. Deliberately narrower than [`is_startable`]: that one also
/// admits `Error`/`Cancelled`, which a user retries deliberately one at a time, and it must never
/// admit `AwaitingRevisionConfirmation` — bulk-starting a parked item would answer, by omission,
/// the very question it is parked on (issue #107).
fn is_bulk_startable(state: DownloadQueueState) -> bool {
    matches!(state, DownloadQueueState::Queued)
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
    auth: Option<axum::extract::Extension<AuthContext>>,
    body: axum::Json<StartSelectedBody>,
) -> Response {
    start_many(&state, auth.as_ref().map(|e| &e.0), body.ids.clone()).await
}

async fn start_many(state: &AppState, auth: Option<&AuthContext>, ids: Vec<String>) -> Response {
    let mut started = Vec::new();
    let mut skipped = Vec::new();
    for id in ids {
        // Same per-item `download_queue.start` entry `start_queue_item` writes for a single start
        // — written *before* `start_one` for the same reason that one is (see its own docs): the
        // entry needs to exist so `start_one`/`plugins::start_download`'s background task can
        // patch its `after` with the real `archive_ids` once ingestion finishes. A bulk start
        // launching N items is N individual downloads exactly as much as N separate single-starts
        // would be — each deserves its own record for the same reason each shows up as its own
        // row in the download queue, not a `bulk_start`-only summary a user can't click into for
        // any *one* item's own eventual result.
        let item = state.download_queue.get(&id).await.ok().flatten();
        let label = item
            .as_ref()
            .map(|i| i.title.clone().unwrap_or_else(|| i.url.clone()));
        let activity_entry_id = record_manual(
            state,
            auth,
            action_types::DOWNLOAD_QUEUE_START,
            ActivityTarget {
                id: Some(id.clone()),
                label,
                kind: Some("download_queue_item".to_string()),
            },
            // Same "written before the attempt runs" reasoning as `start_queue_item`'s own entry.
            Outcome::Success,
            None,
            item.as_ref().map(|i| {
                // Same retry-staleness reasoning as `start_queue_item`'s own record.
                let cached = i.state == DownloadQueueState::Queued;
                download_activity_payload(
                    &i.url,
                    &i.plugin_namespace,
                    None,
                    None,
                    i.category.as_deref(),
                    i.auto_fetch_metadata,
                    i.overwrite_on_duplicate,
                    if cached { i.title.as_deref() } else { None },
                    if cached {
                        i.metadata_preview.as_ref()
                    } else {
                        None
                    },
                )
            }),
        )
        .await;
        match start_one(state, auth, &id, activity_entry_id, false).await {
            Ok(job_id) => started.push(json!({ "id": id, "job": job_id })),
            Err(StartError::NotQueued) => {} // silent no-op, per contract
            Err(StartError::Storage(e)) => {
                // A real Redis failure marking the item `Starting` — mirrors
                // `start_queue_item`'s own `StartError::Storage` failure record.
                record_manual(
                    state,
                    auth,
                    action_types::DOWNLOAD_QUEUE_START,
                    ActivityTarget {
                        id: Some(id.clone()),
                        label: None,
                        kind: Some("download_queue_item".to_string()),
                    },
                    Outcome::Failure { reason: e.clone() },
                    None,
                    None,
                )
                .await;
                skipped.push(json!({ "id": id, "reason": e }));
            }
            Err(e) => skipped.push(json!({ "id": id, "reason": start_error_message(&e) })),
        }
    }
    if !started.is_empty() {
        record_manual(
            state,
            auth,
            action_types::DOWNLOAD_QUEUE_BULK_START,
            ActivityTarget {
                id: None,
                label: Some(format!("{} items", started.len())),
                kind: Some("download_queue_item".to_string()),
            },
            Outcome::Success,
            None,
            Some(json!({ "started": started, "skipped": skipped })),
        )
        .await;
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
    auth: Option<axum::extract::Extension<AuthContext>>,
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
    if !deleted.is_empty() {
        record_manual(
            &state,
            auth.as_ref().map(|e| &e.0),
            action_types::DOWNLOAD_QUEUE_DELETE,
            ActivityTarget {
                id: None,
                label: Some(format!("{} items", deleted.len())),
                kind: Some("download_queue_item".to_string()),
            },
            Outcome::Success,
            None,
            Some(json!({ "ids": deleted })),
        )
        .await;
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

/// Starts a queue item on the subscription orchestrator's behalf — server-side, with no request
/// and nobody pressing Start behind it.
///
/// Needed because "download automatically" means exactly that (spec FR-007a and its own Q&A: the
/// alternative to downloading automatically is the approval list, *not* a second click on Start),
/// and because an approval is a decision the user has already made — parking the item in `Queued`
/// afterwards asks the same question twice. Everything else about the item is unchanged: it is a
/// normal queue row from here on, with the same stop/retry/conflict handling.
///
/// Recorded by the caller, not here (`auto_download.fetch` for the unattended path), so this stays
/// a plain "start it" with no opinion about who asked.
pub(crate) async fn start_item_from_subscription(
    state: &AppState,
    item_id: &str,
) -> Result<String, String> {
    start_one(state, None, item_id, None, false)
        .await
        .map_err(|e| start_error_message(&e))
}

/// `from_revision_confirmation` is set only by `confirm_older_revision`, and means "the user just
/// answered the older-revision prompt for this exact run" — it preserves the `revision_confirmed`
/// flag that would otherwise be cleared here as a stale approval. See the clearing logic below.
async fn start_one(
    state: &AppState,
    auth: Option<&AuthContext>,
    id: &str,
    activity_entry_id: Option<String>,
    from_revision_confirmation: bool,
) -> Result<String, StartError> {
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

    // 017: a share import runs its own transfer task, not a plugin; stopping and retrying go
    // through the same queue states and cancellation map as a download.
    if item.origin == lanrurugi_storage::download_queue::QueueItemOrigin::ShareImport {
        let import_id = item
            .share_import_id
            .clone()
            .ok_or_else(|| StartError::Storage("share import id missing".to_string()))?;
        item.state = DownloadQueueState::Starting;
        item.transferring = false;
        item.started_at = Some(now_ms());
        item.finished_at = None;
        item.error = None;
        state
            .download_queue
            .update(&item)
            .await
            .map_err(|e| StartError::Storage(e.to_string()))?;
        let job_id = crate::share_net::import::start(
            state,
            &item.id,
            lanrurugi_core::ids::ShareImportId(import_id),
        )
        .await;
        return Ok(job_id);
    }

    let info = state
        .plugins
        .plugin_info(&item.plugin_namespace)
        .await
        .map_err(|_| StartError::PluginMissing)?;

    // A retry (state was `Error`/`Cancelled`, not a fresh `Queued` item) clears whatever
    // title/tags a *previous* run already cached — `plugins.rs::start_download`'s own
    // post-download `ensure_metadata_cached` call is gated on `item.title.is_none()` when
    // `auto_fetch_metadata` is off (its own doc comment: "avoid re-fetching metadata that's
    // already been fetched"), which is the right call for a first run but silently skips a real
    // re-fetch on retry once a *prior* attempt already got as far as caching a title before
    // later failing/being stopped — confirmed live: a retried item kept showing the stale title
    // from before its failure instead of asking the metadata plugin again. Clearing
    // `metadata_preview_at` too, not just `title`/`metadata_preview`, matters just as much: that
    // field alone drives `ensure_metadata_cached`'s own independent 10-minute TTL cache-hit
    // check, which would otherwise return the stale cached value even after `title` was cleared.
    // `Paused` is deliberately exempt: resuming a paused download must keep the title/metadata the
    // paused row was showing, not blank them and re-fetch.
    if item.state != DownloadQueueState::Queued && item.state != DownloadQueueState::Paused {
        item.title = None;
        item.metadata_preview = None;
        item.metadata_preview_at = None;
        // A retry after a filename-conflict `Error` must also drop the stale conflict record —
        // otherwise `QueueItemRow.tsx`'s right-side button rendering (`item.pending_filename_
        // conflict ? <ConflictMenu> : ...`) keeps showing the old resolve-conflict button through
        // the entire new attempt instead of the expected Stop button, even once this fresh run is
        // genuinely back in flight (reported live, 2026-08-25).
        item.pending_filename_conflict = None;
    }

    // A `revision_confirmed` yes applies to the one run it was given for. `confirm_older_revision`
    // sets it immediately before calling this function, so that run keeps it; any *other* entry
    // into `start_one` (the plain Start button, Start All, a retry after an error) is a new
    // decision point and must ask again rather than inherit a stale approval.
    if !from_revision_confirmation {
        item.revision_confirmed = false;
    }
    // Any start clears a leftover prompt: either it was just answered (and the answer is carried by
    // the flag above), or this is a fresh run whose own check will re-raise it if still warranted.
    // Without this the row would keep rendering the old banner through the entire new attempt —
    // the same bug `pending_filename_conflict` had to be cleared here for.
    item.pending_revision_confirmation = None;
    // Re-learned from the domain registry (and this run's own resume attempt): the previous run may
    // have used a different host, so its observation must not carry over.
    item.resume_support = None;

    item.state = DownloadQueueState::Starting;
    // A previous attempt's transfer never survived its own end, so a retry starts out having no
    // bytes in flight — the "transferring now" count must not inherit the old attempt's flag.
    item.transferring = false;
    // Stamped here rather than read off the job: this is the moment the item actually left the
    // queue, and the row needs it even after the job that ran it is gone. A restart clears the
    // previous attempt's end stamp so a retry reports its own duration, not both runs'.
    item.started_at = Some(now_ms());
    item.finished_at = None;
    state
        .download_queue
        .update(&item)
        .await
        .map_err(|e| StartError::Storage(e.to_string()))?;

    // Fire-and-forget: populate the title/tooltip while the item is still in flight, not only
    // once it reaches `Done` — regressed when metadata caching moved server-side (2cbe2ce), whose
    // own commit message notes "前端不再依赖 fetchMetadataOnStart 的运行时调用" without adding an
    // equivalent start-time call here, so the title stayed blank for the whole download instead of
    // appearing the moment it began (reported live, 2026-08-25). `ensure_metadata_cached` already
    // persists + broadcasts its own result over SSE, so nothing here needs to await or use it.
    if item.title.is_none() {
        let state_for_metadata = state.clone();
        let mut item_for_metadata = item.clone();
        tokio::spawn(async move {
            crate::plugins::ensure_metadata_cached(&state_for_metadata, &mut item_for_metadata)
                .await;
        });
    }

    let job_id = crate::plugins::start_download(
        state.clone(),
        item.plugin_namespace.clone(),
        info,
        item.url.clone(),
        item.category.clone(),
        item.overwrite_on_duplicate,
        Some((state.download_queue.clone(), item.id.clone())),
        activity_entry_id,
        auth.cloned(),
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
        DownloadQueueState::Queued
            | DownloadQueueState::Error
            | DownloadQueueState::Cancelled
            | DownloadQueueState::Paused
    )
}

/// True while a download-queue item has a live background task holding it — either actually
/// transferring bytes, or blocked waiting for a concurrency permit to do so (`Waiting`) — see
/// `delete_queue_item`'s own docs for why these states must not be deletable.
fn is_in_flight(state: DownloadQueueState) -> bool {
    matches!(
        state,
        DownloadQueueState::Starting
            | DownloadQueueState::Waiting
            | DownloadQueueState::Downloading
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
async fn stop_queue_item(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    match stop_one(&state, &id).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::DOWNLOAD_QUEUE_STOP,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: None,
                    kind: Some("download_queue_item".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({ "operation": "stop_download_queue_item", "success": 1 }))
                .into_response()
        }
        Err(StopError::NotFound) => {
            not_found("stop_download_queue_item", format!("Item {id} not found."))
        }
        Err(StopError::NotRunning) => error(
            StatusCode::CONFLICT,
            "stop_download_queue_item",
            format!("Item {id} is not currently downloading."),
        ),
        Err(StopError::Storage(e)) => {
            // The item was genuinely in flight (found + is_in_flight) but the Redis read/write to
            // actually cancel it failed — a real attempted-and-failed stop, unlike NotFound/NotRunning.
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::DOWNLOAD_QUEUE_STOP,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: None,
                    kind: Some("download_queue_item".to_string()),
                },
                Outcome::Failure { reason: e.clone() },
                None,
                None,
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stop_download_queue_item",
                e,
            )
        }
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

/// `POST /download_queue/{id}/pause` — cancels an in-flight item's transfer like `stop`, but labels
/// the result `Paused` instead of `Cancelled`, keeping its `.part` file for a resume (issue #88).
///
/// The state is written *before* cancelling the token: the background task's own cancellation
/// branch then re-reads the item and leaves a `Paused` item alone instead of overwriting it with
/// `Cancelled` (see that branch in `plugins.rs`). The reverse order would race.
async fn pause_queue_item(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    match pause_one(&state, &id).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::DOWNLOAD_QUEUE_PAUSE,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: None,
                    kind: Some("download_queue_item".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({ "operation": "pause_download_queue_item", "success": 1 }))
                .into_response()
        }
        Err(StopError::NotFound) => {
            not_found("pause_download_queue_item", format!("Item {id} not found."))
        }
        Err(StopError::NotRunning) => error(
            StatusCode::CONFLICT,
            "pause_download_queue_item",
            format!("Item {id} is not currently downloading."),
        ),
        Err(StopError::Storage(e)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "pause_download_queue_item",
            e,
        ),
    }
}

async fn pause_one(state: &AppState, id: &str) -> Result<(), StopError> {
    let item = state
        .download_queue
        .get(id)
        .await
        .map_err(|e| StopError::Storage(e.to_string()))?
        .ok_or(StopError::NotFound)?;
    if !is_in_flight(item.state) {
        return Err(StopError::NotRunning);
    }
    let token = state
        .download_cancellations
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or(StopError::NotRunning)?;

    crate::plugins::update_queue_item_state(
        &state.download_queue,
        state.download_queue_tx.as_ref(),
        id,
        DownloadQueueState::Paused,
        item.job_id.clone(),
        None,
        None,
    )
    .await;
    token.cancel();
    Ok(())
}

/// Deliberately excludes `Error` — unlike `Done`, an errored item is a restartable, still-
/// actionable state (grouped with `Queued`/`Cancelled` everywhere else in this module: selectable,
/// has a retry button, shows its own error text), not "completed" work a user is done looking at.
/// A user wanting to discard a specific failed item can still do so individually via
/// `DELETE /download_queue/{id}` (which does clean up a `pending_filename_conflict`'s own staged
/// temp file — `Done` items never carry one, so this handler has nothing analogous to worry
/// about).
async fn clear_completed(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
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
        // A real, attempted bulk-delete that hit a Redis failure (not just "nothing to do" —
        // `cleared` items were already identified) — worth recording since the user's clear
        // action genuinely didn't take effect.
        record_manual(
            &state,
            auth.as_ref().map(|e| &e.0),
            action_types::DOWNLOAD_QUEUE_CLEAR_COMPLETED,
            ActivityTarget {
                id: None,
                label: None,
                kind: Some("download_queue_item".to_string()),
            },
            Outcome::Failure {
                reason: e.to_string(),
            },
            None,
            None,
        )
        .await;
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
    if cleared > 0 {
        record_manual(
            &state,
            auth.as_ref().map(|e| &e.0),
            action_types::DOWNLOAD_QUEUE_CLEAR_COMPLETED,
            ActivityTarget {
                id: None,
                label: None,
                kind: Some("download_queue_item".to_string()),
            },
            Outcome::Success,
            None,
            Some(json!({ "cleared": cleared })),
        )
        .await;
    }
    ok(
        "clear_completed_download_queue",
        [("cleared", json!(cleared))],
    )
}

/// Wall-clock milliseconds — the unit every other timestamp on the wire uses (`created_at`,
/// `metadata_preview_at`), and the one the queue row renders its elapsed time from.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use lanrurugi_storage::download_queue::DownloadQueueItem;

    use super::*;

    #[test]
    fn paused_is_resumable_but_not_in_flight() {
        assert!(is_startable(DownloadQueueState::Paused));
        assert!(!is_in_flight(DownloadQueueState::Paused));
        assert!(!is_bulk_startable(DownloadQueueState::Paused));
    }

    fn item(id: &str, url: &str, state: DownloadQueueState) -> DownloadQueueItem {
        DownloadQueueItem {
            subscription_id: None,
            share_import_id: None,
            id: id.to_string(),
            origin: lanrurugi_storage::download_queue::QueueItemOrigin::Download,
            url: url.to_string(),
            plugin_namespace: "download/ehentai".to_string(),
            file_size: None,
            average_bytes_per_sec: None,
            resume_support: None,
            category: None,
            metadata_tags: Vec::new(),
            auto_fetch_metadata: false,
            overwrite_on_duplicate: false,
            conflict_policy: lanrurugi_storage::subscriptions::ConflictPolicy::Ask,
            state,
            transferring: false,
            version_history: None,
            started_at: None,
            finished_at: None,
            job_id: None,
            archive_ids: None,
            title: None,
            metadata_preview: None,
            metadata_preview_at: None,
            error: None,
            pending_filename_conflict: None,
            pending_revision_confirmation: None,
            revision_confirmed: false,
            created_at: 0,
        }
    }

    #[test]
    fn download_activity_payload_omits_unknown_optional_fields() {
        let payload = download_activity_payload(
            "https://example.com/gallery/1",
            "download/ehentai",
            None,
            None,
            Some("manga"),
            true,
            false,
            None,
            None,
        );
        assert_eq!(payload["url"], "https://example.com/gallery/1");
        // No resolved `plugin_info` yet — the start-request side records just the namespace; the
        // full name/type are patched in later.
        assert_eq!(payload["plugin"]["namespace"], "download/ehentai");
        assert_eq!(payload["plugin_namespace"], "download/ehentai");
        assert_eq!(payload["execution_parameters"]["category"], "manga");
        assert_eq!(payload["auto_fetch_metadata"], true);
        assert_eq!(payload["overwrite_on_duplicate"], false);
        assert!(payload.get("title").is_none());
        assert!(payload.get("metadata").is_none());
        assert!(payload["execution_parameters"]
            .get("plugin_parameters")
            .is_none());
    }

    #[test]
    fn download_activity_payload_carries_title_metadata_and_named_parameters() {
        let metadata = json!({ "title": "Example", "tags": "artist: someone" });
        let parameters = json!({ "forceresampled": false });
        let payload = download_activity_payload(
            "https://example.com/gallery/1",
            "download/ehentai",
            None,
            Some(parameters.clone()),
            None,
            false,
            true,
            Some("Example"),
            Some(&metadata),
        );
        assert_eq!(payload["title"], "Example");
        assert_eq!(payload["metadata"], metadata);
        assert_eq!(
            payload["execution_parameters"]["plugin_parameters"],
            parameters
        );
        assert_eq!(payload["overwrite_on_duplicate"], true);
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

    /// The whole point of the parked state (issue #107): the ordinary Start button — and
    /// `start_selected`, which shares `is_startable` — must not be able to bypass the question the
    /// user was asked. Resuming goes through `confirm_older_revision` instead.
    #[test]
    fn awaiting_revision_confirmation_is_not_startable() {
        assert!(!is_startable(
            DownloadQueueState::AwaitingRevisionConfirmation
        ));
    }

    /// Nothing holds a parked item — no background task, no transfer — so `delete`/`clear` must
    /// treat it as an ordinary at-rest item rather than refusing to touch it.
    #[test]
    fn awaiting_revision_confirmation_is_not_in_flight() {
        assert!(!is_in_flight(
            DownloadQueueState::AwaitingRevisionConfirmation
        ));
    }

    /// `start_all` only ever picks up `Queued` items, so a parked one can't be swept back into
    /// flight by the bulk action either — the per-item `is_startable` guard is not the only path.
    /// Calls the same predicate `start_all` itself filters on — re-implementing that filter here
    /// would stay green even if production started sweeping up parked items.
    #[test]
    fn start_all_skips_a_parked_item() {
        assert!(is_bulk_startable(DownloadQueueState::Queued));
        assert!(!is_bulk_startable(
            DownloadQueueState::AwaitingRevisionConfirmation
        ));
        // Not bulk-started either: a retry after failure/stop is a per-item decision.
        assert!(!is_bulk_startable(DownloadQueueState::Error));
        assert!(!is_bulk_startable(DownloadQueueState::Cancelled));
    }

    /// A parked item is at rest, so a *different* queue entry for the same URL must not be
    /// considered blocked by it — `has_running_duplicate` keys off `is_in_flight`.
    #[test]
    fn a_parked_item_does_not_block_another_item_for_the_same_url() {
        let items = vec![
            item("a", "same-url", DownloadQueueState::Queued),
            item(
                "b",
                "same-url",
                DownloadQueueState::AwaitingRevisionConfirmation,
            ),
        ];
        assert!(!has_running_duplicate(&items[0], &items));
    }

    /// ...while a genuinely in-flight one still does, so the parked state didn't punch a hole in
    /// the existing dedup guard.
    #[test]
    fn an_in_flight_item_still_blocks_another_item_for_the_same_url() {
        let items = vec![
            item("a", "same-url", DownloadQueueState::Queued),
            item("b", "same-url", DownloadQueueState::Downloading),
        ];
        assert!(has_running_duplicate(&items[0], &items));
    }
}
