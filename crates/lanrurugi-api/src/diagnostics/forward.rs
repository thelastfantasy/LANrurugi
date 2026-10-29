//! The optional forwarder (FR-018…FR-022): one tracker, one window, enforced where data leaves.
//!
//! The window is server state (`diagnostics_window_deadline` on `LRR_CONFIG`), not a browser flag:
//! a reload, a second tab or a restart cannot extend it. Every send re-reads the deadline, so the
//! moment it expires nothing more goes out.

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_storage::diagnostics::DiagnosticEvent;
use lanrurugi_storage::keys::CONFIG_KEY;
use serde_json::{json, Value};
use url::Url;

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

/// Submits a user report (Sentry's "feedback") to the tracker: a note the administrator wrote,
/// optionally tied to one event. Sent with this instance's own DSN, never from a browser.
pub async fn submit_user_report(
    state: &AppState,
    message: &str,
    email: Option<&str>,
    event_id: Option<&str>,
    user_id: Option<&str>,
) -> Result<(), String> {
    let settings = crate::diagnostics_settings::load(state).await;
    let Some(raw) = settings.tracker_dsn() else {
        return Err("configure a tracker first.".to_string());
    };
    let Some(dsn) = TrackerDsn::parse(raw) else {
        return Err("the configured tracker address is not a valid DSN.".to_string());
    };
    let mut report = serde_json::json!({ "comments": message });
    if let Some(user_id) = user_id {
        // `user.id` is what the tracker counts distinct reporters by; it is an opaque hash, never
        // the account's own identity.
        report["user"] = serde_json::json!({ "id": user_id });
    }
    if let Some(email) = email.filter(|value| !value.trim().is_empty()) {
        report["email"] = serde_json::json!(email.trim());
    }
    if let Some(event_id) = event_id.filter(|value| !value.trim().is_empty()) {
        report["event_id"] = serde_json::json!(event_id.trim());
    }
    let envelope = format!(
        "{{\"event_id\":\"{}\",\"sent_at\":\"{}\"}}\n{{\"type\":\"user_report\"}}\n{}\n",
        event_id.unwrap_or("00000000000000000000000000000000"),
        humantime_secs(now_ms()),
        report
    );
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

/// RFC3339 in UTC, seconds precision — what an envelope header expects for `sent_at`.
fn humantime_secs(millis: i64) -> String {
    crate::diagnostics::text::format_instant(millis)
        .replace(" UTC", "Z")
        .replace(' ', "T")
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
            bump(state, "failed", Some(&format!("tracker answered {status}"))).await
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
}
