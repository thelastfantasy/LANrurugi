//! `shinobu` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml`. There is
//! no separate Shinobu process in LANrurugi (constitution Principle III) — `pid` reports the
//! current (single) process PID rather than a distinct watcher-process PID, and `is_alive`
//! reflects whether the in-process watcher task is running.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use serde_json::json;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/shinobu", get(shinobu_status))
        .route("/shinobu/stop", post(shinobu_stop))
        .route("/shinobu/restart", post(shinobu_restart))
        .route("/shinobu/rescan", post(shinobu_rescan))
}

async fn shinobu_status(State(state): State<AppState>) -> Response {
    let is_alive = state.scanner.is_alive().await;
    axum::Json(json!({
        "operation": "shinobu_status",
        "success": 1,
        "is_alive": if is_alive { 1 } else { 0 },
        "pid": std::process::id(),
    }))
    .into_response()
}

async fn shinobu_stop(State(state): State<AppState>) -> Response {
    state.scanner.stop().await;
    axum::Json(json!({ "operation": "shinobu_stop", "success": 1 })).into_response()
}

async fn shinobu_restart(State(state): State<AppState>) -> Response {
    let result = state
        .scanner
        .restart(
            state.library.archive_dir.clone(),
            state.library.thumb_dir.clone(),
            state.redis.config.clone(),
            state.redis.search.clone(),
            (*state.repos.archives).clone(),
            Some(state.new_archive_tx.clone()),
        )
        .await;
    match result {
        Ok(()) => axum::Json(json!({
            "operation": "shinobu_restart",
            "success": 1,
            "new_pid": std::process::id(),
        }))
        .into_response(),
        Err(e) => crate::common::error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "shinobu_restart",
            e.to_string(),
        ),
    }
}

/// `reset_filemap` semantics: legacy deletes the `LRR_FILEMAP` hash and restarts Shinobu, forcing
/// a full rescan. LANrurugi's watcher re-discovers on-disk files reactively (via `notify` events)
/// rather than an eager `update_filemap` walk on start, so clearing the filemap here mainly
/// affects rename/duplicate detection going forward — matching the documented *effect*
/// ("effectively prompting a full rescan") even though the mechanism differs.
async fn shinobu_rescan(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return crate::common::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "shinobu_rescan",
                e.to_string(),
            )
        }
    };
    use deadpool_redis::redis::AsyncCommands;
    use lanrurugi_storage::keys::FILEMAP_KEY;
    let _: Result<(), _> = conn.del(FILEMAP_KEY).await;

    let result = state
        .scanner
        .restart(
            state.library.archive_dir.clone(),
            state.library.thumb_dir.clone(),
            state.redis.config.clone(),
            state.redis.search.clone(),
            (*state.repos.archives).clone(),
            Some(state.new_archive_tx.clone()),
        )
        .await;
    match result {
        Ok(()) => axum::Json(json!({
            "operation": "shinobu_rescan",
            "success": 1,
            "new_pid": std::process::id(),
        }))
        .into_response(),
        Err(e) => crate::common::error(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "shinobu_rescan",
            e.to_string(),
        ),
    }
}
