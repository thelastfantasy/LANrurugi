//! Diagnostics: local error capture plus the feedback-bundle surface
//! (specs/018-error-telemetry-feedback). This module owns the runtime switches, the settings
//! surface and — as the stories land — the envelope ingest, the issue views, the bundle builder
//! and the time-boxed forwarder.
//!
//! Nothing here transmits anything by itself: the browser SDK posts to this instance's own
//! `/api/diagnostics/envelope`, and the only outbound destination is whatever tracker an
//! administrator configured *and* a window they opened.

pub mod bundle;
pub mod forward;
pub mod issue;
pub mod logs;
pub mod scrub;
pub mod symbolicate;
pub mod text;

use std::sync::OnceLock;

/// The one background writer: envelopes are accepted on the request path and written off it.
static WRITER: OnceLock<
    tokio::sync::mpsc::Sender<lanrurugi_storage::diagnostics::DiagnosticEvent>,
> = OnceLock::new();

/// Starts the writer task. Called once from `main`; tests write inline instead.
pub fn start_writer(state: AppState) {
    let (tx, mut rx) =
        tokio::sync::mpsc::channel::<lanrurugi_storage::diagnostics::DiagnosticEvent>(256);
    if WRITER.set(tx).is_err() {
        return;
    }
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Err(e) = state.repos.diagnostics.save_event(&event).await {
                tracing::warn!(error = %e, "diagnostics: could not store an event");
                state.repos.diagnostics.bump_drop("store_error").await;
                continue;
            }
            let limits = crate::diagnostics_settings::load(&state).await.retention();
            if let Err(e) = state.repos.diagnostics.evict(limits, now_ms()).await {
                tracing::warn!(error = %e, "diagnostics: eviction failed");
            }
            forward::maybe_forward(&state, &event).await;
        }
    });
}

/// Accepts an event for storage. Writes inline when no writer is running (tests, one-shot tools).
pub(super) async fn store(
    state: &AppState,
    event: lanrurugi_storage::diagnostics::DiagnosticEvent,
) {
    if let Some(tx) = WRITER.get() {
        // Sending consumes the event, so the inline fallback needs its own copy — the envelope can
        // be a megabyte, but this path only runs when no writer is registered.
        if tx.try_send(event.clone()).is_ok() {
            return;
        }
    }
    if let Err(e) = state.repos.diagnostics.save_event(&event).await {
        tracing::warn!(error = %e, "diagnostics: could not store an event");
        state.repos.diagnostics.bump_drop("store_error").await;
    }
}

/// Per-instance ingest budget (default 60/min, 1,000/h).
fn allow_ingest(settings: &crate::diagnostics_settings::DiagnosticsSettings) -> bool {
    use std::sync::Mutex;
    static BUDGET: OnceLock<Mutex<Budget>> = OnceLock::new();
    let budget = BUDGET.get_or_init(|| Mutex::new(Budget::default()));
    let per_minute = settings.number("diagnostics_rate_per_minute").max(1);
    let per_hour = settings.number("diagnostics_rate_per_hour").max(1);
    match budget.lock() {
        Ok(mut budget) => budget.take(now_ms(), per_minute, per_hour),
        Err(_) => true,
    }
}

#[derive(Default)]
struct Budget {
    minute_start: i64,
    minute_count: u64,
    hour_start: i64,
    hour_count: u64,
}

impl Budget {
    fn take(&mut self, now: i64, per_minute: u64, per_hour: u64) -> bool {
        if now - self.minute_start >= 60_000 {
            self.minute_start = now;
            self.minute_count = 0;
        }
        if now - self.hour_start >= 3_600_000 {
            self.hour_start = now;
            self.hour_count = 0;
        }
        if self.minute_count >= per_minute || self.hour_count >= per_hour {
            return false;
        }
        self.minute_count += 1;
        self.hour_count += 1;
        true
    }
}

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::error;

use crate::AppState;
use lanrurugi_storage::diagnostics::{DiagnosticEvent, DiagnosticIssue, Fingerprint, IssueState};

