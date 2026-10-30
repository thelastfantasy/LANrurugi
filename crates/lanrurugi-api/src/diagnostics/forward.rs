//! The optional forwarder (FR-018…FR-022): one tracker, one window, enforced where data leaves.
//!
//! The window is server state (`diagnostics_window_deadline` on `LRR_CONFIG`), not a browser flag:
//! a reload, a second tab or a restart cannot extend it. Every send re-reads the deadline, so the
//! moment it expires nothing more goes out.

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_storage::device_info::{ClientReportedInfo, UserAgentInfo};
use lanrurugi_storage::diagnostics::DiagnosticEvent;
use lanrurugi_storage::keys::CONFIG_KEY;
use serde_json::{json, Value};
use url::Url;

use crate::version::Deployment;
use crate::AppState;

const COUNTERS: &str = "LANRURUGI_DIAGNOSTICS_FORWARD";
const DEADLINE_FIELD: &str = "diagnostics_window_deadline";
/// Ids of events the tracker actually accepted, newest last. A local event id is only usable by the
/// tracker if that event was forwarded, so this is what a user report may reference.
const FORWARDED: &str = "LANRURUGI_DIAGNOSTICS_FORWARDED";

/// Allowed window lengths: five minutes, one hour, one day.
pub const ALLOWED: &[u64] = &[300, 3_600, 86_400];

#[derive(Debug, Clone)]
pub struct Window {
    pub open: bool,
    pub deadline: i64,
    pub remaining_secs: i64,
    pub sent: u64,
    pub failed: u64,
    pub last_error: Option<String>,
}

impl Window {
    pub fn to_json(&self) -> Value {
        json!({
            "open": self.open,
            "deadline": self.deadline,
            "remaining_secs": self.remaining_secs,
            "sent": self.sent,
            "failed": self.failed,
            "last_error": self.last_error,
        })
    }
}

fn now_ms() -> i64 {
    super::now_ms()
}

/// A parsed tracker DSN: where to send, and the key to send with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackerDsn {
    pub envelope_url: String,
    pub key: String,
    /// The numeric project id, for the endpoints that are not the envelope one (crons).
    pub project: String,
    /// Scheme + host + port only: what the settings page may show without leaking the key.
    pub host: String,
}

impl TrackerDsn {
    /// Accepts a Sentry DSN (`https://<key>@<host>/<project>`) or a bare project URL. The project id
    /// is the last path segment; the key is the URL's username, which is exactly the DSN shape.
    /// The check-in endpoint for one monitor slug.
    pub fn cron_url(&self, slug: &str) -> String {
        format!("{}/api/{}/cron/{}", self.host, self.project, slug)
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let url = Url::parse(raw.trim()).ok()?;
        if !matches!(url.scheme(), "http" | "https") {
            return None;
        }
        let project = url
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|segment| !segment.is_empty())
            .map(str::to_string)?;
        let key = if url.username().is_empty() {
            "lanrurugi".to_string()
        } else {
            url.username().to_string()
        };
        let mut base = url.clone();
        base.set_username("").ok()?;
        base.set_password(None).ok()?;
        base.set_path("");
        base.set_query(None);
        base.set_fragment(None);
        let host = base.as_str().trim_end_matches('/').to_string();
        let envelope_url = format!("{host}/api/{project}/envelope/");
        Some(Self {
            envelope_url,
            key,
            project,
            host,
        })
    }
}

pub async fn window(state: &AppState) -> Window {
    let deadline = read_deadline(state).await;
    let (sent, failed, last_error) = counters(state).await;
    let now = now_ms();
    Window {
        open: deadline > now,
        deadline,
        remaining_secs: ((deadline - now).max(0)) / 1000,
        sent,
        failed,
        last_error,
    }
}

pub async fn open_window(state: &AppState, duration_secs: u64) -> Result<Window, String> {
    if !ALLOWED.contains(&duration_secs) {
        return Err("the window must be 5 minutes, 1 hour or 1 day.".to_string());
    }
    let settings = crate::diagnostics_settings::load(state).await;
    let Some(raw) = settings.tracker_dsn() else {
        return Err("configure a tracker first.".to_string());
    };
    if TrackerDsn::parse(raw).is_none() {
        return Err("the configured tracker address is not a valid DSN.".to_string());
    }
    // A window's counters describe that window. Without this they accumulate forever and the
    // panel shows a fresh five-minute window above totals from months ago.
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn.del(COUNTERS).await;
    }
    write_deadline(state, now_ms() + (duration_secs as i64) * 1000).await?;
    Ok(window(state).await)
}

