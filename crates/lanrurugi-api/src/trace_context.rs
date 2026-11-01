//! Carries the current HTTP request's trace identity into code that reports its own spans.
//!
//! `sentry_tower`'s [`SentryHttpLayer`] creates one transaction per request and installs it in the
//! hub's *scope* — which is thread-local, not task-local. A handler may resume on a different
//! worker thread than the one that first polled the request, so a span started deep inside a
//! handler has no reliable way to find that transaction, and `sentry::start_span` would attach to
//! whatever the current thread happens to have (usually nothing). That is why the AI-agent
//! monitoring transaction for one model call landed in a *different* trace from the request that
//! paid for it: the recommendation endpoint's 28 s showed up as an empty-span HTTP transaction plus
//! an unrelated `gen_ai.chat` transaction.
//!
//! The fix is to read the identity once, synchronously, on the same poll that installed it — this
//! middleware runs directly inside `SentryHttpLayer`, so `configure_scope` still sees the request's
//! transaction at that moment — and stash it in a tokio **task**-local, which follows the future
//! across thread switches for as long as it lives. [`current`] then works from anywhere in that
//! task (including deep inside `lanrurugi_llm`'s call seam), and background tasks — which have no
//! request — simply get `None` and keep reporting their own top-level transaction.
//!
//! Scope: identity only (`trace_id` + the request span's id), not the span itself. Nothing here
//! mutates the request's transaction, so a reporting failure can't affect the response.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use sentry::protocol::{SpanId, TraceId};

tokio::task_local! {
    static REQUEST_TRACE: Option<TraceContext>;
}

/// The identity of the in-flight request's transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraceContext {
    pub trace_id: TraceId,
    /// The request transaction's own span id — the parent any child operation should point at.
    pub span_id: SpanId,
}

/// Middleware: capture the request's trace identity for the rest of the request's task.
///
/// Must be layered *inside* `sentry_tower::SentryHttpLayer` (added before it in axum's builder, so
/// it ends up closer to the handler); placed outside, the transaction does not exist yet and this
/// finds nothing to capture.
pub async fn capture_request_trace(request: Request, next: Next) -> Response {
    let context = sentry::configure_scope(|scope| {
        scope.get_span().map(|span| {
            let trace = span.get_trace_context();
            TraceContext {
                trace_id: trace.trace_id,
                span_id: trace.span_id,
            }
        })
    });
    match context {
        Some(context) => REQUEST_TRACE.scope(Some(context), next.run(request)).await,
        None => next.run(request).await,
    }
}

/// The current request's trace identity, when this code is running inside one.
pub fn current() -> Option<TraceContext> {
    REQUEST_TRACE.try_with(|context| *context).ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_a_request_there_is_no_context_rather_than_a_panic() {
        assert!(current().is_none());
    }

    #[tokio::test]
    async fn a_scoped_task_sees_the_context_and_a_background_task_does_not() {
        let context = TraceContext {
            trace_id: TraceId::default(),
            span_id: SpanId::default(),
        };
        let background = REQUEST_TRACE
            .scope(
                Some(context),
                tokio::spawn(async {
                    // A spawned task does not inherit task-locals: it is a separate task, so it
                    // reports its own top-level transaction instead of a bogus parent.
                    current().is_none()
                }),
            )
            .await
            .unwrap();
        assert!(background);

        let seen = REQUEST_TRACE
            .scope(Some(context), async { current() })
            .await;
        assert_eq!(seen, Some(context));
    }

    #[tokio::test]
    async fn a_task_outside_any_scope_sees_nothing() {
        assert!(current().is_none());
    }
}
