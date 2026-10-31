//! `lanrurugi-plugin-converter serve` — a minimal web UI wrapping [`crate::convert_source`], for
//! pasting a legacy `.pm` plugin's source and getting back the converted TS + any warnings
//! without needing a terminal. Meant to run inside the same container as the CLI (see
//! `Dockerfile.build` at the repo root) — it needs the exact same `perl`/PPI runtime dependency,
//! nothing extra.

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

const INDEX_HTML: &str = include_str!("../web/index.html");

#[derive(Clone)]
struct AppState;

pub fn router() -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/convert", post(convert))
        .with_state(AppState)
}

async fn index() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
        .into_response()
}

#[derive(Deserialize)]
struct ConvertRequest {
    source: String,
}

#[derive(Serialize)]
struct ConvertResponse {
    ts: String,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct ConvertErrorResponse {
    error: String,
}

async fn convert(State(_state): State<AppState>, Json(request): Json<ConvertRequest>) -> Response {
    if request.source.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ConvertErrorResponse {
                error: "Paste a .pm file's source first.".to_string(),
            }),
        )
            .into_response();
    }

    // `convert_source` shells out to `perl` per request — fine for this tool's actual usage
    // pattern (a developer converting a handful of plugin files by hand, not a high-throughput
    // service), so no need for a worker pool/queue here.
    match tokio::task::spawn_blocking(move || crate::convert_source(&request.source)).await {
        Ok(Ok(result)) => Json(ConvertResponse {
            ts: result.ts,
            warnings: result.warnings,
        })
        .into_response(),
        Ok(Err(e)) => (
            StatusCode::BAD_REQUEST,
            Json(ConvertErrorResponse {
                error: e.to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ConvertErrorResponse {
                error: format!("internal error: {e}"),
            }),
        )
            .into_response(),
    }
}
