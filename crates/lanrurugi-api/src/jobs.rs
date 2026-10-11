//! Job-status API. Two contracts live here:
//!
//! - `/minion/*` (T071) — mimics legacy LANraragi's Minion job-status JSON shape for any
//!   third-party tooling or legacy-derived frontend code that polls a specific, already-known job
//!   ID by the legacy contract (`task`/`state`/`notes`/`error` + the detail stub fields). Shapes
//!   verified against `~/LANraragi/tools/openapi.yaml`.
//! - `/jobs` (the Background Job Console, `specs/002-job-console`) — additive, native-shape
//!   endpoints (`id`/`name`/`state`/`progress`/`result`/`error`) with no legacy equivalent (see
//!   `contracts/jobs-api.md` / research.md §1). List, clear-one, and clear-all-finished over the
//!   existing in-process `lanrurugi_core::jobs::JobRegistry`.
//!
//! Both route groups share the same auth middleware: they're merged into the protected
//! `lanrurugi_api::router()`, which `build_app` wraps with `require_api_key` (FR-008 — there is no
//! unauthenticated `/minion`-style bypass; the middleware is exercised by the existing server-level
//! auth coverage shared with every other protected endpoint, so it isn't re-tested per route here).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::Router;
use lanrurugi_core::jobs::{ClearOutcome, JobState};
use serde::Deserialize;
use serde_json::json;

use crate::common::{error, not_found, ok};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        // Legacy-mimicking contract — left untouched (constitution Principle II).
        .route("/minion/{jobid}", get(job_status))
        .route("/minion/{jobid}/detail", get(job_detail))
        .route("/minion/{jobname}/queue", post(queue_job))
        // Native job-console contract (additive — contracts/jobs-api.md).
        .route("/jobs", get(list_jobs).delete(clear_finished_jobs))
        .route("/jobs/{id}", delete(clear_job))
}

fn state_str(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "inactive",
        JobState::Active => "active",
        JobState::Finished => "finished",
        JobState::Failed => "failed",
    }
}

async fn job_status(State(state): State<AppState>, Path(jobid): Path<String>) -> Response {
    match state.jobs.get(&jobid).await {
        Some(job) => axum::Json(json!({
            "task": job.name,
            "state": state_str(job.state),
            "notes": job.result,
            "error": job.error,
        }))
        .into_response(),
        None => not_found("minion_job_status", format!("Job {jobid} not found.")),
    }
}

async fn job_detail(State(state): State<AppState>, Path(jobid): Path<String>) -> Response {
    match state.jobs.get(&jobid).await {
        Some(job) => axum::Json(json!({
            "id": job.id,
            "task": job.name,
            "state": state_str(job.state),
            "result": job.result,
            "error": job.error,
            "notes": {},
            "args": [],
            "attempts": 1,
            "children": [],
            "parents": [],
            "priority": 0,
            "queue": "default",
            "retries": 0,
        }))
        .into_response(),
        None => not_found("minion_job_detail", format!("Job {jobid} not found.")),
    }
}

/// Legacy's own docs disclaim any real contract here ("no API contract in place for whether a Job
/// type exists on a given server version"). LANrurugi honors that by only actually dispatching job
/// names it has real handlers for; anything else is a clear 400 rather than a job that reports
/// "finished" without doing anything.
async fn queue_job(State(state): State<AppState>, Path(jobname): Path<String>) -> Response {
    match jobname.as_str() {
        "regen_all_thumbnails" => {
            let job_id = state.jobs.create(&jobname).await;
            axum::Json(json!({ "operation": "queue_minion_job", "success": 1, "job": job_id }))
                .into_response()
        }
        _ => error(
            StatusCode::BAD_REQUEST,
            "queue_minion_job",
            format!("Unsupported job type {jobname:?} in this version."),
        ),
    }
}

/// `GET /jobs` — every tracked job, most-recently-created first (T003/T004). `JobStatus`'s own
/// `Serialize` impl already omits `downloaded_bytes`/`total_bytes` entirely when absent
/// (`#[serde(skip_serializing_if = "Option::is_none")]` — `lanrurugi_core::jobs::JobStatus`), so
/// this handler needs no changes for `specs/005-download-plugin-progress`'s extended job shape
/// (`contracts/download-settings-api.md`) — it was already forwarding whatever `JobStatus` itself
/// serializes to.
async fn list_jobs(State(state): State<AppState>) -> Response {
    let jobs = state.jobs.list_all().await;
    axum::Json(json!({ "jobs": jobs })).into_response()
}