/// The whole diagnostics surface: the capture path here plus everything in [`routes`].
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/diagnostics/config", get(config))
        .route("/diagnostics/settings", get(get_settings).put(put_settings))
        .route(
            "/diagnostics/envelope",
            post(envelope).layer(axum::extract::DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/diagnostics/issues", get(list_issues))
        .route("/diagnostics/issues/{fingerprint}", get(issue_detail))
        .route(
            "/diagnostics/issues/{fingerprint}/state",
            post(set_issue_state),
        )
        .route("/diagnostics/events", get(list_events))
        .route("/diagnostics/bundles/preview", post(bundle_preview))
        .route("/diagnostics/bundles/text", post(bundle_text))
        .route("/diagnostics/bundles", post(bundle_download))
        .route("/diagnostics/window", get(get_window).post(post_window))
        .route("/diagnostics/window", delete(delete_window))
        .route(
            "/diagnostics/replay",
            post(replay).layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
}

// ---------------------------------------------------------------- issues

#[derive(Debug, Deserialize)]
struct IssueQuery {
    #[serde(default)]
    level: Vec<String>,
    #[serde(default)]
    source: Vec<String>,
    #[serde(default)]
    release: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    from: Option<i64>,
    #[serde(default)]
    to: Option<i64>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn list_issues(State(state): State<AppState>, Query(query): Query<IssueQuery>) -> Response {
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let issues = match state.repos.diagnostics.issues(1_000).await {
        Ok(issues) => issues,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "diagnostics_issues",
                e.to_string(),
            )
        }
    };
    let levels: std::collections::HashSet<&str> = query.level.iter().map(String::as_str).collect();
    let sources: std::collections::HashSet<&str> =
        query.source.iter().map(String::as_str).collect();
    let state_filter = query.state.as_deref().unwrap_or("open");
    let needle = query.q.as_deref().unwrap_or("").to_lowercase();

    let mut filtered: Vec<DiagnosticIssue> = issues
        .into_iter()
        .filter(|issue| {
            if state_filter != "all" && issue.state.as_str() != state_filter {
                return false;
            }
            if !levels.is_empty() && !levels.contains(issue.level.as_str()) {
                return false;
            }
            if !sources.is_empty() && !issue.sources.iter().any(|s| sources.contains(s.as_str())) {
                return false;
            }
            if let Some(release) = &query.release {
                if !release.is_empty() && !issue.releases.contains(release) {
                    return false;
                }
            }
            if let Some(from) = query.from {
                if issue.last_seen < from {
                    return false;
                }
            }
            if let Some(to) = query.to {
                if issue.first_seen > to {
                    return false;
                }
            }
            if !needle.is_empty() && !issue.title.to_lowercase().contains(&needle) {
                return false;
            }
            true
        })
        .collect();
    if query.sort.as_deref() == Some("frequent") {
        filtered.sort_by_key(|issue| std::cmp::Reverse(issue.count));
    }
    let total = filtered.len();
    filtered.truncate(limit);
    Json(json!({
        "issues": filtered.iter().map(issue_json).collect::<Vec<_>>(),
        "total": total,
    }))
    .into_response()
}

fn issue_json(issue: &DiagnosticIssue) -> Value {
    json!({
        "fingerprint": issue.fingerprint.as_str(),
        "title": issue.title,
        "level": issue.level.as_str(),
        "count": issue.count,
        "first_seen": issue.first_seen,
        "last_seen": issue.last_seen,
        "releases": issue.releases,
        "sources": issue.sources.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        "state": issue.state.as_str(),
        "request_id": issue.request_id,
        "sample_event_id": issue.sample_event_id.as_ref().map(|id| id.as_str()),
    })
}

async fn issue_detail(State(state): State<AppState>, Path(fingerprint): Path<String>) -> Response {
    let fingerprint = Fingerprint(fingerprint);
    let Ok(Some(issue)) = state.repos.diagnostics.issue(&fingerprint).await else {
        return error(StatusCode::NOT_FOUND, "diagnostics_issue", "No such issue.");
    };
    let events = state
        .repos
        .diagnostics
        .events_for_issue(&fingerprint, 20)
        .await
        .unwrap_or_default();
    let log_excerpt = match issue.request_id.as_deref() {
        Some(request_id) => log_excerpt(&state, request_id).await,
        None => Value::Null,
    };
    Json(json!({
        "issue": issue_json(&issue),
        "events": events.iter().map(event_json).collect::<Vec<_>>(),
        "log_excerpt": log_excerpt,
    }))
    .into_response()
}

/// The log lines around a request id, from every category that mentions it.
async fn log_excerpt(state: &AppState, request_id: &str) -> Value {
    let Some(dir) = &state.library.log_dir else {
        return Value::Null;
    };
    let mut out = serde_json::Map::new();
    for category in lanrurugi_core::logging::CATEGORIES {
        let entries = logs::read(dir, category).await;
        let around = logs::around_request(&entries, request_id, 40);
        if !around.is_empty() {
            out.insert(
                (*category).to_string(),
                Value::Array(
                    around
                        .iter()
                        .map(|entry| json!({ "line": entry.line, "at": entry.at, "text": entry.text }))
                        .collect(),
                ),
            );
        }
    }
    Value::Object(out)
}

async fn set_issue_state(
    State(state): State<AppState>,
    Path(fingerprint): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(wanted) = body.get("state").and_then(|v| v.as_str()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "diagnostics_state",
            "expected a state.",
        );
    };
    let Some(state_value) = IssueState::from_wire(wanted) else {
        return error(
            StatusCode::BAD_REQUEST,
            "diagnostics_state",
            "state must be open, resolved or ignored.",
        );
    };
    match state
        .repos
        .diagnostics
        .set_issue_state(&Fingerprint(fingerprint), state_value, now_ms())
        .await
    {
        Ok(Some(issue)) => Json(json!({ "issue": issue_json(&issue) })).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "diagnostics_state", "No such issue."),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "diagnostics_state",
            e.to_string(),
        ),
    }
}

