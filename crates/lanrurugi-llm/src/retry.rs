//! The deadline, retry and cancellation policy every model call runs under
//! (`specs/014-ai-harness-hardening/contracts/llm-call-contract.md`). One deadline covers all
//! attempts; at most one retry, only for a retryable class; cancellation drops the in-flight
//! request. Each call ends by folding its report into the usage ledger.

use std::future::Future;
use std::time::{Duration, Instant};

use deadpool_redis::Pool;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::error::LlmError;
use crate::prefix::Conversation;
use crate::usage::{self, FeatureId, ModelCallReport, TokenUsage};

/// Default per-call ceiling: above the longest legitimate single round observed (a long-form
/// generation round at 80+ s) and below the plugin wizard's 300 s overall bound.
pub const DEFAULT_DEADLINE: Duration = Duration::from_secs(180);

/// Pause before retrying a transient failure that carried no `Retry-After` hint.
const TRANSIENT_BACKOFF: Duration = Duration::from_secs(1);

/// Who is calling and under what bounds.
#[derive(Debug, Clone)]
pub struct CallSpec {
    pub feature: FeatureId,
    pub deadline: Duration,
    pub cancel: Option<CancellationToken>,
    /// Set on every round of a multi-round conversation, so the seam checks each round extends
    /// the previous one (`crate::prefix`).
    pub conversation: Option<Conversation>,
}

impl CallSpec {
    pub fn new(feature: FeatureId) -> Self {
        Self {
            feature,
            deadline: DEFAULT_DEADLINE,
            cancel: None,
            conversation: None,
        }
    }

    pub fn in_conversation(mut self, conversation: Conversation) -> Self {
        self.conversation = Some(conversation);
        self
    }

    pub fn with_deadline(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    pub fn with_cancel(mut self, cancel: CancellationToken) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

/// Whether to try again after `err`, and how long to wait first. `None` means stop.
pub(crate) fn retry_delay(err: &LlmError, attempt: u32, remaining: Duration) -> Option<Duration> {
    if attempt >= 2 || !err.is_retryable() {
        return None;
    }
    let wait = if err.is_truncation() {
        Duration::ZERO
    } else {
        err.retry_after().unwrap_or(TRANSIENT_BACKOFF)
    };
    // A hint that does not fit inside the remaining deadline fails now rather than sleeping
    // past the bound.
    (wait < remaining).then_some(wait)
}

/// Runs one model call: `attempt` is invoked once, and once more for a retryable failure, with the
/// previous attempt's error. The whole thing is bounded by the spec's deadline and cancellation.
pub(crate) async fn execute<T, F, Fut>(
    redis_config: &Pool,
    spec: &CallSpec,
    mut attempt: F,
) -> Result<T, LlmError>
where
    F: FnMut(Option<LlmError>) -> Fut,
    Fut: Future<Output = (Result<T, LlmError>, TokenUsage)>,
{
    let started = Instant::now();
    // One observer per call, at the level where attempts/elapsed/usage all exist.
    let observed = crate::telemetry::tracer().map(|tracer| {
        (
            tracer,
            tracer.begin(&crate::telemetry::CallStarted {
                feature: spec.feature.as_str(),
                model: crate::DEEPSEEK_MODEL,
            }),
        )
    });
    let mut attempts = 0u32;
    let mut usage_total = TokenUsage::default();

    let run = async {
        let mut previous: Option<LlmError> = None;
        loop {
            attempts += 1;
            let (result, usage) = attempt(previous.take()).await;
            usage_total = usage_total + usage;
            match result {
                Ok(value) => return Ok(value),
                Err(err) => {
                    let remaining = spec.deadline.saturating_sub(started.elapsed());
                    let Some(wait) = retry_delay(&err, attempts, remaining) else {
                        return Err(err);
                    };
                    warn!(
                        feature = spec.feature.as_str(),
                        class = err.class().as_str(),
                        "retrying model call once"
                    );
                    tokio::time::sleep(wait).await;
                    previous = Some(err);
                }
            }
        }
    };

    let bounded = async {
        match tokio::time::timeout(spec.deadline, run).await {
            Ok(result) => result,
            Err(_) => Err(LlmError::timeout()),
        }
    };

    let result = match &spec.cancel {
        Some(cancel) => tokio::select! {
            result = bounded => result,
            () = cancel.cancelled() => Err(LlmError::cancelled()),
        },
        None => bounded.await,
    };

    let report = ModelCallReport {
        feature: spec.feature,
        outcome: result.as_ref().err().map(LlmError::class),
        attempts: attempts.max(1),
        elapsed: started.elapsed(),
        usage: usage_total,
    };
    usage::record(redis_config, &report).await;
    if let Some((tracer, state)) = observed {
        tracer.end(
            state,
            &crate::telemetry::CallFinished {
                feature: spec.feature.as_str(),
                model: crate::DEEPSEEK_MODEL,
                elapsed: report.elapsed,
                attempts: report.attempts,
                input_tokens: report.usage.prompt.unwrap_or(0),
                output_tokens: report.usage.completion.unwrap_or(0),
                error: report.outcome.map(|class| format!("{class:?}")),
            },
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_balance_and_timeout_are_never_retried() {
        let long = Duration::from_secs(60);
        assert_eq!(
            retry_delay(&LlmError::from_status(401, "x", None), 1, long),
            None
        );
        assert_eq!(
            retry_delay(&LlmError::from_status(402, "x", None), 1, long),
            None
        );
        assert_eq!(retry_delay(&LlmError::timeout(), 1, long), None);
        assert_eq!(retry_delay(&LlmError::cancelled(), 1, long), None);
        assert_eq!(retry_delay(&LlmError::unparseable("x"), 1, long), None);
    }

    #[test]
    fn transient_and_truncation_retry_exactly_once() {
        let long = Duration::from_secs(60);
        let transient = LlmError::from_status(503, "x", None);
        assert_eq!(retry_delay(&transient, 1, long), Some(TRANSIENT_BACKOFF));
        assert_eq!(retry_delay(&transient, 2, long), None);
        assert_eq!(
            retry_delay(&LlmError::truncated(), 1, long),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn a_retry_after_hint_beyond_the_deadline_fails_immediately() {
        let limited = LlmError::from_status(429, "x", Some(Duration::from_secs(30)));
        assert_eq!(retry_delay(&limited, 1, Duration::from_secs(10)), None);
        assert_eq!(
            retry_delay(&limited, 1, Duration::from_secs(60)),
            Some(Duration::from_secs(30))
        );
    }
}
