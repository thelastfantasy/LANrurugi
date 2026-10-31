//! `shinobu` endpoint group. Shapes verified against `~/LANraragi/tools/openapi.yaml`. There is
//! no separate Shinobu process in LANrurugi (constitution Principle III) — `pid` reports the
//! current (single) process PID rather than a distinct watcher-process PID, and `is_alive`
//! reflects whether the in-process watcher is currently watching (not merely started). Restart
//! and rescan also run a catch-up scan for files that arrived while it was not.

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

/// What the watcher's supervisor needs to restart and catch up on its own.
fn scanner_config(state: &AppState) -> lanrurugi_scanner::handle::ScannerConfig {
    lanrurugi_scanner::handle::ScannerConfig {
        archive_dir: state.library.archive_dir.clone(),
        thumb_dir: state.library.thumb_dir.clone(),
        config_pool: state.redis.config.clone(),
        search_pool: state.redis.search.clone(),
        equivalence: state.equivalence.clone(),
        archives: (*state.repos.archives).clone(),
        new_archive_tx: Some(state.new_archive_tx.clone()),
        locks: state.filename_locks.clone(),
        jobs: state.jobs.clone(),
    }
}

/// Legacy's fields unchanged (`pid` is this process's own); `state`, `last_error` and
/// `recoveries` are additive and describe the self-recovering watcher.
async fn shinobu_status(State(state): State<AppState>) -> Response {
    let status = state.scanner.status();
    let is_alive = status.state == lanrurugi_scanner::handle::WatcherState::Running;
    axum::Json(json!({
        "operation": "shinobu_status",
        "success": 1,
        "is_alive": if is_alive { 1 } else { 0 },
        "pid": std::process::id(),
        "state": status.state,
        "last_error": status.last_error,
        "recoveries": status.recoveries,
    }))
    .into_response()
}

async fn shinobu_stop(State(state): State<AppState>) -> Response {
    state.scanner.stop().await;
    axum::Json(json!({ "operation": "shinobu_stop", "success": 1 })).into_response()
}

async fn shinobu_restart(State(state): State<AppState>) -> Response {
    let result = state.scanner.restart(scanner_config(&state)).await;
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

    let result = state.scanner.restart(scanner_config(&state)).await;
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