pub async fn close_window(state: &AppState) -> Result<Window, String> {
    write_deadline(state, 0).await?;
    Ok(window(state).await)
}

async fn read_deadline(state: &AppState) -> i64 {
    let Ok(mut conn) = state.redis.config.get().await else {
        return 0;
    };
    let value: Option<String> = conn.hget(CONFIG_KEY, DEADLINE_FIELD).await.unwrap_or(None);
    value.and_then(|v| v.trim().parse().ok()).unwrap_or(0)
}

async fn write_deadline(state: &AppState, deadline: i64) -> Result<(), String> {
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let _: () = conn
        .hset(CONFIG_KEY, DEADLINE_FIELD, deadline.to_string())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

async fn counters(state: &AppState) -> (u64, u64, Option<String>) {
    let Ok(mut conn) = state.redis.config.get().await else {
        return (0, 0, None);
    };
    let values: std::collections::HashMap<String, String> =
        conn.hgetall(COUNTERS).await.unwrap_or_default();
    (
        values.get("sent").and_then(|v| v.parse().ok()).unwrap_or(0),
        values
            .get("failed")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        values.get("last_error").cloned(),
    )
}

async fn bump(state: &AppState, field: &str, error: Option<&str>) {
    let Ok(mut conn) = state.redis.config.get().await else {
        return;
    };
    let _: Result<i64, _> = conn.hincr(COUNTERS, field, 1).await;
    if let Some(error) = error {
        let _: Result<(), _> = conn.hset(COUNTERS, "last_error", error).await;
    }
}

/// Forwards one envelope unchanged (sessions, transactions). Same rules as events: only while a
/// window is open, and only to a configured tracker.
pub async fn forward_envelope(state: &AppState, body: &[u8]) {
    let deadline = read_deadline(state).await;
    if deadline <= now_ms() {
        return;
    }
    let settings = crate::diagnostics_settings::load(state).await;
    let Some(raw) = settings.tracker_dsn() else {
        return;
    };
    let Some(dsn) = TrackerDsn::parse(raw) else {
        return;
    };
    let auth = format!(
        "Sentry sentry_version=7, sentry_key={}, sentry_client=lanrurugi/{}",
        dsn.key,
        crate::version::current_version()
    );
    let _ = reqwest::Client::new()
        .post(&dsn.envelope_url)
        .header("X-Sentry-Auth", auth)
        .header("Content-Type", "application/x-sentry-envelope")
        .body(body.to_vec())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
}

/// Records an event id the tracker accepted, so a later user report can point at a real event
/// instead of the all-zero "unattached" id (which the tracker renders as a dead link).
async fn remember_forwarded(state: &AppState, event_id: &str) {
    let Ok(mut connection) = state.redis.config.get().await else {
        return;
    };
    let _: Result<(), _> = connection.zadd(FORWARDED, event_id, now_ms()).await;
    // Keep the tail short: only the newest id is ever read, and this must not grow without bound.
    let _: Result<(), _> = connection.zremrangebyrank(FORWARDED, 0, -201).await;
}

/// The newest event id the tracker accepted, if any.
pub async fn newest_forwarded(state: &AppState) -> Option<String> {
    let mut connection = state.redis.config.get().await.ok()?;
    let ids: Vec<String> = connection.zrevrange(FORWARDED, 0, 0).await.ok()?;
    ids.into_iter().next()
}

/// One screenshot sent with a note. Already WebP by the time it gets here — the conversion and
/// its limits live in `diagnostics::attachment`.
#[derive(Debug)]
pub struct FeedbackAttachment {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// One feedback submission: what the administrator wrote, and everything the two sides of the
/// connection know about themselves. The server fills this from the request (User-Agent) and this
/// deployment's own identity; the browser fills `client`, `url` and the screenshots.
pub struct Feedback<'a> {
    pub message: &'a str,
    pub name: &'a str,
    pub email: &'a str,
    /// Opaque hash of the reporter's identity — `user.id`, never the account itself.
    pub user_id: &'a str,
    /// The error event this note is about, when one was forwarded before it.
    pub event_id: Option<&'a str>,
    /// The page the note was written on (path only, no query string).
    pub url: Option<&'a str>,
    pub user_agent: Option<&'a str>,
    /// Browser-side facts no header carries — see `clientReportedInfo.ts`.
    pub client: Option<ClientReportedInfo>,
    /// WebP screenshots, sent as `attachment` items in the same envelope.
    pub attachments: &'a [FeedbackAttachment],
}

/// Submits a user report as a Sentry `feedback` item (the current spec; `user_report` is its
/// deprecated predecessor). Sent with this instance's own DSN, never from a browser.
pub async fn submit_feedback(state: &AppState, report: &Feedback<'_>) -> Result<(), String> {
    let settings = crate::diagnostics_settings::load(state).await;
    let Some(raw) = settings.tracker_dsn() else {
        return Err("configure a tracker first.".to_string());
    };
    let Some(dsn) = TrackerDsn::parse(raw) else {
        return Err("the configured tracker address is not a valid DSN.".to_string());
    };
    let at_ms = now_ms();
    // The feedback's own id. The event it is *about* goes in `associated_event_id`; reusing that id
    // here would make the tracker treat two different things as one.
    let event_id = uuid::Uuid::new_v4().simple().to_string();
    let event = feedback_event(report, &event_id, at_ms, &crate::version::deployment());
    let envelope = feedback_envelope(&event_id, at_ms, &event, report.attachments);
    let auth = format!(
        "Sentry sentry_version=7, sentry_key={}, sentry_client=lanrurugi/{}",
        dsn.key,
        crate::version::current_version()
    );
    let response = reqwest::Client::new()
        .post(&dsn.envelope_url)
        .header("X-Sentry-Auth", auth)
        .header("Content-Type", "application/x-sentry-envelope")
        .body(envelope)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("tracker answered {}", response.status()))
    }
}

