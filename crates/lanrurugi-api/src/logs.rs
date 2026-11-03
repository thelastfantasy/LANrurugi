//! `logs` endpoint group — additive, no OpenAPI contract equivalent (legacy's Logs page is
//! server-rendered HTML polling five separate routes, `Controller/Logging.pm`'s
//! `print_general`/`print_shinobu`/`print_plugins`/`print_redis`/`print_mojo`, each just
//! `print_lines_from_file($category)` — same shape here: last N lines of one category's log
//! file, N defaulting to 100 exactly like legacy's own default (`Controller/Logging.pm`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;

use crate::common::error;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/logs/{category}", get(get_log_lines))
}

#[derive(Debug, Deserialize)]
struct LogLinesParams {
    lines: Option<usize>,
}

async fn get_log_lines(
    State(state): State<AppState>,
    Path(category): Path<String>,
    Query(params): Query<LogLinesParams>,
) -> Response {
    if !lanrurugi_core::logging::CATEGORIES.contains(&category.as_str()) {
        return error(
            StatusCode::BAD_REQUEST,
            "get_log_lines",
            format!("Unknown log category: {category}"),
        );
    }

    let Some(log_dir) = &state.library.log_dir else {
        return (StatusCode::OK, "No logs to be found here!").into_response();
    };

    let path = log_dir.join(format!("{category}.log"));
    let requested_lines = params.lines.unwrap_or(100);

    match tokio::fs::read_to_string(&path).await {
        Ok(contents) => {
            let all_lines: Vec<&str> = contents.lines().collect();
            let start = all_lines.len().saturating_sub(requested_lines);
            (StatusCode::OK, all_lines[start..].join("\n")).into_response()
        }
        Err(_) => (StatusCode::OK, "No logs to be found here!").into_response(),
    }
}