// ---------------------------------------------------------------- merged timeline

#[derive(Debug, Deserialize)]
struct EventQuery {
    #[serde(default)]
    level: Vec<String>,
    #[serde(default)]
    source: Vec<String>,
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    from: Option<i64>,
    #[serde(default)]
    to: Option<i64>,
    #[serde(default)]
    limit: Option<usize>,
}

async fn list_events(State(state): State<AppState>, Query(query): Query<EventQuery>) -> Response {
    let limit = query.limit.unwrap_or(200).clamp(1, 1_000);
    let to = query.to.unwrap_or_else(now_ms);
    let from = query.from.unwrap_or(to - 86_400_000);
    let levels: std::collections::HashSet<&str> = query.level.iter().map(String::as_str).collect();
    let sources: std::collections::HashSet<&str> =
        query.source.iter().map(String::as_str).collect();
    let events = match state
        .repos
        .diagnostics
        .events_between(from, to, limit.saturating_mul(4))
        .await
    {
        Ok(events) => events,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "diagnostics_events",
                e.to_string(),
            )
        }
    };
    let mut events_out = Vec::new();
    for event in events {
        if !levels.is_empty() && !levels.contains(event.level.as_str()) {
            continue;
        }
        if !sources.is_empty() && !sources.contains(event.source.as_str()) {
            continue;
        }
        if let Some(fingerprint) = &query.fingerprint {
            if &event.fingerprint.0 != fingerprint {
                continue;
            }
        }
        events_out.push(event_json(&event));
        if events_out.len() >= limit {
            break;
        }
    }
    let mut log_out = Vec::new();
    if let Some(dir) = &state.library.log_dir {
        'categories: for category in lanrurugi_core::logging::CATEGORIES {
            let entries = logs::read(dir, category).await;
            for entry in logs::slice(&entries, from, to) {
                if log_out.len() >= limit {
                    break 'categories;
                }
                log_out.push(json!({
                    "kind": "log",
                    "category": category,
                    "at": entry.at,
                    "line": entry.line,
                    "level": entry.level.as_str(),
                    "text": entry.text,
                }));
            }
        }
    }
    Json(json!({ "entries": events_out, "log": log_out })).into_response()
}