/// The `feedback` item's payload: a normal event with a `contexts.feedback` object. Device, OS,
/// browser and app go in their own native contexts so the tracker renders them in its usual
/// panels, not as an opaque blob.
fn feedback_event(
    report: &Feedback<'_>,
    event_id: &str,
    at_ms: i64,
    deployment: &Deployment,
) -> Value {
    let ua = crate::device_info::parse_user_agent(report.user_agent);
    let client = report.client.as_ref();

    let mut contexts = serde_json::Map::new();
    let mut feedback = json!({
        "message": report.message,
        "contact_email": report.email,
        "name": report.name,
        "source": "api",
    });
    if let Some(event_id) = report.event_id {
        feedback["associated_event_id"] = json!(event_id);
    }
    if let Some(url) = report.url {
        feedback["url"] = json!(url);
    }
    contexts.insert("feedback".into(), feedback);
    if let Some(ua) = ua.as_ref() {
        // Client Hints carry the full version where the frozen UA string stops at the major one.
        let browser_version = client
            .and_then(|client| client.uach_ua_full_version.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(ua.browser_version.as_str());
        contexts.insert(
            "browser".into(),
            json!({ "name": ua.browser, "version": browser_version }),
        );
        contexts.insert("os".into(), os_context(ua, client));
    } else if let Some(platform) = client.and_then(|client| client.uach_platform.as_deref()) {
        // No UA to parse, but Client Hints still name the OS.
        let mut os = json!({ "name": platform });
        if let Some(version) = client
            .and_then(|client| client.uach_platform_version.as_deref())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            os["version"] = json!(version);
        }
        contexts.insert("os".into(), os);
    }
    if let Some(device) = device_context(ua.as_ref(), client) {
        contexts.insert("device".into(), device);
    }
    contexts.insert("app".into(), app_context(deployment));
    contexts.insert(
        "lanrurugi".into(),
        deployment_context(report, at_ms, deployment),
    );

    let release = deployment
        .release
        .clone()
        .unwrap_or_else(|| deployment.version.to_string());
    let mut event = json!({
        "event_id": event_id,
        // The event's own type, distinct from the envelope item's: trackers key "is this a
        // feedback?" off this field (rustrak does), and a plain error event carrying a feedback
        // context is not one.
        "type": "feedback",
        "timestamp": at_ms as f64 / 1000.0,
        "platform": "javascript",
        "level": "info",
        "sdk": { "name": "lanrurugi", "version": deployment.version },
        "release": release.clone(),
        "environment": deployment.environment,
        "server_name": deployment.hostname,
        "tags": {
            "source": "feedback",
            "deployment": deployment.hostname,
            "environment": deployment.environment,
            "release": release,
        },
        "contexts": contexts,
        "user": {
            "id": report.user_id,
            "email": report.email,
            "username": report.name,
        },
    });
    if let Some(user_agent) = report.user_agent {
        let mut headers = json!({ "User-Agent": user_agent });
        if let Some(languages) = client.and_then(|client| client.languages.as_deref()) {
            headers["Accept-Language"] = json!(languages.replace(", ", ","));
        }
        event["request"] = json!({
            "url": report.url.unwrap_or("/"),
            "headers": headers,
        });
    }
    event
}

