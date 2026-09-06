//! Unauthenticated `/health` endpoint for container/local health checks.
//!
//! This is deliberately kept outside the `/api` namespace and away from `require_api_key`: Docker
//! healthchecks and local monitoring probes should never need a session/API token just to ask
//! "is the process still usable?".
//!
//! It performs real liveness checks rather than merely returning 200 because the HTTP server is
//! up:
//! - every logical Redis pool (archive, minion, config, search, metrics) is asked to PING;
//! - when `LANRURUGI_HEALTHCHECK_FRONTEND_URL` is set (the dev image sets it to Vite at
//!   `http://127.0.0.1:3000`), the Vite/static frontend is also probed;
//! - the recommender model status is reported as informational only, because the 118MB ONNX model
//!   may still be downloading/loading after the server has already become fully usable.
//!
//! Any failed required check returns `503 Service Unavailable` with a JSON body describing which
//! subsystem failed. All Redis pools are checked because `RedisDbs` intentionally keeps five
//! logical databases alive; a broken pool in one of them can silently degrade a different part of
//! the app even while the API still answers requests.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde_json::json;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/health", get(health))
}

async fn ping_pool(pool: &deadpool_redis::Pool) -> bool {
    let Ok(mut conn) = pool.get().await else {
        return false;
    };
    let pong: Result<String, _> = deadpool_redis::redis::cmd("PING")
        .query_async(&mut conn)
        .await;
    pong.is_ok_and(|v| v == "PONG")
}

async fn health(State(state): State<AppState>) -> Response {
    let redis_checks: Vec<(&str, bool)> = vec![
        ("archive", ping_pool(&state.redis.archive).await),
        ("minion", ping_pool(&state.redis.minion).await),
        ("config", ping_pool(&state.redis.config).await),
        ("search", ping_pool(&state.redis.search).await),
        ("metrics", ping_pool(&state.redis.metrics).await),
    ];

    // Optional frontend probe. The production image normally serves the frontend from the same
    // backend process, so it does not set this variable; the dev image sets it to Vite's port.
    let frontend_url = std::env::var("LANRURUGI_HEALTHCHECK_FRONTEND_URL")
        .ok()
        .filter(|s| !s.is_empty());
    let frontend_ok = match frontend_url.as_deref() {
        Some(url) => {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(2))
                .build();
            Some(match client {
                Ok(client) => client
                    .get(url)
                    .send()
                    .await
                    .map(|resp| resp.status().is_success())
                    .unwrap_or(false),
                Err(_) => false,
            })
        }
        None => None,
    };

    let redis_ok = redis_checks.iter().all(|(_, ok)| *ok);
    let frontend_ok_required = frontend_ok.unwrap_or(true);
    let healthy = redis_ok && frontend_ok_required;

    let status = if healthy { "ok" } else { "unhealthy" };

    let body = json!({
        "status": status,
        "checks": {
            "redis": serde_json::Map::from_iter(
                redis_checks.into_iter().map(|(name, ok)| {
                    (name.to_string(), json!(if ok { "ok" } else { "failed" }))
                })
            ),
            "frontend": frontend_ok.map(|ok| if ok { "ok" } else { "failed" }).unwrap_or("skipped"),
            "recommender": if state.recommender.ready() { "ready" } else { "loading" },
        },
    });

    let status_code = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status_code, axum::Json(body)).into_response()
}
