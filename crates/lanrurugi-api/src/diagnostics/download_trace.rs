//! One trace per download task: a `download_url` transaction with a child span per phase.
//!
//! The spans are started and ended at the same boundaries that advance the job's `phases` timeline
//! (`lanrurugi_core::jobs`), so the two views of one task cannot disagree — a tracer that measured
//! on its own schedule would eventually contradict the durations the queue shows (029 contract T2).
//!
//! Keyed by job id because the boundaries are spread across helpers that carry the id and nothing
//! else: the transfer callback is a `move` closure spawned off the task, and the ingest half lives
//! in `download_manager`, which is shared with subscriptions and uploads. A registry makes the
//! lookup possible from all of them without threading a download-only argument through signatures
//! that have no business knowing about tracing.
//!
//! Nothing here is on the failure path: with no DSN sentry never samples or sends, and every call
//! is infallible (contract T4).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use lanrurugi_core::jobs::{
    PHASE_INGEST, PHASE_NEGOTIATE, PHASE_PUBLISH, PHASE_QUEUED, PHASE_TRANSFER, PHASE_WAITING,
};

/// The child span operation for a phase, from `contracts/trace-spans.md`.
fn span_op(phase: &str) -> &'static str {
    match phase {
        PHASE_QUEUED => "queue.wait",
        PHASE_NEGOTIATE => "plugin.exec",
        PHASE_WAITING => "http.wait",
        PHASE_TRANSFER => "file.transfer",
        PHASE_INGEST => "file.ingest",
        PHASE_PUBLISH => "plugin.metadata",
        _ => "queue.phase",
    }
}

fn active() -> &'static Mutex<HashMap<String, Arc<DownloadTrace>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<String, Arc<DownloadTrace>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lookup(job_id: &str) -> Option<Arc<DownloadTrace>> {
    active().lock().ok()?.get(job_id).cloned()
}

/// A download task's trace, advanced by job id through [`phase`], [`fail`] and [`finish`].
struct DownloadTrace {
    transaction: sentry::Transaction,
    open: Mutex<Option<sentry::Span>>,
}

impl DownloadTrace {
    fn begin(queue_item_id: Option<&str>, plugin: &str, url: &str) -> Self {
        let context = match crate::trace_context::current() {
            Some(parent) => sentry::TransactionContext::new_with_details(
                "download_url",
                "queue.task",
                parent.trace_id,
                None,
                Some(parent.span_id),
            ),
            None => sentry::TransactionContext::new("download_url", "queue.task"),
        };
        let transaction = sentry::start_transaction(context);
        if let Some(queue_item_id) = queue_item_id {
            transaction.set_data("download.queue_item_id", queue_item_id.to_string().into());
        }
        transaction.set_data("download.plugin", plugin.to_string().into());
        // The query string is where these URLs carry tokens; it is not what a trace should keep.
        let without_query = url.split('?').next().unwrap_or(url);
        transaction.set_data("download.url", without_query.to_string().into());
        Self {
            transaction,
            open: Mutex::new(None),
        }
    }

    /// Ends the open child span and starts one for `phase`. An unknown name still gets a span, so
    /// a phase added later is visible rather than silently missing.
    fn phase(&self, phase: &str) {
        self.close_open();
        let span = self.transaction.start_child(span_op(phase), phase);
        if let Ok(mut open) = self.open.lock() {
            *open = Some(span);
        }
    }

    /// Marks the transaction failed; the open span is closed by `finish`, which every exit reaches.
    fn fail(&self, reason: &str) {
        self.transaction
            .set_data("error.class", reason.to_string().into());
        self.transaction
            .set_status(sentry::protocol::SpanStatus::InternalError);
    }

    fn finish(&self) {
        self.close_open();
        self.transaction.clone().finish();
    }

    fn close_open(&self) {
        let span = self.open.lock().ok().and_then(|mut open| open.take());
        if let Some(span) = span {
            span.finish();
        }
    }
}

/// Starts the task's trace. Hold the returned guard in the task body: every exit — success,
/// failure, cancellation — reports exactly once, because dropping it finishes the transaction.
pub fn begin(
    job_id: &str,
    queue_item_id: Option<&str>,
    plugin: &str,
    url: &str,
) -> DownloadTraceGuard {
    let trace = Arc::new(DownloadTrace::begin(queue_item_id, plugin, url));
    if let Ok(mut active) = active().lock() {
        active.insert(job_id.to_string(), trace);
    }
    DownloadTraceGuard {
        job_id: job_id.to_string(),
    }
}

/// Advances the task's trace to `phase`. A no-op outside a traced task.
pub fn phase(job_id: &str, name: &str) {
    if let Some(trace) = lookup(job_id) {
        trace.phase(name);
    }
}

/// Marks the task's transaction failed. A no-op outside a traced task.
pub fn fail(job_id: &str, reason: &str) {
    if let Some(trace) = lookup(job_id) {
        trace.fail(reason);
    }
}

/// Finishes and forgets the task's trace. Called by the guard; idempotent.
pub fn finish(job_id: &str) {
    let trace = active().lock().ok().and_then(|mut a| a.remove(job_id));
    if let Some(trace) = trace {
        trace.finish();
    }
}

/// Finishes the trace when the task body ends, however it ends.
pub struct DownloadTraceGuard {
    job_id: String,
}

impl Drop for DownloadTraceGuard {
    fn drop(&mut self) {
        finish(&self.job_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_in_the_vocabulary_has_its_own_operation() {
        // The mapping is the contract's table; a renamed const would leave a silent `queue.phase`.
        assert_eq!(span_op(PHASE_QUEUED), "queue.wait");
        assert_eq!(span_op(PHASE_NEGOTIATE), "plugin.exec");
        assert_eq!(span_op(PHASE_WAITING), "http.wait");
        assert_eq!(span_op(PHASE_TRANSFER), "file.transfer");
        assert_eq!(span_op(PHASE_INGEST), "file.ingest");
        assert_eq!(span_op(PHASE_PUBLISH), "plugin.metadata");
        assert_eq!(span_op("something-new"), "queue.phase");
    }

    #[test]
    fn a_trace_without_a_dsn_records_phases_without_panicking() {
        // No client is installed in a unit test, so this is the disabled path: cheap no-ops, never
        // a panic or an error the task could inherit.
        let guard = begin(
            "job-1",
            Some("queue-1"),
            "download/ehentai",
            "https://x/y?t=1",
        );
        phase("job-1", PHASE_QUEUED);
        phase("job-1", PHASE_TRANSFER);
        fail("job-1", "io");
        drop(guard);
        // The guard removed the entry, so later marks on the same id are no-ops.
        assert!(lookup("job-1").is_none());
        phase("job-1", PHASE_INGEST);
    }
}
