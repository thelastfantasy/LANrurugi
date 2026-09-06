//! `bench` endpoint group (T089, US8/FR-020–022). Additive-only, no legacy equivalent — see
//! `contracts/rest-api.md` and `contracts/benchmark-report.md`. Triggers
//! `lanrurugi_bench::compare::run_full_comparison` as a background job, reusing the same
//! job-tracking abstraction as backup/restore/rebuild-index rather than inventing a separate
//! polling mechanism.

use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use lanrurugi_bench::compare::{run_full_comparison, CompareConfig, SystemEndpoint};
use serde::Deserialize;
use serde_json::json;

use crate::common::not_found;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/bench/run", post(run_bench))
        .route("/bench/{reportid}", get(get_report))
}

#[derive(Debug, Deserialize)]
pub struct RunBenchRequest {
    pub legacy_url: String,
    pub legacy_api_key: Option<String>,
    pub new_url: String,
    pub new_api_key: Option<String>,
    pub archive_count: u64,
    #[serde(default)]
    pub total_size_bytes: u64,
    #[serde(default = "default_hardware_description")]
    pub hardware_description: String,
    #[serde(default = "default_title_needle")]
    pub title_needle: String,
    #[serde(default = "default_interactive_load_iterations")]
    pub interactive_load_iterations: usize,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_operation_timeout_secs")]
    pub operation_timeout_secs: u64,
}

fn default_hardware_description() -> String {
    "unspecified".to_string()
}
fn default_title_needle() -> String {
    "Synthetic".to_string()
}
fn default_interactive_load_iterations() -> usize {
    20
}
fn default_poll_interval_ms() -> u64 {
    250
}
fn default_operation_timeout_secs() -> u64 {
    1800
}

/// Runs the cross-system comparison against the caller-supplied legacy/new endpoints (both are
/// external HTTP targets from this crate's point of view — even when "new" happens to be this
/// same running instance, reached via its own bind address — so one implementation drives either
/// case identically, matching how `lanrurugi-bench-compare` works standalone).
async fn run_bench(
    State(state): State<AppState>,
    axum::Json(req): axum::Json<RunBenchRequest>,
) -> Response {
    let report_id = state.jobs.create("bench").await;
    let jobs = state.jobs.clone();
    let report_id_for_task = report_id.clone();

    tokio::spawn(async move {
        jobs.mark_active(&report_id_for_task).await;

        let config = CompareConfig {
            legacy: SystemEndpoint {
                base_url: req.legacy_url,
                api_key: req.legacy_api_key,
            },
            new: SystemEndpoint {
                base_url: req.new_url,
                api_key: req.new_api_key,
            },
            archive_count: req.archive_count,
            total_size_bytes: req.total_size_bytes,
            hardware_description: req.hardware_description,
            poll_interval: std::time::Duration::from_millis(req.poll_interval_ms),
            operation_timeout: std::time::Duration::from_secs(req.operation_timeout_secs),
            title_needle: req.title_needle,
            interactive_load_iterations: req.interactive_load_iterations,
        };

        match run_full_comparison(&config).await {
            Ok(report) => {
                let value = serde_json::to_value(&report).unwrap_or(serde_json::Value::Null);
                jobs.finish(&report_id_for_task, value).await;
            }
            Err(e) => jobs.fail(&report_id_for_task, e.to_string()).await,
        }
    });

    axum::Json(json!({ "operation": "run_bench", "success": 1, "report_id": report_id }))
        .into_response()
}

async fn get_report(State(state): State<AppState>, Path(reportid): Path<String>) -> Response {
    match state.jobs.get(&reportid).await {
        Some(status) if status.result.is_some() => axum::Json(status.result).into_response(),
        Some(status) if status.error.is_some() => axum::Json(json!({
            "operation": "get_bench_report",
            "success": 0,
            "error": status.error,
        }))
        .into_response(),
        Some(_) => not_found("get_bench_report", "Report not ready yet."),
        None => not_found("get_bench_report", format!("Report {reportid} not found.")),
    }
}