/// `contexts.os`: the UA's own version, with Client Hints' (Chromium-only) real build when the
/// two differ — the plain UA string freezes OS versions for privacy.
fn os_context(ua: &UserAgentInfo, client: Option<&ClientReportedInfo>) -> Value {
    let platform_version = client
        .and_then(|client| client.uach_platform_version.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let ua_version = (!ua.os_version.trim().is_empty()).then_some(ua.os_version.as_str());
    let version = ua_version.or(platform_version);
    let mut os = json!({ "name": ua.os });
    if let Some(version) = version {
        os["version"] = json!(version);
    }
    if let Some(build) = platform_version.filter(|build| Some(*build) != version) {
        os["build"] = json!(build);
    }
    os
}

/// `contexts.device`: only fields with a real value; an absent browser API stays absent rather than
/// becoming a zero.
fn device_context(
    ua: Option<&UserAgentInfo>,
    client: Option<&ClientReportedInfo>,
) -> Option<Value> {
    let mut device = serde_json::Map::new();
    if let Some(ua) = ua {
        if !ua.category.is_empty() {
            device.insert("family".into(), json!(ua.category));
        }
        if !ua.vendor.is_empty() {
            device.insert("brand".into(), json!(ua.vendor));
        }
    }
    if let Some(client) = client {
        if let Some(width) = client.screen_width {
            device.insert("screen_width_pixels".into(), json!(width));
        }
        if let Some(height) = client.screen_height {
            device.insert("screen_height_pixels".into(), json!(height));
        }
        if let Some(ratio) = client.device_pixel_ratio {
            device.insert("screen_density".into(), json!(ratio));
        }
        if let Some(orientation) = client.screen_orientation.as_deref() {
            // Sentry's field is the axis ("portrait"/"landscape"), not the screen API's full type.
            device.insert(
                "orientation".into(),
                json!(orientation.split('-').next().unwrap_or(orientation)),
            );
        }
        if let Some(count) = client.hardware_concurrency {
            device.insert("processor_count".into(), json!(count));
        }
        if let Some(model) = client
            .uach_model
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            device.insert("model".into(), json!(model));
        }
        if let Some(arch) = client
            .uach_architecture
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            // Client Hints splits these; Sentry's `arch` wants the usual joined spelling.
            let arch = match client.uach_bitness.as_deref() {
                Some("64") if arch == "x86" => "x86_64".to_string(),
                Some(bitness) => format!("{arch}{bitness}"),
                None => arch.to_string(),
            };
            device.insert("arch".into(), json!(arch));
        }
        if let Some(gib) = client.device_memory_gib {
            device.insert(
                "memory_size".into(),
                json!((f64::from(gib) * 1024.0 * 1024.0 * 1024.0) as u64),
            );
        }
        if let Some(locale) = &client.language {
            device.insert("locale".into(), json!(locale));
        }
        if let Some(timezone) = &client.timezone {
            device.insert("timezone".into(), json!(timezone));
        }
        if let Some(online) = client.online {
            device.insert("online".into(), json!(online));
        }
    }
    (!device.is_empty()).then_some(Value::Object(device))
}

