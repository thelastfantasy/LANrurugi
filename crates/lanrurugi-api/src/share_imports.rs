//! `/api/shares/imports*`: importing another instance's share (017 US4, FR-016–FR-021,
//! contracts/http-api.md). Administrators of the receiving instance only.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use lanrurugi_core::ids::{ArchiveId, ShareImportId};
use lanrurugi_share_proto::code::ShareRef;
use lanrurugi_storage::download_queue::{DownloadQueueState, NewQueueItem, QueueItemOrigin};
use lanrurugi_storage::shares::{
    HelpState, ImportArchiveState, ImportState, ShareImport, HELP_OFFLINE_AFTER_SECS,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth_context::AuthContext;
use crate::common::error;
use crate::share_net::import::{self, Opened};
use crate::share_net::{self, ShareNetError};
use crate::AppState;

/// How long a preview's connection is kept for its thumbnails and the confirm step.
const PREVIEW_TTL: Duration = Duration::from_secs(10 * 60);

struct Preview {
    opened: Arc<Opened>,
    code: String,
    created: Instant,
}

static PREVIEWS: Mutex<Option<HashMap<String, Preview>>> = Mutex::new(None);

fn store_preview(code: String, opened: Opened) -> String {
    let id = lanrurugi_core::ids::random_base32_id();
    let mut guard = PREVIEWS.lock().expect("previews lock");
    let map = guard.get_or_insert_with(HashMap::new);
    map.retain(|_, p| p.created.elapsed() < PREVIEW_TTL);
    map.insert(
        id.clone(),
        Preview {
            opened: Arc::new(opened),
            code,
            created: Instant::now(),
        },
    );
    id
}

fn preview(id: &str) -> Option<(Arc<Opened>, String)> {
    let guard = PREVIEWS.lock().expect("previews lock");
    guard
        .as_ref()?
        .get(id)
        .filter(|p| p.created.elapsed() < PREVIEW_TTL)
        .map(|p| (p.opened.clone(), p.code.clone()))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shares/imports", get(list_imports).post(create_import))
        .route("/shares/imports/resolve", post(resolve))
        .route(
            "/shares/imports/preview/{preview_id}/thumbnail/{archive_id}",
            get(preview_thumbnail),
        )
        .route("/shares/imports/{id}", get(get_import).patch(update_import))
        .route(
            "/shares/imports/{id}/archives/{sharer_archive_id}/retry",
            post(retry_archive),
        )
}

/// Refusals keep their stable code; an unreachable sharer is a 502, not a 404 (FR-029).
fn net_error(e: ShareNetError) -> Response {
    let status = match &e {
        ShareNetError::Refused(r) => match r.code() {
            "not_found" => StatusCode::NOT_FOUND,
            "exhausted" => StatusCode::CONFLICT,
            "rate_limited" => StatusCode::TOO_MANY_REQUESTS,
            "paused" => StatusCode::SERVICE_UNAVAILABLE,
            // Not 403: the web client treats a 403 as a possibly expired session and retries,
            // and a retried import would claim again.
            _ => StatusCode::UNPROCESSABLE_ENTITY,
        },
        ShareNetError::Unreachable(_) | ShareNetError::NotRunning => StatusCode::BAD_GATEWAY,
        ShareNetError::Protocol(_) => StatusCode::BAD_GATEWAY,
    };
    tracing::info!(error = %e, "share import request failed");
    error(status, "share_import", e.code())
}

// Fails with a ready-made axum `Response`, this helper's error by design; boxing it would
// ripple through every `?` for no gain — the same call as `plugins::move_into_category`.
#[allow(clippy::result_large_err)]
async fn running() -> Result<Arc<share_net::ShareNet>, Response> {
    share_net::handle()
        .await
        .ok_or_else(|| net_error(ShareNetError::NotRunning))
}

#[derive(Deserialize)]
struct ResolveBody {
    code: String,
}

/// Whether `code` is one of this instance's own shares. Importing it could only dial ourselves,
/// which fails as "sharer unreachable"; saying what it is instead is the useful answer.
async fn is_own_share(state: &AppState, code: &str) -> bool {
    let Ok(reference) = ShareRef::parse(code) else {
        return false;
    };
    let hash = crate::shares::hex(&lanrurugi_share_proto::derive::access_hash(&reference.code));
    matches!(
        crate::shares::repo(state).by_code_hash(&hash).await,
        Ok(Some(_))
    )
}

fn own_share() -> Response {
    error(StatusCode::CONFLICT, "share_import", "own_share")
}

/// `POST /shares/imports/resolve`: the preview (FR-018).
async fn resolve(State(state): State<AppState>, Json(body): Json<ResolveBody>) -> Response {
    if ShareRef::parse(&body.code).is_err() {
        return error(StatusCode::BAD_REQUEST, "share_import", "not_a_share_code");
    }
    if is_own_share(&state, &body.code).await {
        return own_share();
    }
    let net = match running().await {
        Ok(n) => n,
        Err(r) => return r,
    };
    let opened = match import::open(&state, &net, &body.code).await {
        Ok(o) => o,
        Err(e) => return net_error(e),
    };
    let mut total_size = 0u64;
    let mut already_present = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in opened.manifest.archives() {
        if !seen.insert(entry.archive_id.clone()) {
            continue;
        }
        total_size += entry.size;
        if matches!(
            state
                .repos
                .archives
                .get(&ArchiveId(entry.archive_id.clone()))
                .await,
            Ok(Some(_))
        ) {
            already_present.push(entry.archive_id.clone());
        }
    }
    let manifest = opened.manifest.clone();
    let public_url = opened.public_url.clone();
    let allow_helpers = opened.allow_helpers;
    let preview_id = store_preview(body.code, opened);
    Json(json!({
        "preview_id": preview_id,
        "manifest": manifest,
        "permissions": manifest.permissions,
        "archive_count": seen.len(),
        "total_size": total_size,
        "already_present": already_present,
        "public_url": public_url,
        "allow_helpers": allow_helpers,
    }))
    .into_response()
}

async fn preview_thumbnail(Path((preview_id, archive_id)): Path<(String, String)>) -> Response {
    let Some((opened, _)) = preview(&preview_id) else {
        return error(StatusCode::NOT_FOUND, "share_import", "not_found");
    };
    match import::thumbnail(&opened, &archive_id).await {
        Ok((content_type, bytes)) => (
            [
                (
                    header::CONTENT_TYPE,
                    content_type.unwrap_or_else(|| "image/jpeg".into()),
                ),
                (header::CACHE_CONTROL, "private, max-age=600".to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => net_error(e),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    code: String,
    #[serde(default)]
    preview_id: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    help_deliver: bool,
    #[serde(default)]
    inherit_categories: bool,
    /// The sharer archive ids to import; absent means every archive in the share.
    #[serde(default)]
    archives: Option<Vec<String>>,
}

/// `POST /shares/imports`: claims the share, then queues the import as one item (FR-019, FR-021).
async fn create_import(
    State(state): State<AppState>,
    _auth: Option<Extension<AuthContext>>,
    Json(body): Json<CreateBody>,
) -> Response {
    if is_own_share(&state, &body.code).await {
        return own_share();
    }
    let net = match running().await {
        Ok(n) => n,
        Err(r) => return r,
    };
    let opened = match body.preview_id.as_deref().and_then(preview) {
        Some((opened, code)) if code == body.code => opened,
        _ => match import::open(&state, &net, &body.code).await {
            Ok(o) => Arc::new(o),
            Err(e) => return net_error(e),
        },
    };
    // Checked before claiming, so a selection that matches nothing never spends a download.
    let chosen: Option<std::collections::HashSet<String>> =
        body.archives.map(|ids| ids.into_iter().collect());
    if let Some(chosen) = &chosen {
        if !opened
            .manifest
            .archives()
            .any(|a| chosen.contains(&a.archive_id))
        {
            return error(StatusCode::BAD_REQUEST, "share_import", "nothing_selected");
        }
    }
    let wanted = |id: &str| chosen.as_ref().is_none_or(|c| c.contains(id));
    // Refused before anything is queued, with the reason (FR-021).
    let (claim_id, manifest, _ticket) = match import::claim(&opened).await {
        Ok(c) => c,
        Err(e) => return net_error(e),
    };
    let settings = crate::share_settings::load(&state).await;
    let title = if manifest.title.is_empty() {
        "share".to_string()
    } else {
        manifest.title.clone()
    };
    let queued = state
        .download_queue
        .add(NewQueueItem {
            origin: QueueItemOrigin::ShareImport,
            subscription_id: None,
            url: format!("lrrg-share:{title}"),
            plugin_namespace: "share_import".to_string(),
            file_size: Some(
                import::archives_from(&manifest)
                    .iter()
                    .filter(|a| wanted(&a.sharer_archive_id))
                    .map(|a| a.size)
                    .sum(),
            ),
            category: body.category.clone(),
            metadata_tags: Vec::new(),
            auto_fetch_metadata: false,
            overwrite_on_duplicate: false,
            conflict_policy: Default::default(),
            state: DownloadQueueState::Queued,
        })
        .await;
    let mut item = match queued {
        Ok(i) => i,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "share_import",
                e.to_string(),
            )
        }
    };
    let record = ShareImport {
        id: ShareImportId::generate(),
        code: Some(body.code.clone()),
        sharer_endpoint_id: share_net::endpoint_hex(&opened.conn.remote_id()),
        share_id: manifest.share_id.clone(),
        claim_id,
        title: title.clone(),
        archives: import::archives_from(&manifest)
            .into_iter()
            .filter(|a| wanted(&a.sharer_archive_id))
            .collect(),
        manifest: serde_json::to_value(&manifest).unwrap_or(Value::Null),
        category: body.category,
        queue_item_id: item.id.clone(),
        concurrency: settings.number("share_import_concurrency").unwrap_or(3) as u32,
        help_deliver: body.help_deliver,
        help_hidden: false,
        help_state: Default::default(),
        help_reason: None,
        help_state_at: None,
        help_last_contact_at: None,
        help_removed_archives: Vec::new(),
        help_bytes_served: 0,
        inherit_categories: body.inherit_categories,
        state: ImportState::Transferring,
        error: None,
        paused_until: None,
        created_at: crate::shares::now_secs(),
    };
    if let Err(e) = import::repo(&state).save(&record).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "share_import",
            e.to_string(),
        );
    }
    item.share_import_id = Some(record.id.to_string());
    item.title = Some(title);
    let _ = state.download_queue.update(&item).await;
    if let Err(e) = crate::download_queue::start_item_from_subscription(&state, &item.id).await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "share_import", e);
    }
    (
        StatusCode::CREATED,
        Json(json!({ "import_id": record.id, "queue_item_id": item.id })),
    )
        .into_response()
}

