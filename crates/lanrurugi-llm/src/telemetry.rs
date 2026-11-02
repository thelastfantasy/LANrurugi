//! A hook for observing model calls (specs/018's AI-agent monitoring). The crate itself knows
//! nothing about the tracker: the API installs an implementation once, and only when an LLM key is
//! configured — a deployment that never calls a model reports nothing.

use std::any::Any;
use std::sync::OnceLock;
use std::time::Duration;

/// One call as it starts.
#[derive(Debug, Clone)]
pub struct CallStarted {
    /// Which feature is calling (`FeatureId`'s own name).
    pub feature: &'static str,
    /// The model the request asks for.
    pub model: &'static str,
}

/// One call as it ends. `elapsed` and `attempts` cover every retry, which is what a slow call means
/// in practice.
#[derive(Debug, Clone)]
pub struct CallFinished {
    pub feature: &'static str,
    pub model: &'static str,
    pub elapsed: Duration,
    pub attempts: u32,
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// `None` for success; the error class otherwise — never the prompt or the reply.
    pub error: Option<String>,
}

/// Observes calls. `begin` may return state (a span/transaction) that `end` gets back.
pub trait Tracer: Send + Sync {
    fn begin(&self, call: &CallStarted) -> Option<Box<dyn Any + Send>>;
    fn end(&self, state: Option<Box<dyn Any + Send>>, call: &CallFinished);
}

static TRACER: OnceLock<&'static dyn Tracer> = OnceLock::new();

/// The installed tracer, if any. When absent the call path skips both hooks entirely.
pub fn tracer() -> Option<&'static dyn Tracer> {
    TRACER.get().copied()
}

pub fn install(tracer: &'static dyn Tracer) {
    let _ = TRACER.set(tracer);
}
