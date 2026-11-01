//! Cron check-ins for the tracker's Monitors page.
//!
//! Every long-lived scheduled loop in this process (queue sweep, subscription scheduler, share
//! republisher, stale-rename sweeper, …) reports one check-in per tick. Without these the tracker's
//! Crons page has nothing to plot: a monitor only appears once a check-in names it, and "the job
//! stopped running" is exactly the failure mode an in-process timer cannot report about itself.
//!
//! A check-in is an ordinary Sentry envelope item, so it rides the same transport and forwarding
//! window as everything else — no separate credentials, no second HTTP client.

use std::time::Duration;

/// Reports one run of a scheduled job.
///
/// `slug` is the monitor's identity on the tracker (`lanrurugi-queue-sweep`, …), `status` is `"ok"`
/// or `"error"`, `elapsed` is that run's own duration, and `interval_minutes` is the job's own
/// period — it travels with **every** check-in as `monitor_config`, because the tracker only
/// creates a monitor from a check-in that carries a schedule (verified live 2026-11-01: a
/// check-in without one is accepted with a 200 and creates nothing).
pub fn check_in(slug: &str, status: &str, elapsed: Duration, interval_minutes: u64) {
    // Built by hand rather than through the SDK: `sentry` 0.49 has no `capture_check_in`, and the
    // envelope format is three JSON lines (header, item header, payload).
    let check_in_id = uuid_like_id();
    let payload = serde_json::json!({
        "check_in_id": check_in_id,
        "monitor_slug": slug,
        "status": status,
        "duration": elapsed.as_secs_f64(),
        "environment": crate::version::deployment().environment,
        "release": crate::version::deployment().release,
        "monitor_config": {
            "schedule": { "type": "interval", "value": interval_minutes, "unit": "minute" },
            // One missed run is worth knowing about, and one good run is enough to clear it.
            "checkin_margin": interval_minutes.min(15),
            "max_runtime": (interval_minutes * 2).max(5),
            "failure_issue_threshold": 1,
            "recovery_threshold": 1,
        },
    });
    let header = serde_json::json!({
        "event_id": uuid_like_id(),
        "sent_at": chrono::Utc::now().to_rfc3339(),
    });
    let item = serde_json::json!({ "type": "check_in" });
    let body = format!("{header}\n{item}\n{payload}\n");
    forward(&body);
}

fn forward(body: &str) {
    let Some(state) = super::CAPTURE_STATE.get().cloned() else {
        return;
    };
    let body = body.to_owned();
    super::spawn_capture(async move {
        crate::diagnostics::forward::forward_envelope(&state, body.as_bytes()).await;
    });
}

/// A random hex id of Sentry's own 32-character shape, without taking a uuid dependency for it.
fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mixed = nanos ^ (nanos >> 64) ^ (u128::from(std::process::id()) << 32);
    format!("{mixed:032x}")
}