/// One import with every archive, as `GET /shares/imports/{id}` returns it. `with_code` adds the
/// share code, which only that session-only route may carry: the queue stream is readable by
/// read-only tokens.
pub(crate) fn detail(import: &ShareImport, with_code: bool) -> Value {
    let mut value = summary(import);
    value["archives"] = serde_json::to_value(&import.archives).unwrap_or(Value::Null);
    if with_code {
        value["code"] = json!(import.code);
    }
    value
}

/// Collapsed-row figures for one import.
fn summary(import: &ShareImport) -> Value {
    let done = import
        .archives
        .iter()
        .filter(|a| {
            matches!(
                a.state,
                ImportArchiveState::Done | ImportArchiveState::Reused
            )
        })
        .count();
    let failed = import
        .archives
        .iter()
        .filter(|a| a.state == ImportArchiveState::Failed)
        .count();
    json!({
        "id": import.id,
        "title": import.title,
        "queue_item_id": import.queue_item_id,
        "state": import.state,
        "error": import.error,
        "items_done": done,
        "items_failed": failed,
        "items_total": import.archives.len(),
        // A reused archive transferred nothing but is complete; older records kept its bytes at 0.
        "bytes_done": import
            .archives
            .iter()
            .map(|a| match a.state {
                ImportArchiveState::Done | ImportArchiveState::Reused => a.size,
                _ => a.bytes_done,
            })
            .sum::<u64>(),
        "bytes_total": import.archives.iter().map(|a| a.size).sum::<u64>(),
        "help_deliver": import.help_deliver,
        "help_hidden": import.help_hidden,
        "help_state": import.shown_help_state(crate::shares::now_secs(), HELP_OFFLINE_AFTER_SECS),
        "help_reason": import.help_reason,
        "help_state_at": import.help_state_at,
        "help_last_contact_at": import.help_last_contact_at,
        "help_removed_archives": import.help_removed_archives.len(),
        "help_bytes_served": import.help_bytes_served,
        "sharer_endpoint_id": import.sharer_endpoint_id,
        "paused_until": import.paused_until,
        "created_at": import.created_at,
    })
}

