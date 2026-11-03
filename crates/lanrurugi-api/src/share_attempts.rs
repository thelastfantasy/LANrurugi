//! Failed share-code lookups are rate-limited so guessing a code stays impractical (FR-011,
//! SC-010, research §3). Shared by the HTTP `open` route and the P2P `Open` request.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const SOURCE_WINDOW: Duration = Duration::from_secs(60);
const SOURCE_MAX_FAILURES: usize = 10;
const BASE_LOCKOUT: Duration = Duration::from_secs(60);
const MAX_LOCKOUT_DOUBLINGS: u32 = 6;
const LOCKOUT_MEMORY: Duration = Duration::from_secs(60 * 60);

const INSTANCE_WINDOW: Duration = Duration::from_secs(60 * 60);
const INSTANCE_FREE_FAILURES: usize = 500;
/// Beyond the free budget, failures are answered one at a time, each held this long.
pub const SERIAL_PENALTY: Duration = Duration::from_secs(10);

/// Caps memory under a flood of distinct sources.
const MAX_TRACKED_SOURCES: usize = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Penalty {
    None,
    /// Hold the answer in the instance-wide queue for `SERIAL_PENALTY`.
    Serialized,
}

#[derive(Default)]
struct Source {
    failures: VecDeque<Instant>,
    blocked_until: Option<Instant>,
    lockouts: u32,
    last_lockout: Option<Instant>,
}

#[derive(Default)]
struct State {
    sources: HashMap<String, Source>,
    instance: VecDeque<Instant>,
}

pub struct ShareAttemptLimiter {
    state: Mutex<State>,
    serial: tokio::sync::Mutex<()>,
}

impl Default for ShareAttemptLimiter {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            serial: tokio::sync::Mutex::new(()),
        }
    }
}

/// The process-wide limiter.
pub fn shared() -> &'static ShareAttemptLimiter {
    static LIMITER: OnceLock<ShareAttemptLimiter> = OnceLock::new();
    LIMITER.get_or_init(ShareAttemptLimiter::default)
}

impl ShareAttemptLimiter {
    /// Whether `source` may try a lookup now.
    pub fn allowed(&self, source: &str, now: Instant) -> bool {
        let state = self.state.lock().expect("limiter lock");
        state
            .sources
            .get(source)
            .and_then(|s| s.blocked_until)
            .is_none_or(|until| now >= until)
    }

    /// Records a failed lookup and returns the penalty to apply before answering.
    pub fn record_failure(&self, source: &str, now: Instant) -> Penalty {
        let mut state = self.state.lock().expect("limiter lock");
        if state.sources.len() >= MAX_TRACKED_SOURCES && !state.sources.contains_key(source) {
            state
                .sources
                .retain(|_, s| s.blocked_until.is_some_and(|u| u > now));
        }
        let entry = state.sources.entry(source.to_string()).or_default();
        prune(&mut entry.failures, now, SOURCE_WINDOW);
        entry.failures.push_back(now);
        if entry.failures.len() >= SOURCE_MAX_FAILURES {
            if entry
                .last_lockout
                .is_some_and(|t| now.duration_since(t) > LOCKOUT_MEMORY)
            {
                entry.lockouts = 0;
            }
            let doublings = entry.lockouts.min(MAX_LOCKOUT_DOUBLINGS);
            entry.blocked_until = Some(now + BASE_LOCKOUT * 2u32.pow(doublings));
            entry.lockouts += 1;
            entry.last_lockout = Some(now);
            entry.failures.clear();
        }

        prune(&mut state.instance, now, INSTANCE_WINDOW);
        state.instance.push_back(now);
        if state.instance.len() > INSTANCE_FREE_FAILURES {
            Penalty::Serialized
        } else {
            Penalty::None
        }
    }

    /// Applies a penalty: serialized failures wait their turn, then hold it.
    pub async fn apply(&self, penalty: Penalty) {
        if penalty == Penalty::Serialized {
            let _turn = self.serial.lock().await;
            tokio::time::sleep(SERIAL_PENALTY).await;
        }
    }
}

fn prune(times: &mut VecDeque<Instant>, now: Instant, window: Duration) {
    while times
        .front()
        .is_some_and(|t| now.duration_since(*t) >= window)
    {
        times.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_locked_out_after_ten_failures_and_backs_off_exponentially() {
        let limiter = ShareAttemptLimiter::default();
        let t0 = Instant::now();
        for i in 0..9 {
            limiter.record_failure("a", t0 + Duration::from_secs(i));
            assert!(limiter.allowed("a", t0 + Duration::from_secs(i)));
        }
        limiter.record_failure("a", t0 + Duration::from_secs(9));
        let locked_at = t0 + Duration::from_secs(9);
        assert!(!limiter.allowed("a", locked_at));
        assert!(limiter.allowed("b", locked_at));
        assert!(limiter.allowed("a", locked_at + BASE_LOCKOUT));

        let again = locked_at + BASE_LOCKOUT;
        for i in 0..10 {
            limiter.record_failure("a", again + Duration::from_secs(i));
        }
        let second = again + Duration::from_secs(9);
        assert!(!limiter.allowed("a", second + BASE_LOCKOUT));
        assert!(limiter.allowed("a", second + BASE_LOCKOUT * 2));
    }

    #[test]
    fn failures_spread_beyond_the_window_never_lock_out() {
        let limiter = ShareAttemptLimiter::default();
        let t0 = Instant::now();
        for i in 0..30 {
            limiter.record_failure("a", t0 + Duration::from_secs(i * 7));
        }
        assert!(limiter.allowed("a", t0 + Duration::from_secs(30 * 7)));
    }

    #[test]
    fn the_instance_budget_switches_to_serialized_penalties() {
        let limiter = ShareAttemptLimiter::default();
        let t0 = Instant::now();
        for i in 0..INSTANCE_FREE_FAILURES {
            let source = format!("s{i}");
            assert_eq!(limiter.record_failure(&source, t0), Penalty::None);
        }
        assert_eq!(limiter.record_failure("next", t0), Penalty::Serialized);
        assert_eq!(
            limiter.record_failure("later", t0 + INSTANCE_WINDOW),
            Penalty::None
        );
    }
}