/// `DELETE /jobs/{id}` — clears one job by ID, only if it has reached a terminal state (T013).
async fn clear_job(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let outcome = state.jobs.clear(&id).await;
    match clear_job_status(outcome) {
        StatusCode::OK => ok("clear_job", []),
        status => error(status, "clear_job", clear_job_message(outcome, &id)),
    }
}

/// Maps the registry's three-valued clear outcome to its HTTP status code. Pure (no I/O), so it's
/// the unit-tested surface for the 200/404/409 response cases (T013) — the handler above is just
/// the thin glue that turns this status into the right envelope.
fn clear_job_status(outcome: ClearOutcome) -> StatusCode {
    match outcome {
        ClearOutcome::Cleared => StatusCode::OK,
        ClearOutcome::NotFound => StatusCode::NOT_FOUND,
        ClearOutcome::NotTerminal => StatusCode::CONFLICT,
    }
}

fn clear_job_message(outcome: ClearOutcome, id: &str) -> String {
    match outcome {
        ClearOutcome::Cleared => format!("Job {id} cleared."),
        ClearOutcome::NotFound => format!("Job {id} not found."),
        ClearOutcome::NotTerminal => format!(
            "Job {id} is still queued or active — wait for it to reach a terminal state before clearing."
        ),
    }
}

#[derive(Debug, Deserialize)]
struct ClearQuery {
    state: Option<String>,
}

/// Decision for `DELETE /jobs?state=…` (T014): only `state=finished` is accepted (meaning "any
/// terminal-state job" — both `Finished` and `Failed`, per data-model.md); a missing or any other
/// value is a 400. Pure (no I/O), so it's the unit-tested surface for the 200/400 cases.
enum BulkClearFilter {
    Accepted,
    Rejected,
}

impl BulkClearFilter {
    fn parse(state: Option<&str>) -> Self {
        if state.map(str::trim) == Some("finished") {
            Self::Accepted
        } else {
            Self::Rejected
        }
    }
}

/// `DELETE /jobs?state=finished` — clears every terminal-state job, leaving in-flight ones tracked
/// (T014). Returns the count cleared.
async fn clear_finished_jobs(
    State(state): State<AppState>,
    Query(query): Query<ClearQuery>,
) -> Response {
    match BulkClearFilter::parse(query.state.as_deref()) {
        BulkClearFilter::Accepted => {
            let cleared = state.jobs.clear_finished().await;
            ok("clear_finished_jobs", [("cleared", json!(cleared))])
        }
        BulkClearFilter::Rejected => error(
            StatusCode::BAD_REQUEST,
            "clear_finished_jobs",
            "Only ?state=finished is supported by this operation.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_job_maps_outcomes_to_status_codes() {
        assert_eq!(clear_job_status(ClearOutcome::Cleared), StatusCode::OK);
        assert_eq!(
            clear_job_status(ClearOutcome::NotFound),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            clear_job_status(ClearOutcome::NotTerminal),
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn bulk_clear_accepts_only_state_finished() {
        // Accepted (200 path).
        assert!(matches!(
            BulkClearFilter::parse(Some("finished")),
            BulkClearFilter::Accepted
        ));
        // Trimmed — a stray space still counts.
        assert!(matches!(
            BulkClearFilter::parse(Some("  finished  ")),
            BulkClearFilter::Accepted
        ));

        // Rejected (400 path): missing, empty, wrong value, wrong case.
        assert!(matches!(
            BulkClearFilter::parse(None),
            BulkClearFilter::Rejected
        ));
        assert!(matches!(
            BulkClearFilter::parse(Some("")),
            BulkClearFilter::Rejected
        ));
        assert!(matches!(
            BulkClearFilter::parse(Some("active")),
            BulkClearFilter::Rejected
        ));
        assert!(matches!(
            BulkClearFilter::parse(Some("failed")),
            BulkClearFilter::Rejected
        ));
        assert!(matches!(
            BulkClearFilter::parse(Some("Finished")),
            BulkClearFilter::Rejected
        ));
    }
}
