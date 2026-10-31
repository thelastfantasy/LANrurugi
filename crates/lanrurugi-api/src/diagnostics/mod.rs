//! Diagnostics: local error capture plus the feedback-bundle surface
//! (specs/018-error-telemetry-feedback). This module owns the runtime switches, the settings
//! surface and — as the stories land — the envelope ingest, the issue views, the bundle builder
//! and the time-boxed forwarder.
//!
//! Nothing here transmits anything by itself: the browser SDK posts to this instance's own
//! `/api/diagnostics/envelope`, and the only outbound destination is whatever tracker an
//! administrator configured *and* a window they opened.

pub mod attachment;
pub mod bundle;
pub mod forward;
pub mod issue;
pub mod logs;
pub mod report;
pub mod scrub;
pub mod symbolicate;
pub mod text;

use std::sync::OnceLock;

/// The running state, installed once at startup so the SDK transport (which has no arguments) can
/// hand an envelope to the same store the HTTP ingest uses.
static CAPTURE_STATE: OnceLock<AppState> = OnceLock::new();

/// Installed by `main` right after the state exists; before that, captured events are dropped.
pub fn attach_capture_state(state: AppState) {
    let _ = CAPTURE_STATE.set(state);
}

/// The server's own SDK transport: it never leaves the process. It serialises the envelope and
/// feeds it through the same decode → scrub → store path the browser's events take.
/// The runtime captured when capture is initialised, so the transport can spawn from a thread
/// that has no runtime of its own — an OCR/rayon worker, or a panic hook.
///
/// `tokio::spawn` panics there, and a panic raised *inside* a panic hook aborts the process
/// outright ("thread panicked while processing panic"). That is not hypothetical: it killed this
/// server 21 times on 2026-10-31, each time taking the reader's page down with it. A future that
/// cannot be spawned is dropped instead — capture is diagnostics, never worth the process.
static CAPTURE_RUNTIME: OnceLock<tokio::runtime::Handle> = OnceLock::new();

fn spawn_capture(work: impl std::future::Future<Output = ()> + Send + 'static) -> bool {
    let handle = tokio::runtime::Handle::try_current()
        .ok()
        .or_else(|| CAPTURE_RUNTIME.get().cloned());
    match handle {
        Some(handle) => {
            handle.spawn(work);
            true
        }
        None => false,
    }
}

struct LocalTransport;

/// The SDK wants a factory, not a transport instance; this one hands back the stateless
/// [`LocalTransport`].
impl sentry::TransportFactory for LocalTransport {
    fn create_transport_with_options(
        &self,
        _options: sentry::TransportOptions,
    ) -> std::sync::Arc<dyn sentry::Transport> {
        std::sync::Arc::new(LocalTransport)
    }
}

impl sentry::Transport for LocalTransport {
    fn send_envelope(&self, envelope: sentry::Envelope) {
        let mut body = Vec::new();
        if envelope.to_writer(&mut body).is_err() || body.is_empty() {
            return;
        }
        let Some(state) = CAPTURE_STATE.get().cloned() else {
            return;
        };
        spawn_capture(async move {
            let profile = crate::diagnostics_settings::load(&state)
                .await
                .scrub_profile();
            let Ok(Some(mut event)) = ingest::decode(&body, profile) else {
                return;
            };
            event.fingerprint = issue::fingerprint(&event);
            store(&state, event).await;
        });
    }
}

static CAPTURE_GUARD: OnceLock<sentry::ClientInitGuard> = OnceLock::new();

/// The installed [`report::Reporter`]; `report::Silent` until `init_server_capture` runs.
pub(crate) static REPORTER: OnceLock<&'static dyn report::Reporter> = OnceLock::new();