#[derive(Deserialize)]
struct ListQuery {
    /// `1`: only imports that help (or helped) deliver their share and are not hidden.
    #[serde(default)]
    helping: Option<String>,
}

async fn list_imports(State(state): State<AppState>, Query(q): Query<ListQuery>) -> Response {
    let helping = q.helping.as_deref() == Some("1");
    match import::repo(&state).list().await {
        Ok(list) => Json(
            list.iter()
                .filter(|i| {
                    !helping
                        || (!i.help_hidden
                            && (i.help_deliver || i.help_state != HelpState::Inactive))
                })
                .map(summary)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "share_import",
            e.to_string(),
        ),
    }
}

// Fails with a ready-made axum `Response`, this helper's error by design; boxing it would
// ripple through every `?` for no gain — the same call as `plugins::move_into_category`.
#[allow(clippy::result_large_err)]
async fn load(state: &AppState, id: &str) -> Result<ShareImport, Response> {
    match import::repo(state).get(&ShareImportId::from(id)).await {
        Ok(Some(i)) => Ok(i),
        Ok(None) => Err(error(StatusCode::NOT_FOUND, "share_import", "not_found")),
        Err(e) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "share_import",
            e.to_string(),
        )),
    }
}

async fn get_import(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match load(&state, &id).await {
        Ok(record) => Json(detail(&record, true)).into_response(),
        Err(r) => r,
    }
}