fn event_json(event: &DiagnosticEvent) -> Value {
    // A web frame is minified; resolve it against the release's own maps when they are present.
    // The maps themselves never leave the process.
    let stack = event.stack.as_deref().map(|stack| {
        if event.source.as_str() == "web" {
            symbolicate::resolve_stack(event.release.as_deref(), stack)
        } else {
            stack.to_string()
        }
    });
    json!({
        "kind": "event",
        "id": event.id.as_str(),
        "at": event.at,
        "level": event.level.as_str(),
        "source": event.source.as_str(),
        "message": event.message,
        "stack": stack,
        "fingerprint": event.fingerprint.as_str(),
        "request_id": event.request_id,
        "release": event.release,
        "tags": event.tags,
        "breadcrumbs": event.breadcrumbs,
    })
}

// ---------------------------------------------------------------- bundles

const CAP: usize = 20 * 1024 * 1024;

async fn bundle_preview(
    State(state): State<AppState>,
    Json(selection): Json<bundle::Selection>,
) -> Response {
    match bundle::plan(&state, &selection).await {
        Ok(plan) => Json(json!({
            "files": plan.files,
            "total_bytes": plan.total_bytes,
            "over_cap": plan.total_bytes > CAP,
            "cap_bytes": CAP,
            "truncations": plan.summary.truncations,
            "range": { "from": plan.summary.from, "to": plan.summary.to },
            "events": plan.summary.events,
            "log_lines": plan.summary.log_lines,
        }))
        .into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, "diagnostics_bundle", message),
    }
}

async fn bundle_text(
    State(state): State<AppState>,
    Json(selection): Json<bundle::Selection>,
) -> Response {
    match build(&state, &selection, true).await {
        Ok(Built {
            rendered,
            bundle_name,
            ..
        }) => Json(json!({
            "zh": { "title": rendered.zh.title, "body": rendered.zh.body },
            "en": { "title": rendered.en.title, "body": rendered.en.body },
            "bundle_name": bundle_name,
        }))
        .into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, "diagnostics_bundle", message),
    }
}

async fn bundle_download(
    State(state): State<AppState>,
    Json(selection): Json<bundle::Selection>,
) -> Response {
    match build(&state, &selection, false).await {
        Ok(Built {
            bytes, bundle_name, ..
        }) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/zip".to_string()),
                (
                    header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{bundle_name}\""),
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(message) => error(StatusCode::BAD_REQUEST, "diagnostics_bundle", message),
    }
}

struct Built {
    bytes: Vec<u8>,
    rendered: text::Bilingual,
    bundle_name: String,
}

async fn build(
    state: &AppState,
    selection: &bundle::Selection,
    text_only: bool,
) -> Result<Built, String> {
    let plan = bundle::plan(state, selection).await?;
    let facts = facts(state);
    let top = match selection.issues.first() {
        Some(fingerprint) => state
            .repos
            .diagnostics
            .issue(&Fingerprint(fingerprint.clone()))
            .await
            .map_err(|e| e.to_string())?,
        None => None,
    };
    let rendered = bundle::feedback(&plan, top.as_ref(), &facts);
    if text_only {
        return Ok(Built {
            bytes: Vec::new(),
            rendered,
            bundle_name: facts.bundle_name,
        });
    }
    if plan.total_bytes > CAP && !selection.truncate {
        return Err(format!(
            "the selection is {} bytes; the cap is {CAP}. Narrow it, or accept truncation.",
            plan.total_bytes
        ));
    }
    let bytes = bundle::archive(&plan, selection, &facts, &rendered, &json!({}))?;
    Ok(Built {
        bytes,
        rendered,
        bundle_name: facts.bundle_name,
    })
}