/// Initialises the server-side capture: panics, error-level `tracing` events and (via the
/// middleware in [`capture_5xx`]) 5xx responses all become local events. The DSN is a dummy — the
/// transport above is the only sink, so nothing is ever sent anywhere by the SDK itself.
pub fn init_server_capture() {
    // One source of truth for deployment identity: the same values a forwarded feedback reports.
    let deployment = crate::version::deployment();
    let release = deployment.release.clone();
    let environment = deployment.environment.clone();
    let hostname = deployment.hostname.clone();
    // `ClientOptions` is `#[non_exhaustive]`: start from the defaults and set what matters.
    let mut options = sentry::ClientOptions::default();
    options.dsn = "http://lanrurugi@127.0.0.1/1".parse().ok();
    options.release = release.clone().map(Into::into);
    options.environment = Some(environment.clone().into());
    options.transport = Some(std::sync::Arc::new(LocalTransport));
    options.attach_stacktrace = true;
    // Performance: 1% by default. Enough for a p95 on a self-hosted instance, nowhere near enough
    // to become the bulk of what is stored.
    let trace_rate = std::env::var("LANRURUGI_TRACES_SAMPLE_RATE")
        .ok()
        .and_then(|value| value.trim().parse::<f32>().ok())
        .map(|rate| rate.clamp(0.0, 1.0))
        .unwrap_or(0.01);
    options = options.traces_sample_rate(trace_rate);
    let _ = CAPTURE_RUNTIME.set(tokio::runtime::Handle::current());
    let _ = CAPTURE_GUARD.set(sentry::init(options));
    report::install(&report::SentryReporter);
    // Deployment identity, so several installs forwarding to one project stay distinguishable.
    sentry::configure_scope(|scope| {
        scope.set_tag("deployment", hostname);
        scope.set_tag("environment", &environment);
        if let Some(release) = &release {
            scope.set_tag("release", release);
        }
    });
}

/// Records a 5xx response as one event. Handlers answer with a `Response` (not `Result`), so a
/// tower error layer never sees them; this checks the status instead.
pub async fn capture_5xx(
    State(state): State<AppState>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let response = next.run(request).await;
    if response.status().is_server_error() {
        let settings = crate::diagnostics_settings::load(&state).await;
        if settings.capture_source("server") {
            let status = response.status().as_u16();
            let mut attrs = report::Attrs::new()
                .tag("source", "server")
                .tag("http_method", method.to_string())
                .tag("http_status", status.to_string())
                .tag("http_path", &path);
            // The request id ties this event to the log lines of the same request, which is what
            // makes an issue's "related logs" excerpt work.
            if let Some(id) = request_id.as_deref() {
                attrs = attrs.tag("request_id", id);
            }
            report::reporter().error(&format!("{method} {path} answered {status}"), attrs);
        }
    }
    response
}

/// Records a plugin run that failed (Deno-side error, timeout, crash). Plugin failures are logged
/// at `warn`, so the error-level tracing bridge would miss them; a manual run failing is a real
/// defect from the administrator's point of view, so it is captured explicitly as a plugin event.
pub fn capture_plugin_error(plugin: &str, message: &str) {
    report::reporter().error(message, report::Attrs::new().plugin(plugin));
}

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
use lanrurugi_storage::device_info::ClientReportedInfo;
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
        // The Sentry SDK derives its ingest URL from the DSN as `<dsn>/api/<project>/envelope/`,
        // trailing slash included, so both spellings have to reach the handler.
        .route(
            "/diagnostics/envelope/",
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
        // Multipart with up to three images: the default 2 MB body limit would reject every one.
        .route(
            "/diagnostics/feedback",
            post(feedback).layer(axum::extract::DefaultBodyLimit::max(
                attachment::MAX_IMAGES * attachment::MAX_UPLOAD_BYTES + 1024 * 1024,
            )),
        )
        .route("/diagnostics/window", get(get_window).post(post_window))
        .route("/diagnostics/window", delete(delete_window))
        .route(
            "/diagnostics/replay",
            post(replay).layer(axum::extract::DefaultBodyLimit::max(64 * 1024 * 1024)),
        )
}

// ---------------------------------------------------------------- issues

/// The issues list's query string. Repeated keys (`level=error&level=warning`) need
/// `form_urlencoded`, not `serde_urlencoded`: the latter cannot deserialise a `Vec` from them.
#[derive(Debug, Default)]
struct IssueQuery {
    level: Vec<String>,
    source: Vec<String>,
    release: Option<String>,
    state: Option<String>,
    from: Option<i64>,
    to: Option<i64>,
    q: Option<String>,
    sort: Option<String>,
    limit: Option<usize>,
}

