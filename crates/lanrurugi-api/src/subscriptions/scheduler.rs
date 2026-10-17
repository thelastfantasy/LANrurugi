//! Deciding *when* a subscription checks, and guaranteeing it never checks itself twice at once
//! (`specs/010-subscription-orchestrator/data-model.md`).
//!
//! One scheduler owns every start. That is what makes FR-012's non-overlap a local question —
//! "is this subscription already running?" is answered by looking at one in-flight set, with no
//! cross-task coordination. Per-subscription tasks would have multiplied idle tasks for no gain at
//! personal-library scale while making that question genuinely hard.

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

/// Subscriptions currently mid-check. Cloned cheaply; one instance is shared by the scheduler loop
/// and any manual "check now" request, so the two can never start the same subscription twice.
#[derive(Clone, Default)]
pub struct InFlight {
    ids: Arc<Mutex<HashSet<String>>>,
}

impl InFlight {
    pub fn new() -> Self {
        Self::default()
    }

    /// Claims `id` for a check. Returns `None` when it is already claimed, which the caller must
    /// treat as "skip this one", not as an error — a check running longer than its own interval is
    /// normal on a slow source, not a fault.
    pub async fn claim(&self, id: &str) -> Option<InFlightGuard> {
        let mut ids = self.ids.lock().await;
        if !ids.insert(id.to_string()) {
            return None;
        }
        Some(InFlightGuard {
            ids: self.ids.clone(),
            id: id.to_string(),
        })
    }

    pub async fn is_running(&self, id: &str) -> bool {
        self.ids.lock().await.contains(id)
    }
}

/// Releases its claim on drop, so a panicking or early-returning check cannot leave a subscription
/// permanently wedged as "already running".
pub struct InFlightGuard {
    ids: Arc<Mutex<HashSet<String>>>,
    id: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        let ids = self.ids.clone();
        let id = std::mem::take(&mut self.id);
        // `Drop` cannot await, so the release is spawned. Worst case is a brief window where a
        // finished subscription still looks busy — which only delays its next check by one tick,
        // whereas never releasing would stop it forever.
        tokio::spawn(async move {
            ids.lock().await.remove(&id);
        });
    }
}

/// How often the scheduler wakes to look for due subscriptions. Independent of any subscription's
/// own interval: this is the resolution at which "due" is noticed, not how often anything runs.
pub const TICK_SECS: u64 = 60;

/// How many reservation entries to keep before pruning the oldest (FR-018).
pub const RESERVATION_LIMIT: usize = 500;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn claiming_twice_is_refused_until_the_first_claim_is_released() {
        let flight = InFlight::new();
        let first = flight.claim("sub-1").await;
        assert!(first.is_some(), "first claim should succeed");
        assert!(
            flight.claim("sub-1").await.is_none(),
            "a second concurrent claim must be refused — this is FR-012's non-overlap guarantee"
        );
        drop(first);
        // The guard releases asynchronously, so give that spawned task a chance to run.
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(
            flight.claim("sub-1").await.is_some(),
            "once released, the subscription must be claimable again"
        );
    }

    #[tokio::test]
    async fn different_subscriptions_do_not_block_each_other() {
        let flight = InFlight::new();
        let a = flight.claim("sub-a").await;
        let b = flight.claim("sub-b").await;
        assert!(a.is_some() && b.is_some());
    }

    #[tokio::test]
    async fn is_running_reflects_the_claim() {
        let flight = InFlight::new();
        assert!(!flight.is_running("sub-1").await);
        let _g = flight.claim("sub-1").await.unwrap();
        assert!(flight.is_running("sub-1").await);
    }
}