fn facts(state: &AppState) -> text::Facts {
    let _ = state;
    let release = std::env::var("LANRURUGI_GIT_SHA").unwrap_or_else(|_| "unknown".to_string());
    text::Facts {
        version: crate::version::current_version().to_string(),
        release,
        engine: lanrurugi_share_proto::messages::ShareEngine::current().to_string(),
        platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        bundle_name: format!("lrrg-diagnostics-{}.zip", timestamp_for_name(now_ms())),
    }
}

fn timestamp_for_name(millis: i64) -> String {
    let formatted = text::format_instant(millis); // "2026-10-28 14:12 UTC"
    let (date, rest) = formatted.split_once(' ').unwrap_or((&formatted, ""));
    let time = rest.trim_end_matches(" UTC").replace(':', "");
    format!("{}-{}", date.replace('-', ""), time)
}

// ---------------------------------------------------------------- window

async fn get_window(State(state): State<AppState>) -> Response {
    Json(forward::window(&state).await.to_json()).into_response()
}

async fn post_window(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let duration = body
        .get("duration_secs")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    match forward::open_window(&state, duration).await {
        Ok(window) => Json(window.to_json()).into_response(),
        Err(message) => error(StatusCode::CONFLICT, "diagnostics_window", message),
    }
}

async fn delete_window(State(state): State<AppState>) -> Response {
    match forward::close_window(&state).await {
        Ok(window) => Json(window.to_json()).into_response(),
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "diagnostics_window",
            message,
        ),
    }
}

// ---------------------------------------------------------------- offline replay (US5)

async fn replay(State(state): State<AppState>, body: Bytes) -> Response {
    let settings = crate::diagnostics_settings::load(&state).await;
    let Some(raw) = settings.tracker_dsn() else {
        return error(
            StatusCode::CONFLICT,
            "diagnostics_replay",
            "configure a tracker first.",
        );
    };
    let Some(dsn) = forward::TrackerDsn::parse(raw) else {
        return error(
            StatusCode::CONFLICT,
            "diagnostics_replay",
            "the configured tracker address is not a valid DSN.",
        );
    };
    let envelopes = match bundle::envelopes_from_archive(&body) {
        Ok(envelopes) => envelopes,
        Err(message) => return error(StatusCode::BAD_REQUEST, "diagnostics_replay", message),
    };
    let mut sent = 0;
    let mut errors = Vec::new();
    for envelope in envelopes {
        let auth = format!(
            "Sentry sentry_version=7, sentry_key={}, sentry_client=lanrurugi/{}",
            dsn.key,
            crate::version::current_version()
        );
        match reqwest::Client::new()
            .post(&dsn.envelope_url)
            .header("X-Sentry-Auth", auth)
            .header("Content-Type", "application/x-sentry-envelope")
            .body(envelope)
            .timeout(std::time::Duration::from_secs(10))
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => sent += 1,
            Ok(response) => errors.push(format!("tracker answered {}", response.status())),
            Err(error) => errors.push(error.to_string()),
        }
    }
    Json(json!({ "sent": sent, "failed": errors.len(), "errors": errors })).into_response()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Kept out of the route module so the decode can be unit-tested without an `AppState`.
pub mod ingest {
    use lanrurugi_storage::diagnostics::{
        Breadcrumb, DiagnosticEvent, DiagnosticEventId, Fingerprint, Level, Source,
    };
    use serde_json::Value;