impl IssueQuery {
    fn parse(raw: &str) -> Self {
        let mut query = Self::default();
        for (key, value) in url::form_urlencoded::parse(raw.as_bytes()) {
            match key.as_ref() {
                "level" => query.level.push(value.into_owned()),
                "source" => query.source.push(value.into_owned()),
                "release" => query.release = Some(value.into_owned()),
                "state" => query.state = Some(value.into_owned()),
                "from" => query.from = value.parse().ok(),
                "to" => query.to = value.parse().ok(),
                "q" => query.q = Some(value.into_owned()),
                "sort" => query.sort = Some(value.into_owned()),
                "limit" => query.limit = value.parse().ok(),
                _ => {}
            }
        }
        query
    }
}

async fn list_issues(
    State(state): State<AppState>,
    axum::extract::RawQuery(raw): axum::extract::RawQuery,
) -> Response {
    let query = IssueQuery::parse(raw.as_deref().unwrap_or_default());
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
            download: Some(path),
            bundle_name,
            ..
        }) => {
            let length = std::fs::metadata(&path).map(|m| m.len()).ok();
            match crate::common::stream_temp_file(path.clone()) {
                Ok(body) => {
                    let mut response = (
                        StatusCode::OK,
                        [
                            (header::CONTENT_TYPE, "application/zip".to_string()),
                            (
                                header::CONTENT_DISPOSITION,
                                format!("attachment; filename=\"{bundle_name}\""),
                            ),
                        ],
                        body,
                    )
                        .into_response();
                    if let Some(length) = length {
                        response
                            .headers_mut()
                            .insert(header::CONTENT_LENGTH, length.into());
                    }
                    response
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&path);
                    error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "diagnostics_bundle",
                        e.to_string(),
                    )
                }
            }
        }
        Ok(Built { download: None, .. }) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "diagnostics_bundle",
            "bundle was not produced",
        ),
        Err(message) => error(StatusCode::BAD_REQUEST, "diagnostics_bundle", message),
    }
}