/// `contexts.app`: the instance's identity, in the fields the tracker already knows how to show.
fn app_context(deployment: &Deployment) -> Value {
    let mut app = json!({
        "app_name": "lanrurugi",
        "app_version": deployment.version,
        "app_start_time": humantime_secs(deployment.started_at_ms),
    });
    if let Some(release) = &deployment.release {
        app["app_build"] = json!(release);
    }
    if let Some(memory) = deployment.process_memory_bytes {
        app["app_memory"] = json!(memory);
    }
    app
}

/// `contexts.lanrurugi`: everything that has no native Sentry field — this deployment's start
/// time, uptime, build time, server OS, and the raw client-reported facts behind the mapped ones.
fn deployment_context(report: &Feedback<'_>, at_ms: i64, deployment: &Deployment) -> Value {
    let mut context = json!({
        "deployment": deployment.hostname,
        "environment": deployment.environment,
        "version": deployment.version,
        "server_os": deployment.os,
        "server_arch": deployment.arch,
        "containerized": deployment.containerized,
        "started_at": humantime_secs(deployment.started_at_ms),
        "uptime_secs": deployment.uptime_secs,
        "server_time": humantime_secs(at_ms),
    });
    if let Some(release) = &deployment.release {
        context["release"] = json!(release);
    }
    if let Some(built_at) = deployment.built_at_ms {
        context["built_at"] = json!(humantime_secs(built_at));
    }
    if let Some(memory) = deployment.process_memory_bytes {
        context["process_memory_bytes"] = json!(memory);
    }
    if let Some(user_agent) = report.user_agent {
        context["user_agent"] = json!(user_agent);
    }
    if let Some(client) = report.client.as_ref() {
        if let Ok(Value::Object(fields)) = serde_json::to_value(client) {
            for (key, value) in fields {
                if !value.is_null() {
                    context[key] = value;
                }
            }
        }
    }
    context
}