    pub(super) fn decode(body: &[u8]) -> Result<Option<DiagnosticEvent>, String> {
        let text = String::from_utf8_lossy(body);
        let mut lines = text.split('\n');
        let header = lines.next().ok_or("empty envelope")?;
        let header: Value = serde_json::from_str(header.trim())
            .map_err(|e| format!("malformed envelope header: {e}"))?;
        let mut event: Option<Value> = None;
        let mut current: Option<Value> = None;
        for line in lines {
            let line = line.trim_end_matches('\r');
            if let Some(kind) = current.as_ref().and_then(|v| v.get("type")) {
                if kind.as_str() == Some("event") {
                    event = serde_json::from_str::<Value>(line).ok();
                }
                current = None;
                // A payload's own remaining bytes are governed by `length`; a simple envelope
                // written by one of the SDKs is one line per item, which is what we accept.
                continue;
            }
            if line.is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(line) {
                if value.get("type").is_some() {
                    current = Some(value);
                }
            }
        }
        let event = match event {
            Some(event) => event,
            None => return Ok(None),
        };
        Ok(Some(project(&header, &event, body)))
    }

    fn project(header: &Value, event: &Value, raw: &[u8]) -> DiagnosticEvent {
        let at = event
            .get("timestamp")
            .and_then(|v| v.as_f64())
            .map(|secs| (secs * 1000.0) as i64)
            .unwrap_or_else(crate::diagnostics::now_ms);
        let level = event
            .get("level")
            .and_then(|v| v.as_str())
            .unwrap_or("error");
        let exception = event.get("exception").and_then(|v| v.get("values"));
        let message = exception
            .and_then(|values| values.as_array())
            .and_then(|values| values.last())
            .and_then(|last| {
                last.get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| last.get("type").and_then(|v| v.as_str()))
            })
            .map(str::to_string)
            .or_else(|| {
                event
                    .get("message")
                    .and_then(|m| m.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown error".to_string());
        let stack = exception
            .and_then(|values| values.as_array())
            .and_then(|values| values.last())
            .and_then(|last| last.get("stacktrace"))
            .map(render_stack);
        let platform = event
            .get("platform")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let release = event
            .get("release")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let source = Source::from_wire(platform.as_deref().unwrap_or("rust"));
        let client_at = event.get("time_spent").and_then(|v| v.as_i64()).map(|_| at);
        let tags = event
            .get("tags")
            .and_then(|v| v.as_object())
            .map(|tags| {
                tags.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let breadcrumbs = event
            .get("breadcrumbs")
            .and_then(|v| v.get("values"))
            .and_then(|v| v.as_array())
            .map(|values| {
                values
                    .iter()
                    .map(|value| Breadcrumb {
                        at: value
                            .get("timestamp")
                            .and_then(|v| v.as_f64())
                            .map(|secs| (secs * 1000.0) as i64)
                            .unwrap_or(at),
                        category: value
                            .get("category")
                            .and_then(|v| v.as_str())
                            .unwrap_or("default")
                            .to_string(),
                        level: value
                            .get("level")
                            .and_then(|v| v.as_str())
                            .unwrap_or("info")
                            .to_string(),
                        message: crate::diagnostics::scrub::text(
                            value.get("message").and_then(|v| v.as_str()).unwrap_or(""),
                            crate::diagnostics::scrub::ArchiveNames::Strip,
                        ),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let id = event
            .get("event_id")
            .and_then(|v| v.as_str())
            .or_else(|| header.get("event_id").and_then(|v| v.as_str()))
            .map(|v| DiagnosticEventId(v.to_string()))
            .unwrap_or_else(DiagnosticEventId::generate);
        DiagnosticEvent {
            id,
            // Filled in by the caller through `issue::fingerprint`; a placeholder keeps the
            // projection side-effect free.
            fingerprint: Fingerprint(String::new()),
            at,
            client_at,
            level: Level::from_wire(level),
            source,
            message: crate::diagnostics::scrub::text(&message, scrub_names()),
            stack: stack.map(|stack| crate::diagnostics::scrub::text(&stack, scrub_names())),
            release,
            platform,
            request_id: event
                .get("tags")
                .and_then(|tags| tags.get("request_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string),
            tags,
            breadcrumbs,
            envelope: raw.to_vec(),
            bytes: raw.len() as u64,
        }
    }

    fn scrub_names() -> crate::diagnostics::scrub::ArchiveNames {
        crate::diagnostics::scrub::ArchiveNames::Strip
    }

    fn render_stack(stack: &Value) -> String {
        let frames = stack
            .get("frames")
            .and_then(|v| v.as_array())
            .map(|frames| frames.as_slice())
            .unwrap_or(&[]);
        let mut out = String::new();
        for frame in frames.iter().rev() {
            let function = frame
                .get("function")
                .and_then(|v| v.as_str())
                .unwrap_or("<anonymous>");
            let file = frame
                .get("filename")
                .or_else(|| frame.get("abs_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>");
            let line = frame.get("lineno").and_then(|v| v.as_i64()).unwrap_or(0);
            let column = frame.get("colno").and_then(|v| v.as_i64()).unwrap_or(0);
            out.push_str(&format!("at {function} ({file}:{line}:{column})\n"));
        }
        out
    }
}

// ---------------------------------------------------------------- handlers

async fn config(State(state): State<AppState>) -> Response {
    let settings = crate::diagnostics_settings::load(&state).await;
    let enabled = settings.enabled() && settings.flag("diagnostics_web");
    Json(json!({
        "enabled": enabled,
        "sources": {
            "web": settings.flag("diagnostics_web"),
            "server": settings.flag("diagnostics_server"),
            "plugins": settings.flag("diagnostics_plugins"),
        },
        "breadcrumbs": settings.flag("diagnostics_breadcrumbs"),
        "window": {
            "open": settings.window_deadline() > now_ms(),
            "deadline": settings.window_deadline(),
            "remaining_secs": ((settings.window_deadline() - now_ms()).max(0) / 1000),
        },
    }))
    .into_response()
}

/// The settings page's own read: the settings plus what the store currently holds.
pub async fn get_settings(State(state): State<AppState>) -> Response {
    let settings = crate::diagnostics_settings::load(&state).await;
    let usage = state.repos.diagnostics.usage().await.unwrap_or_default();
    let drops = state.repos.diagnostics.drops().await;
    Json(json!({
        "settings": settings.to_json(),
        "usage": usage,
        "drops": drops,
    }))
    .into_response()
}

pub async fn put_settings(
    State(state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let Some(object) = body.as_object() else {
        return error(
            axum::http::StatusCode::BAD_REQUEST,
            "diagnostics_settings",
            "expected an object.",
        );
    };
    match crate::diagnostics_settings::save(&state, object).await {
        Ok(settings) => Json(json!({ "settings": settings.to_json() })).into_response(),
        Err(message) => error(
            axum::http::StatusCode::BAD_REQUEST,
            "diagnostics_settings",
            message,
        ),
    }
}

/// Raw envelope ingest — the browser SDK's transport target (see `research.md` §1). The detailed
/// decode lands with US1; until then this answers honestly rather than pretending to store.
pub async fn envelope(State(state): State<AppState>, body: axum::body::Bytes) -> Response {
    let settings = crate::diagnostics_settings::load(&state).await;
    if !settings.enabled() {
        return axum::http::StatusCode::NO_CONTENT.into_response();
    }
    if !allow_ingest(&settings) {
        state.repos.diagnostics.bump_drop("rate_limit").await;
        return axum::http::StatusCode::NO_CONTENT.into_response();
    }
    match ingest::decode(&body) {
        Ok(Some(mut event)) => {
            event.fingerprint = issue::fingerprint(&event);
            if !settings.capture_source(event.source.as_str()) {
                return axum::http::StatusCode::NO_CONTENT.into_response();
            }
            if event.bytes as usize > lanrurugi_storage::diagnostics::DiagnosticEvent::MAX_BYTES {
                let _ = state.repos.diagnostics.bump_drop("too_large").await;
                return axum::http::StatusCode::NO_CONTENT.into_response();
            }
            let id = event.id.as_str().to_string();
            store(&state, event).await;
            Json(json!({ "id": id })).into_response()
        }
        Ok(None) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(message) => error(
            axum::http::StatusCode::BAD_REQUEST,
            "diagnostics_envelope",
            message,
        ),
    }
}