struct Built {
    /// Temp file the archive was streamed into (`None` for the text-only preview). Deleted once
    /// the download response's body is dropped.
    download: Option<std::path::PathBuf>,
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
            download: None,
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
    // Written straight to a temp file and streamed from there: a bundle no longer exists as one
    // `Vec<u8>` in the process (it is capped at `CAP`, but the zip plus its entries were both held).
    let path = std::env::temp_dir().join(format!(
        "lrr-diagnostics-{}.zip",
        uuid::Uuid::new_v4().simple()
    ));
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    if let Err(e) = bundle::archive_to(
        std::io::BufWriter::new(file),
        &plan,
        selection,
        &facts,
        &rendered,
        &json!({}),
    ) {
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(Built {
        download: Some(path),
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

// ---------------------------------------------------------------- user feedback

/// Bounds on a submitted note. Generous enough for a real description, small enough that nobody can
/// use the instance as a message pump.
const FEEDBACK_MAX_CHARS: usize = 2_000;
/// Five notes per ten minutes, and the same text is not accepted twice within five minutes.
const FEEDBACK_PER_WINDOW: u32 = 5;
const FEEDBACK_WINDOW_SECS: i64 = 600;
const FEEDBACK_DEDUPE_SECS: i64 = 300;

struct FeedbackBudget {
    started: i64,
    used: u32,
    last_hash: u64,
    last_at: i64,
}

static FEEDBACK_BUDGET: OnceLock<std::sync::Mutex<FeedbackBudget>> = OnceLock::new();

/// Where a note came from, for the per-identity daily quota. `session` is the administrator's own
/// login; a token is identified by its id so one leaked token cannot exhaust the day's allowance
/// for the logged-in administrator.
fn identity_key(auth: Option<&crate::auth_context::AuthContext>) -> String {
    use crate::auth_context::AuthMethod;
    match auth.map(|auth| &auth.method) {
        Some(AuthMethod::Token { id, .. }) => format!("token:{id}"),
        Some(AuthMethod::Session) => "session".to_string(),
        _ => "guest".to_string(),
    }
}

/// A stable, opaque id for a report's `user.id` — the tracker groups by it, and nothing about the
/// account itself is sent.
fn feedback_user_id(identity: &str) -> String {
    // blake3, already a dependency here, and truncated: enough to be unique per identity, short
    // enough to read in the tracker's UI.
    let digest = blake3::hash(identity.as_bytes());
    format!("lrrg-{}", &digest.to_hex()[..16])
}

/// `Ok(())` when a note may be sent; `Err(reason)` when it is spam, oversized or too frequent.
fn admit_feedback(message: &str) -> Result<(), String> {
    let budget = FEEDBACK_BUDGET.get_or_init(|| {
        std::sync::Mutex::new(FeedbackBudget {
            started: now_ms() / 1000,
            used: 0,
            last_hash: 0,
            last_at: 0,
        })
    });
    let Ok(mut budget) = budget.lock() else {
        return Ok(());
    };
    admit(&mut budget, now_ms() / 1000, message)
}

/// The decision itself, over explicit state — unit-testable without touching the process global.
fn admit(budget: &mut FeedbackBudget, now: i64, message: &str) -> Result<(), String> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return Err("the message is empty.".to_string());
    }
    if trimmed.chars().count() > FEEDBACK_MAX_CHARS {
        return Err(format!(
            "the message is longer than {FEEDBACK_MAX_CHARS} characters."
        ));
    }
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        trimmed.hash(&mut hasher);
        hasher.finish()
    };
    if now - budget.started >= FEEDBACK_WINDOW_SECS {
        budget.started = now;
        budget.used = 0;
    }
    if budget.used >= FEEDBACK_PER_WINDOW {
        return Err("too many notes in a short time; try again later.".to_string());
    }
    if hash == budget.last_hash && now - budget.last_at < FEEDBACK_DEDUPE_SECS {
        return Err("that note was already submitted.".to_string());
    }
    budget.used += 1;
    budget.last_hash = hash;
    budget.last_at = now;
    Ok(())
}

/// How many notes this identity may still send today. `None` means the quota is spent. The counter
/// lives in Redis with an expiry just past midnight UTC, so restarts cannot reset it.
async fn daily_feedback_budget(state: &AppState, identity: &str) -> Option<u32> {
    const DAILY_LIMIT: u32 = 20;
    let key = format!(
        "LANRURUGI_FEEDBACK_DAILY:{identity}:{}",
        now_ms() / 86_400_000
    );
    let Ok(mut conn) = state.redis.config.get().await else {
        return Some(DAILY_LIMIT);
    };
    let used: u32 = deadpool_redis::redis::cmd("INCR")
        .arg(&key)
        .query_async::<u32>(&mut conn)
        .await
        .unwrap_or(0);
    if used == 1 {
        let _: Result<(), _> = deadpool_redis::redis::cmd("EXPIRE")
            .arg(&key)
            .arg(90_000)
            .query_async(&mut conn)
            .await;
    }
    (used <= DAILY_LIMIT).then(|| DAILY_LIMIT - used)
}

/// A feedback note's required contact fields, validated once so the handler's body stays flat.
struct Contact {
    name: String,
    email: String,
}

/// Name and email are required: the tracker shows them as the reporter's identity, and the email
/// is the only way back to the person who wrote the note.
fn feedback_contact(name: &str, email: &str) -> Result<Contact, String> {
    let clean =
        |value: &str| -> String { value.trim().chars().filter(|c| !c.is_control()).collect() };
    let name = clean(name);
    if name.is_empty() {
        return Err("a name is required.".to_string());
    }
    if name.chars().count() > 128 {
        return Err("the name is longer than 128 characters.".to_string());
    }
    let email = clean(email);
    if email.is_empty() {
        return Err("an email address is required.".to_string());
    }
    if email.chars().count() > 254 {
        return Err("the email address is longer than 254 characters.".to_string());
    }
    // Deliberately loose: one `@`, non-empty sides, no whitespace. Anything stricter rejects real
    // addresses (plus-addressing, one-label internal domains) for no benefit here.
    let valid = email.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty() && !domain.is_empty() && !domain.contains('@')
    }) && !email.contains(char::is_whitespace);
    if !valid {
        return Err("the email address is not valid.".to_string());
    }
    Ok(Contact { name, email })
}