/// An envelope with one `feedback` item and, optionally, one `attachment` item per screenshot.
/// Bytes, not a string: attachment payloads are binary (a `Vec<u8>` body also skips the UTF-8
/// validation `reqwest` would do for text). `length` is what the SDKs send and what a strict
/// envelope reader expects.
fn feedback_envelope(
    event_id: &str,
    at_ms: i64,
    event: &Value,
    attachments: &[FeedbackAttachment],
) -> Vec<u8> {
    let body = event.to_string();
    let mut out = Vec::with_capacity(body.len() + 256);
    out.extend_from_slice(
        format!(
            "{{\"event_id\":\"{event_id}\",\"sent_at\":\"{}\"}}\n",
            humantime_secs(at_ms)
        )
        .as_bytes(),
    );
    out.extend_from_slice(
        format!(
            "{{\"type\":\"feedback\",\"length\":{},\"content_type\":\"application/json\"}}\n",
            body.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(body.as_bytes());
    out.push(b'\n');
    for attachment in attachments {
        let filename = serde_json::to_string(&attachment.filename)
            .unwrap_or_else(|_| "\"feedback.webp\"".to_string());
        out.extend_from_slice(
            format!(
                "{{\"type\":\"attachment\",\"length\":{},\"filename\":{filename},\
                 \"content_type\":\"image/webp\",\"attachment_type\":\"event.attachment\"}}\n",
                attachment.bytes.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&attachment.bytes);
        out.push(b'\n');
    }
    out
}

/// RFC3339 in UTC, seconds precision — what an envelope header expects for `sent_at` and what
/// Sentry contexts expect for a timestamp. `format_instant` stops at minutes (it is a human-facing
/// label), so this one goes through chrono.
fn humantime_secs(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

/// Sends one cron check-in to the tracker, if one is configured. Deliberately not gated on the
/// forwarding window: a monitor is about the task, not about the administrator watching errors.
pub fn cron(state: &AppState, slug: &str, status: &str, duration_ms: Option<i64>) {
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let state = state.clone();
    let slug = slug.to_string();
    let status = status.to_string();
    handle.spawn(async move {
        let settings = crate::diagnostics_settings::load(&state).await;
        let Some(raw) = settings.tracker_dsn() else {
            return;
        };
        let Some(dsn) = TrackerDsn::parse(raw) else {
            return;
        };
        let mut url = format!("{}?status={status}", dsn.cron_url(&slug));
        if let Some(duration) = duration_ms {
            url.push_str(&format!("&duration={}", duration as f64 / 1000.0));
        }
        let _ = reqwest::Client::new()
            .post(url)
            .header(
                "X-Sentry-Auth",
                format!("Sentry sentry_version=7, sentry_key={}", dsn.key),
            )
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
    });
}

/// A cron check-in failed: also record the reason as one event, so the monitor's history has
/// something to read beyond "error".
pub fn cron_error(slug: &str, reason: &str) {
    crate::diagnostics::report::reporter().error(
        reason,
        crate::diagnostics::report::Attrs::new().tag("cron", slug),
    );
}

/// A rejected forward's reason, short enough for the panel. rustrak's error body is
/// `{"error":{"message":…}}`; anything else falls back to the raw (clipped) text.
fn rejection_reason(status: reqwest::StatusCode, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .or_else(|| value.get("message").and_then(Value::as_str))
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.trim().to_string());
    let detail = clip(&detail, 200);
    if detail.is_empty() {
        format!("tracker answered {status}")
    } else {
        format!("tracker answered {status}: {detail}")
    }
}

fn clip(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let clipped: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        format!("{clipped}…")
    } else {
        clipped
    }
}

/// Forwards one stored event if a window is open. Called from the writer, never from a request.
pub async fn maybe_forward(state: &AppState, event: &DiagnosticEvent) {
    let deadline = read_deadline(state).await;
    if deadline <= now_ms() {
        return;
    }
    let settings = crate::diagnostics_settings::load(state).await;
    // A per-source override wins: this is the seam that lets one deployment send web and server
    // events to different projects without a code change.
    let Some(raw) = settings.tracker_dsn_for(event.source.as_str()) else {
        return;
    };
    let Some(dsn) = TrackerDsn::parse(&raw) else {
        bump(state, "failed", Some("invalid DSN")).await;
        return;
    };
    if event.envelope.is_empty() {
        return;
    }
    let client = reqwest::Client::new();
    let auth = format!(
        "Sentry sentry_version=7, sentry_key={}, sentry_client=lanrurugi/{}",
        dsn.key,
        crate::version::current_version()
    );
    let result = client
        .post(&dsn.envelope_url)
        .header("X-Sentry-Auth", auth)
        .header("Content-Type", "application/x-sentry-envelope")
        .body(event.envelope.clone())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;
    match result {
        Ok(response) if response.status().is_success() => {
            remember_forwarded(state, event.id.as_str()).await;
            bump(state, "sent", None).await
        }
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            let reason = rejection_reason(status, &body);
            tracing::warn!(
                event = %event.id,
                source = event.source.as_str(),
                message = %event.message,
                reason = %reason,
                "the tracker rejected a forwarded event"
            );
            bump(state, "failed", Some(&reason)).await
        }
        Err(error) => bump(state, "failed", Some(&error.to_string())).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sentry_dsn_becomes_an_envelope_url() {
        let dsn = TrackerDsn::parse("https://abc123@tracker.example.com:8080/17").unwrap();
        assert_eq!(
            dsn.envelope_url,
            "https://tracker.example.com:8080/api/17/envelope/"
        );
        assert_eq!(dsn.key, "abc123");
    }

    #[test]
    fn http_is_accepted_for_a_lan_tracker() {
        let dsn = TrackerDsn::parse("http://key@192.168.3.214:30077/3").unwrap();
        assert_eq!(
            dsn.envelope_url,
            "http://192.168.3.214:30077/api/3/envelope/"
        );
    }

    #[test]
    fn a_url_without_a_project_is_not_a_dsn() {
        assert!(TrackerDsn::parse("http://192.168.3.214:30077/").is_none());
        assert!(TrackerDsn::parse("not a url").is_none());
        assert!(TrackerDsn::parse("ftp://host/1").is_none());
    }

    #[test]
    fn only_the_documented_durations_are_offered() {
        assert!(ALLOWED.contains(&300));
        assert!(ALLOWED.contains(&3_600));
        assert!(ALLOWED.contains(&86_400));
        assert!(!ALLOWED.contains(&60));
    }

    fn fixture_deployment() -> Deployment {
        Deployment {
            hostname: "lrr-test".to_string(),
            environment: "test".to_string(),
            release: Some("abc1234".to_string()),
            version: "1.2.3",
            started_at_ms: 1_700_000_000_000,
            uptime_secs: 90,
            built_at_ms: Some(1_699_900_000_000),
            os: "linux",
            arch: "x86_64",
            containerized: true,
            process_memory_bytes: Some(64 * 1024 * 1024),
        }
    }

    fn fixture_client() -> ClientReportedInfo {
        ClientReportedInfo {
            screen_width: Some(1080),
            screen_height: Some(2400),
            device_pixel_ratio: Some(2.6),
            screen_orientation: Some("portrait-primary".to_string()),
            language: Some("ja-JP".to_string()),
            languages: Some("ja-JP, en-US".to_string()),
            timezone: Some("Asia/Tokyo".to_string()),
            timezone_offset_minutes: Some(540),
            platform: Some("Linux armv8l".to_string()),
            hardware_concurrency: Some(8),
            device_memory_gib: Some(4.0),
            online: Some(true),
            uach_architecture: Some("arm".to_string()),
            uach_bitness: Some("64".to_string()),
            uach_model: Some("Pixel 7".to_string()),
            ..Default::default()
        }
    }

    const FIREFOX_ANDROID: &str =
        "Mozilla/5.0 (Android 14; Mobile; rv:141.0) Gecko/141.0 Firefox/141.0";

    #[test]
    fn a_feedback_event_carries_device_app_and_deployment_contexts() {
        let report = Feedback {
            message: "the thumbnail is missing",
            name: "Jade",
            email: "jade@example.com",
            user_id: "lrrg-0123456789abcdef",
            event_id: Some("deadbeef"),
            url: Some("/config"),
            user_agent: Some(FIREFOX_ANDROID),
            client: Some(fixture_client()),
            attachments: &[],
        };
        let event = feedback_event(
            &report,
            "0123456789abcdef",
            1_700_000_100_000,
            &fixture_deployment(),
        );
        let contexts = &event["contexts"];

        // The feedback itself, in the modern spec's own context, linked to the error event.
        assert_eq!(contexts["feedback"]["message"], "the thumbnail is missing");
        assert_eq!(contexts["feedback"]["contact_email"], "jade@example.com");
        assert_eq!(contexts["feedback"]["name"], "Jade");
        assert_eq!(contexts["feedback"]["associated_event_id"], "deadbeef");
        assert_eq!(contexts["feedback"]["url"], "/config");
        // Native browser/OS/device fields, not a blob.
        assert_eq!(contexts["browser"]["name"], "Firefox");
        assert_eq!(contexts["os"]["name"], "Android");
        assert_eq!(contexts["os"]["version"], "14");
        assert_eq!(contexts["device"]["screen_width_pixels"], 1080);
        assert_eq!(contexts["device"]["orientation"], "portrait");
        assert_eq!(contexts["device"]["processor_count"], 8);
        assert_eq!(contexts["device"]["memory_size"], 4 * 1024 * 1024 * 1024u64);
        assert_eq!(contexts["device"]["locale"], "ja-JP");
        assert_eq!(contexts["device"]["timezone"], "Asia/Tokyo");
        assert_eq!(contexts["device"]["online"], true);
        assert_eq!(contexts["device"]["model"], "Pixel 7");
        assert_eq!(contexts["device"]["arch"], "arm64");
        // The deployment's own identity and uptime.
        assert_eq!(contexts["app"]["app_name"], "lanrurugi");
        assert_eq!(contexts["app"]["app_version"], "1.2.3");
        assert_eq!(contexts["app"]["app_start_time"], "2023-11-14T22:13:20Z");
        assert_eq!(contexts["lanrurugi"]["deployment"], "lrr-test");
        assert_eq!(contexts["lanrurugi"]["uptime_secs"], 90);
        assert_eq!(contexts["lanrurugi"]["containerized"], true);
        assert_eq!(contexts["lanrurugi"]["timezone"], "Asia/Tokyo");
        // Top-level native fields the tracker groups and displays by.
        assert_eq!(event["type"], "feedback");
        assert_eq!(event["server_name"], "lrr-test");
        assert_eq!(event["release"], "abc1234");
        assert_eq!(event["environment"], "test");
        assert_eq!(event["tags"]["source"], "feedback");
        assert_eq!(event["user"]["username"], "Jade");
        assert_eq!(event["user"]["email"], "jade@example.com");
        assert_eq!(event["request"]["url"], "/config");
        assert_eq!(event["request"]["headers"]["User-Agent"], FIREFOX_ANDROID);
        assert_eq!(
            event["request"]["headers"]["Accept-Language"],
            "ja-JP,en-US"
        );
    }

    #[test]
    fn a_feedback_event_without_a_user_agent_stays_valid() {
        let report = Feedback {
            message: "note",
            name: "Jade",
            email: "jade@example.com",
            user_id: "lrrg-x",
            event_id: None,
            url: None,
            user_agent: None,
            client: None,
            attachments: &[],
        };
        let event = feedback_event(&report, "id", 0, &fixture_deployment());
        assert!(event["contexts"]["feedback"]
            .get("associated_event_id")
            .is_none());
        assert!(event["contexts"].get("browser").is_none());
        assert!(event["contexts"]["lanrurugi"].get("user_agent").is_none());
        assert!(event.get("request").is_none());
        // A missing UA must not invent an OS or device block.
        assert!(event["contexts"].get("os").is_none());
        assert!(event["contexts"].get("device").is_none());
    }

    #[test]
    fn a_rejection_reason_prefers_the_trackers_own_message() {
        let status = reqwest::StatusCode::BAD_REQUEST;
        // rustrak's shape: {"error":{"message":…}}.
        let reason = rejection_reason(
            status,
            r#"{"error":{"error_type":"validation","message":"Invalid event JSON: missing value"}}"#,
        );
        assert_eq!(
            reason,
            "tracker answered 400 Bad Request: Invalid event JSON: missing value"
        );
        // A plain-text body from anything else is still better than no reason at all.
        assert_eq!(
            rejection_reason(status, "  nope\n"),
            "tracker answered 400 Bad Request: nope"
        );
        // No body: the status alone.
        assert_eq!(
            rejection_reason(status, ""),
            "tracker answered 400 Bad Request"
        );
    }

    #[test]
    fn a_long_rejection_body_is_clipped() {
        let long = "x".repeat(500);
        let reason = rejection_reason(reqwest::StatusCode::BAD_REQUEST, &long);
        assert!(reason.ends_with('…'), "{reason}");
        assert_eq!(
            reason.chars().count(),
            "tracker answered 400 Bad Request: ".chars().count() + 201
        );
        assert_eq!(clip("short", 10), "short");
        assert_eq!(clip("abcdef", 3), "abc…");
    }

    #[test]
    fn a_feedback_envelope_is_one_feedback_item() {
        let event = json!({ "event_id": "abc", "platform": "javascript" });
        let bytes = feedback_envelope("abc", 1_700_000_100_000, &event, &[]);
        let text = String::from_utf8(bytes).unwrap();
        let lines: Vec<&str> = text.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert!(lines[0].contains("\"event_id\":\"abc\""));
        assert!(lines[0].contains("\"sent_at\":\"2023-11-14T22:15:00Z\""));
        let header: Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(header["type"], "feedback");
        assert_eq!(header["content_type"], "application/json");
        // The declared length is the byte length of the payload line, as the protocol requires.
        assert_eq!(header["length"], lines[2].len());
        assert!(lines[2].contains("\"platform\":\"javascript\""));
    }

    #[test]
    fn a_screenshot_rides_along_as_an_attachment_item() {
        let event = json!({ "event_id": "abc" });
        let attachment = FeedbackAttachment {
            filename: "reader-bug.webp".to_string(),
            bytes: b"RIFF....WEBP".to_vec(),
        };
        let bytes = feedback_envelope("abc", 1_700_000_100_000, &event, &[attachment]);
        // Header, feedback header, event JSON, attachment header, then the raw WebP bytes.
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.trim_end().split('\n').collect();
        assert_eq!(lines.len(), 5, "{text}");
        let header: Value = serde_json::from_str(lines[3]).unwrap();
        assert_eq!(header["type"], "attachment");
        assert_eq!(header["filename"], "reader-bug.webp");
        assert_eq!(header["content_type"], "image/webp");
        assert_eq!(header["attachment_type"], "event.attachment");
        assert_eq!(header["length"], 12);
        assert_eq!(lines[4].as_bytes(), b"RIFF....WEBP");
    }
}
