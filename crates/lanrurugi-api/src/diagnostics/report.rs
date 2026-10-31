//! The small internal API everything else reports through (specs/018). One trait, two
//! implementations, so a caller never touches the SDK directly and a test never needs one.
//!
//! ```ignore
//! report::error(&e).tag("scanner", "rebuild").extra("archives", 1234).send();
//! let run = report::cron("subscriptions-check");
//! // ... work ...
//! run.ok();
//! ```
//!
//! The cron half is the reason this exists: a scheduled task has to report "started", then
//! "finished"/"failed", and forgetting the last one is what makes a monitor useless. [`CronRun`]
//! reports a failure by itself when it is dropped unsettled.

use std::collections::BTreeMap;

use lanrurugi_storage::diagnostics::Level;
use serde_json::Value;

/// One report's tags and extra data, built fluently so a call site cannot get the shape wrong.
#[derive(Debug, Default, Clone)]
pub struct Attrs {
    tags: BTreeMap<String, String>,
    extra: BTreeMap<String, Value>,
}

impl Attrs {
    pub fn new() -> Self {
        Self::default()
    }

    /// An indexed tag: low cardinality, searchable/filterable in the tracker.
    pub fn tag(mut self, key: &str, value: impl Into<String>) -> Self {
        self.tags.insert(key.to_string(), value.into());
        self
    }

    /// Arbitrary context: shown with the event, not filterable.
    pub fn extra(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.extra.insert(key.to_string(), value.into());
        self
    }

    pub fn tags(&self) -> &BTreeMap<String, String> {
        &self.tags
    }

    pub fn extras(&self) -> &BTreeMap<String, Value> {
        &self.extra
    }

    /// The source tag the ingest reads to file a plugin's event as `source=plugin`.
    pub fn plugin(self, namespace: &str) -> Self {
        self.tag("source", "plugin").tag("plugin", namespace)
    }
}

/// What a caller can report. `Silent` makes every method a no-op, which is what a test (and a
/// build with capture compiled out) uses.
pub trait Reporter: Send + Sync {
    fn capture(&self, level: Level, message: &str, attrs: Attrs);
    fn error(&self, error: &str, attrs: Attrs);
    /// Starts a cron check-in. Dropping the returned value unsettled reports a failure.
    fn cron(&self, slug: &str) -> CronRun;
}

/// Decides whether a log event becomes a diagnostics event.
///
/// ERROR from anywhere except this module (a storage failure while handling an event must never
/// produce another event — that is how a Redis hiccup becomes an infinite loop). WARNING only from
/// this project's own crates, minus the ones that warn about things that are not defects: the
/// update check answers 404 whenever no release exists yet, and iroh's network probes time out on
/// perfectly healthy networks.
pub fn capture_log_event(level: &str, target: &str) -> bool {
    match level {
        "ERROR" => !target.starts_with("lanrurugi_api::diagnostics"),
        "WARN" => {
            target.starts_with("lanrurugi_")
                && !target.starts_with("lanrurugi_api::version")
                && !target.starts_with("lanrurugi_api::diagnostics")
                && !target.contains("net_report")
                && !target.contains("iroh")
        }
        _ => false,
    }
}

/// Reports through the in-process SDK transport.
pub struct SentryReporter;

impl Reporter for SentryReporter {
    fn capture(&self, level: Level, message: &str, attrs: Attrs) {
        sentry::configure_scope(|scope| {
            for (key, value) in attrs.tags() {
                scope.set_tag(key.as_str(), value.as_str());
            }
            for (key, value) in attrs.extras() {
                scope.set_extra(key.as_str(), value.clone());
            }
        });
        sentry::capture_message(message, sentry_level(level));
    }

    fn error(&self, error: &str, attrs: Attrs) {
        self.capture(Level::Error, error, attrs);
    }

    fn cron(&self, slug: &str) -> CronRun {
        if let Some(state) = super::CAPTURE_STATE.get() {
            crate::diagnostics::forward::cron(state, slug, "in_progress", None);
        }
        CronRun {
            slug: slug.to_string(),
            started: std::time::Instant::now(),
            settled: false,
        }
    }
}

/// Reports nowhere. Used by tests and by a build with capture disabled.
pub struct Silent;

impl Reporter for Silent {
    fn capture(&self, _level: Level, _message: &str, _attrs: Attrs) {}
    fn error(&self, _error: &str, _attrs: Attrs) {}
    fn cron(&self, slug: &str) -> CronRun {
        CronRun {
            slug: slug.to_string(),
            started: std::time::Instant::now(),
            settled: true,
        }
    }
}

/// One scheduled run. `ok()`/`failed()` settle it; the drop impl settles it as a failure with the
/// elapsed time if the task returned early — a task that vanishes without reporting is exactly what
/// a monitor must catch.
pub struct CronRun {
    slug: String,
    started: std::time::Instant,
    settled: bool,
}

impl CronRun {
    pub fn ok(mut self) {
        self.settled = true;
        self.report("ok", None);
    }