/// The page the note was written on: a path on this instance, never an absolute URL or a query
/// string (a share link lives there).
fn feedback_url(raw: &str) -> Option<String> {
    let clean: String = raw.trim().chars().filter(|c| !c.is_control()).collect();
    (clean.starts_with('/') && clean.len() <= 500).then_some(clean)
}

/// One submitted note: the text fields plus up to [`attachment::MAX_IMAGES`] screenshots.
struct FeedbackForm {
    fields: std::collections::HashMap<String, String>,
    images: Vec<(String, Vec<u8>)>,
}

/// Reads the multipart body. Screenshots are files; every other field is a short string (`client`
/// is JSON encoded as one, see `FeedbackView.tsx`).
async fn read_feedback_form(
    multipart: &mut axum::extract::Multipart,
) -> Result<FeedbackForm, (StatusCode, String)> {
    let mut form = FeedbackForm {
        fields: std::collections::HashMap::new(),
        images: Vec::new(),
    };
    loop {
        let field = match multipart.next_field().await {
            Ok(Some(field)) => field,
            Ok(None) => break,
            Err(error) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("malformed form data: {error}"),
                ))
            }
        };
        let name = field.name().unwrap_or_default().to_string();
        if name == "image" {
            if form.images.len() >= attachment::MAX_IMAGES {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("at most {} images per note.", attachment::MAX_IMAGES),
                ));
            }
            let filename = field.file_name().unwrap_or("feedback.png").to_string();
            let bytes = match field.bytes().await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("could not read the image: {error}"),
                    ))
                }
            };
            if bytes.len() > attachment::MAX_UPLOAD_BYTES {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!(
                        "an image is larger than {} MB.",
                        attachment::MAX_UPLOAD_BYTES / 1024 / 1024
                    ),
                ));
            }
            form.images.push((filename, bytes.to_vec()));
        } else {
            match field.text().await {
                Ok(text) => {
                    form.fields.insert(name, text);
                }
                Err(error) => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("could not read a form field: {error}"),
                    ))
                }
            }
        }
    }
    Ok(form)
}

async fn feedback(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    headers: axum::http::HeaderMap,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let form = match read_feedback_form(&mut multipart).await {
        Ok(form) => form,
        Err((status, reason)) => return error(status, "diagnostics_feedback", reason),
    };
    let Some(message) = form.fields.get("message") else {
        return error(
            StatusCode::BAD_REQUEST,
            "diagnostics_feedback",
            "expected a message.",
        );
    };
    let contact = match feedback_contact(
        form.fields
            .get("name")
            .map(String::as_str)
            .unwrap_or_default(),
        form.fields
            .get("email")
            .map(String::as_str)
            .unwrap_or_default(),
    ) {
        Ok(contact) => contact,
        // Before the quota is touched: a form that was never sendable must not cost a send.
        Err(reason) => return error(StatusCode::BAD_REQUEST, "diagnostics_feedback", reason),
    };
    if let Err(reason) = admit_feedback(message) {
        // 429 rather than 400: the content may be fine, the pace is the problem.
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "diagnostics_feedback",
            reason,
        );
    }
    // Control characters (including NUL and stray CRs) are stripped; the note is stored as text.
    let clean: String = message
        .trim()
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    // Per-identity daily quota: the identity is the authenticated caller, which the server already
    // knows, so a client cannot reset it by clearing storage or forging an id.
    let identity = identity_key(auth.as_ref().map(|extension| &extension.0));
    let Some(remaining) = daily_feedback_budget(&state, &identity).await else {
        return error(
            StatusCode::TOO_MANY_REQUESTS,
            "diagnostics_feedback",
            "the daily feedback limit for this account is used up.",
        );
    };
    // Every upload is re-encoded to WebP here, before anything leaves the process.
    let mut attachments = Vec::with_capacity(form.images.len());
    for (filename, bytes) in form.images {
        match attachment::convert(&filename, bytes).await {
            Ok(converted) => attachments.push(converted),
            Err(reason) => return error(StatusCode::BAD_REQUEST, "diagnostics_feedback", reason),
        }
    }
    let user_id = feedback_user_id(&identity);
    let requested = form.fields.get("event_id").map(String::as_str);
    let event_id = match requested {
        Some(id) => Some(id.to_string()),
        None => forward::newest_forwarded(&state).await,
    };
    // Only this request can say what the browser is: the User-Agent header carries OS/browser,
    // `client` carries what no header does (screen, locale, client hints).
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let client = form
        .fields
        .get("client")
        .and_then(|value| serde_json::from_str::<ClientReportedInfo>(value).ok())
        .filter(|client| !client.is_empty());
    let url = form
        .fields
        .get("url")
        .map(String::as_str)
        .and_then(feedback_url);
    match forward::submit_feedback(
        &state,
        &forward::Feedback {
            message: &clean,
            name: &contact.name,
            email: &contact.email,
            user_id: &user_id,
            event_id: event_id.as_deref(),
            url: url.as_deref(),
            user_agent,
            client,
            attachments: &attachments,
        },
    )
    .await
    {
        Ok(()) => {
            report::reporter().capture(
                lanrurugi_storage::diagnostics::Level::Info,
                "feedback submitted",
                report::Attrs::new().tag("source", "feedback"),
            );
            Json(json!({ "sent": true, "remaining_today": remaining })).into_response()
        }
        Err(reason) => error(StatusCode::CONFLICT, "diagnostics_feedback", reason),
    }
}