#[derive(Deserialize)]
struct ImportUpdate {
    #[serde(default)]
    help_deliver: Option<bool>,
    #[serde(default)]
    help_hidden: Option<bool>,
}

async fn update_import(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<ImportUpdate>,
) -> Response {
    let guard = share_net::helper::record_lock().lock().await;
    let mut record = match load(&state, &id).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    if let Some(on) = body.help_deliver {
        record.help_deliver = on;
    }
    if let Some(hidden) = body.help_hidden {
        record.help_hidden = hidden;
        if hidden {
            record.help_deliver = false;
        }
    }
    // Switching helping off stops serving at once (FR-033); `ended` stays final.
    if !record.help_deliver && record.help_state != HelpState::Ended {
        record.help_state = HelpState::Inactive;
    }
    let saved = import::repo(&state).save(&record).await;
    drop(guard);
    if record.help_deliver {
        share_net::helper::wake();
    }
    match saved {
        Ok(()) => Json(summary(&record)).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "share_import",
            e.to_string(),
        ),
    }
}

/// Resets one failed archive and restarts the import if it had stopped.
async fn retry_archive(
    State(state): State<AppState>,
    Path((id, sharer_archive_id)): Path<(String, String)>,
) -> Response {
    let mut record = match load(&state, &id).await {
        Ok(r) => r,
        Err(r) => return r,
    };
    let Some(archive) = record
        .archives
        .iter_mut()
        .find(|a| a.sharer_archive_id == sharer_archive_id)
    else {
        return error(StatusCode::NOT_FOUND, "share_import", "not_found");
    };
    archive.state = ImportArchiveState::Pending;
    archive.error = None;
    if let Err(e) = import::repo(&state).save(&record).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "share_import",
            e.to_string(),
        );
    }
    let running = matches!(
        state.download_queue.get(&record.queue_item_id).await,
        Ok(Some(item)) if matches!(item.state, DownloadQueueState::Starting | DownloadQueueState::Downloading)
    );
    if !running {
        if record.code.is_none() {
            return error(
                StatusCode::CONFLICT,
                "share_import",
                "the import has finished",
            );
        }
        if let Err(e) =
            crate::download_queue::start_item_from_subscription(&state, &record.queue_item_id).await
        {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "share_import", e);
        }
    }
    StatusCode::NO_CONTENT.into_response()
}