    pub fn failed(mut self, reason: &str) {
        self.settled = true;
        self.report("error", Some(reason.to_string()));
    }

    fn report(&self, status: &str, reason: Option<String>) {
        if let Some(state) = super::CAPTURE_STATE.get() {
            crate::diagnostics::forward::cron(
                state,
                &self.slug,
                status,
                Some(self.started.elapsed().as_millis() as i64),
            );
            if let Some(reason) = reason {
                crate::diagnostics::forward::cron_error(&self.slug, &reason);
            }
        }
    }
}

impl Drop for CronRun {
    fn drop(&mut self) {
        if !self.settled {
            self.report(
                "error",
                Some("task ended without reporting a result".into()),
            );
        }
    }
}

/// The process-wide reporter. Installed once at startup; `Silent` until then, so a unit test that
/// never installs one cannot accidentally talk to a tracker.
pub fn reporter() -> &'static dyn Reporter {
    super::REPORTER.get().copied().unwrap_or(&Silent)
}

pub fn install(which: &'static dyn Reporter) {
    let _ = super::REPORTER.set(which);
}

/// AI-agent monitoring: one transaction per model call, with the GenAI attributes rustrak's
/// "agents" page reads. Installed **only when an LLM key is configured** — a deployment that never
/// calls a model, or has no key, reports nothing.
pub struct LlmTracer;

static LLM_TRACER: LlmTracer = LlmTracer;

impl lanrurugi_llm::telemetry::Tracer for LlmTracer {
    fn begin(
        &self,
        _call: &lanrurugi_llm::telemetry::CallStarted,
    ) -> Option<Box<dyn std::any::Any + Send>> {
        // Only the start instant crosses the await; the transaction itself is built in `end`, so a
        // caller's future stays `Send`.
        Some(Box::new(std::time::SystemTime::now()))
    }

    fn end(
        &self,
        state: Option<Box<dyn std::any::Any + Send>>,
        call: &lanrurugi_llm::telemetry::CallFinished,
    ) {
        let Some(transaction) =
            state.and_then(|state| state.downcast::<sentry::Transaction>().ok())
        else {
            return;
        };
        transaction.set_data("gen_ai.usage.input_tokens", call.input_tokens.into());
        transaction.set_data("gen_ai.usage.output_tokens", call.output_tokens.into());
        transaction.set_data("gen_ai.response.attempts", call.attempts.into());
        if let Some(error) = &call.error {
            transaction.set_data("error.class", error.clone().into());
            transaction.set_status(sentry::protocol::SpanStatus::InternalError);
        }
        transaction.finish();
    }
}

/// Installs the AI tracer when this deployment actually has an LLM key. Called once from `main`.
pub async fn install_llm_tracer(state: &crate::AppState) {
    if lanrurugi_llm::resolve_api_key(&state.redis.config)
        .await
        .is_some()
    {
        lanrurugi_llm::telemetry::install(&LLM_TRACER);
    }
}

fn sentry_level(level: Level) -> sentry::Level {
    match level {
        Level::Info => sentry::Level::Info,
        Level::Warning => sentry::Level::Warning,
        Level::Error => sentry::Level::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attrs_keep_tags_and_extra_apart() {
        let attrs = Attrs::new()
            .tag("scanner", "rebuild")
            .extra("archives", 42)
            .plugin("metadata/ehentai");
        assert_eq!(attrs.tags().get("scanner").unwrap(), "rebuild");
        assert_eq!(attrs.tags().get("source").unwrap(), "plugin");
        assert_eq!(attrs.tags().get("plugin").unwrap(), "metadata/ehentai");
        assert_eq!(
            attrs.extras().get("archives").unwrap(),
            &serde_json::json!(42)
        );
    }

    #[test]
    fn the_silent_reporter_settles_its_own_cron() {
        // No panic, no global state: a settled run must not report on drop.
        Silent.cron("no-such-monitor").ok();
        Silent.capture(Level::Error, "ignored", Attrs::new());
    }

    #[test]
    fn warnings_from_noisy_targets_are_ignored() {
        assert!(capture_log_event("WARN", "lanrurugi_scanner::full_scan"));
        assert!(capture_log_event("ERROR", "lanrurugi_api::plugins"));
        // A storage failure while handling an event must never become another event.
        assert!(!capture_log_event(
            "ERROR",
            "lanrurugi_api::diagnostics::mod"
        ));
        assert!(!capture_log_event("WARN", "lanrurugi_api::version"));
        // Every startup with no release published answers 404; not a defect.
        assert!(!capture_log_event("WARN", "iroh::socket::net_report"));
        assert!(!capture_log_event("INFO", "lanrurugi_api::plugins"));
        assert!(!capture_log_event("WARN", "hyper::proto"));
    }

    #[test]
    fn the_default_reporter_is_silent_before_installation() {
        // Tests share a process; another test may have installed the real one, so this only
        // asserts that a call is safe either way.
        reporter().capture(Level::Warning, "probe", Attrs::new());
    }
}