// ---------------------------------------------------------------- window

async fn get_window(State(state): State<AppState>) -> Response {
    Json(forward::window(&state).await.to_json()).into_response()
}

/// Recorded when the administrator opens a forwarding window: a deliberate action worth having in
/// the history next to whatever it captures.
fn note_window_closed() {
    report::reporter().capture(
        lanrurugi_storage::diagnostics::Level::Info,
        "forwarding window closed",
        report::Attrs::new(),
    );
}

fn note_window_opened(seconds: i64) {
    report::reporter().capture(
        lanrurugi_storage::diagnostics::Level::Info,
        "forwarding window opened",
        report::Attrs::new().extra("duration_secs", seconds),
    );
}

async fn post_window(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    let duration = body
        .get("duration_secs")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    match forward::open_window(&state, duration).await {
        Ok(window) => {
            if window.open {
                note_window_opened(duration as i64);
            }
            Json(window.to_json()).into_response()
        }
        Err(message) => error(StatusCode::CONFLICT, "diagnostics_window", message),
    }
}

async fn delete_window(State(state): State<AppState>) -> Response {
    match forward::close_window(&state).await {
        Ok(window) => {
            note_window_closed();
            Json(window.to_json()).into_response()
        }
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

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod capture_spawn_tests {
    use super::*;

    /// The regression this guards: on a thread with no runtime, `tokio::spawn` panics, and a panic
    /// raised while a panic hook is running aborts the whole server (measured 2026-10-31, 21
    /// restarts). Capture must drop the future instead.
    #[test]
    fn spawning_capture_from_a_plain_thread_never_panics() {
        let outcome = std::thread::spawn(|| spawn_capture(async {}))
            .join()
            .expect("spawn_capture must not panic off-runtime");
        // Either it found the runtime captured at init (another test initialised capture) or it
        // declined to spawn; both are acceptable, panicking is not.
        let _ = outcome;
    }
}

#[cfg(test)]
mod feedback_tests {
    use super::*;

    fn fresh() -> FeedbackBudget {
        FeedbackBudget {
            started: 0,
            used: 0,
            last_hash: 0,
            last_at: 0,
        }
    }

    #[test]
    fn an_empty_note_is_refused() {
        assert!(admit(&mut fresh(), 1_000, "   ").is_err());
    }

    #[test]
    fn an_oversized_note_is_refused() {
        let note = "x".repeat(FEEDBACK_MAX_CHARS + 1);
        let error = admit(&mut fresh(), 1_000, &note).unwrap_err();
        assert!(error.contains("longer than"), "{error}");
    }

    #[test]
    fn the_same_note_twice_is_deduplicated() {
        let mut budget = fresh();
        assert!(admit(&mut budget, 1_000, "same note").is_ok());
        let error = admit(&mut budget, 1_010, "same note").unwrap_err();
        assert!(error.contains("already submitted"), "{error}");
        // After the dedupe window a deliberate re-send is allowed again.
        assert!(admit(&mut budget, 1_000 + FEEDBACK_DEDUPE_SECS + 1, "same note").is_ok());
    }

    #[test]
    fn the_rate_limit_trips_and_recovers_next_window() {
        let mut budget = fresh();
        for index in 0..FEEDBACK_PER_WINDOW {
            assert!(admit(&mut budget, 1_000, &format!("note {index}")).is_ok());
        }
        let error = admit(&mut budget, 1_010, "one too many").unwrap_err();
        assert!(error.contains("too many notes"), "{error}");
        // A new window resets the allowance.
        assert!(admit(&mut budget, 1_000 + FEEDBACK_WINDOW_SECS, "next window").is_ok());
    }

    #[test]
    fn a_note_needs_a_name_and_a_usable_email() {
        assert!(feedback_contact("", "jade@example.com").is_err());
        assert!(feedback_contact("Jade", "").is_err());
        assert!(feedback_contact("Jade", "not-an-email").is_err());
        assert!(feedback_contact("Jade", "two@at@example.com").is_err());
        assert!(feedback_contact("Jade", "has space@example.com").is_err());
        let contact = feedback_contact(" Jade ", " jade@example.com ").unwrap();
        assert_eq!(contact.name, "Jade");
        assert_eq!(contact.email, "jade@example.com");
        // Control characters are stripped rather than rejected: a pasted newline is not an attack.
        assert_eq!(feedback_contact("Ja\nde", "a@b.c").unwrap().name, "Jade");
    }

    #[test]
    fn a_reported_url_must_be_a_local_path() {
        assert_eq!(feedback_url("/config").as_deref(), Some("/config"));
        assert_eq!(feedback_url(" https://elsewhere.test/x "), None);
        assert_eq!(feedback_url("relative/path"), None);
        assert_eq!(feedback_url(&format!("/{}", "x".repeat(600))), None);
    }
}

/// Kept out of the route module so the decode can be unit-tested without an `AppState`.
pub mod ingest {
    use lanrurugi_storage::diagnostics::{
        Breadcrumb, DiagnosticEvent, DiagnosticEventId, Fingerprint, Level, Source,
    };
    use serde_json::Value;

    pub fn decode(
        body: &[u8],
        profile: crate::diagnostics::scrub::Profile,
    ) -> Result<Option<DiagnosticEvent>, String> {
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
        Ok(Some(project(&header, &event, body, profile)))
    }

    fn project(
        header: &Value,
        event: &Value,
        raw: &[u8],
        profile: crate::diagnostics::scrub::Profile,
    ) -> DiagnosticEvent {
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
        let explicit_source = event
            .get("tags")
            .and_then(|tags| tags.get("source"))
            .and_then(|value| value.as_str());
        let source = match explicit_source {
            Some(source) => Source::from_wire(source),
            None => Source::from_wire(platform.as_deref().unwrap_or("rust")),
        };
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
                            profile,
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
            message: crate::diagnostics::scrub::text(&message, profile),
            stack: stack.map(|stack| crate::diagnostics::scrub::text(&stack, profile)),
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
        // The panel's own type has always expected this here; without it the open window, its
        // countdown and its stop button never rendered (the `/diagnostics/window` route existed,
        // but nothing on the settings path ever read it).
        "window": forward::window(&state).await.to_json(),
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
    match ingest::decode(&body, settings.scrub_profile()) {
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
        Ok(None) => {
            // Not an error event: a session (release health) or a transaction. The SDK sends these
            // in their own envelopes, so they are forwarded as-is rather than stored — the store is
            // for issues, the tracker is for session/transaction statistics.
            forward::forward_envelope(&state, &body).await;
            axum::http::StatusCode::NO_CONTENT.into_response()
        }
        Err(message) => error(
            axum::http::StatusCode::BAD_REQUEST,
            "diagnostics_envelope",
            message,
        ),
    }
}
