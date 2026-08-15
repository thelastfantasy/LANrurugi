//! Rust-side download-plugin pipeline: real streaming HTTP downloads (a download plugin no
//! longer performs its own byte-level fetch — see `contracts/plugin-download-protocol.md` under
//! `specs/005-download-plugin-progress/`), per-domain concurrency limiting, and per-domain rate
//! limiting. Progress is reported into `lanrurugi_core::jobs::JobRegistry` as the transfer
//! proceeds, which the existing `GET /api/jobs` polling endpoint already surfaces to the frontend.

pub mod bundle;
pub mod domain_rules;
pub mod ingest;
pub mod live_rate;
pub mod rate_limit;
pub mod settings;
pub mod stream;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, Semaphore};

use domain_rules::{resolve, resolved_key, DomainRule};
use rate_limit::RateLimiterMap;

/// A semaphore together with the capacity it was actually constructed with — `Semaphore` itself
/// never exposes this once permits start being acquired/released (`available_permits()` reflects
/// current availability, not original capacity, and dips below it for perfectly ordinary reasons:
/// an in-flight download holding a permit), so it must be tracked alongside the `Arc` rather than
/// inferred from it.
struct SizedSemaphore {
    semaphore: Arc<Semaphore>,
    capacity: usize,
}

/// Holds the per-domain concurrency (`Semaphore`) and rate-limit (`governor::RateLimiter`) state
/// shared across every download this process performs. One instance lives in `AppState` (per
/// plugin, keyed by namespace — see `plugins.rs`'s wiring), since different plugins' domain rules
/// are independent (spec Assumptions: "settings changes apply per-plugin, not globally").
#[derive(Default)]
pub struct DownloadManager {
    /// Keyed by [`domain_rules::resolved_key`] (the *matching rule's own pattern*, not the raw
    /// hostname) so two different subdomains matching the same wildcard rule correctly share one
    /// limit (spec US2 Acceptance Scenario 2) — `tokio::sync::Mutex<HashMap<...>>`, not
    /// `dashmap`, to avoid a new dependency for what's a low-contention map (one lock per download
    /// *start*, not per byte).
    semaphores: Mutex<HashMap<String, SizedSemaphore>>,
    rate_limiters: RateLimiterMap,
}

/// A concurrency permit plus the initial resolved rate limit (kept for caller convenience — the
/// actual per-chunk rate limit is re-resolved on every chunk via [`stream::RateResolver`] rather
/// than using this snapshot). Dropping this releases the concurrency permit.
pub struct DownloadPermit {
    _permit: Option<tokio::sync::OwnedSemaphorePermit>,
    pub max_bytes_per_sec: Option<u64>,
    /// The matching domain-rule pattern (exact hostname / wildcard / `"*"`) that this download's
    /// limits were resolved from — `domain_rules::resolved_key`'s value, surfaced through the
    /// permit so the throttle key and the pattern exposed via `JobStatus` (issue #2) are guaranteed
    /// to agree without a second independent traversal. See `acquire`'s resolution-order caveat for
    /// the known approximation when one rule declares concurrency and another the rate limit.
    pub matched_pattern: String,
}

impl DownloadManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Resolves `hostname` against `rules`, acquires a concurrency permit for however many
    /// simultaneous downloads that resolved domain currently allows (waiting if the limit is
    /// already reached — spec US2 Acceptance Scenario 1: "the rest wait their turn rather than
    /// failing outright"), and returns a [`DownloadPermit`] holding the permit. The rate limit
    /// (`max_bytes_per_sec`) is captured at this moment only as a convenience snapshot — the real
    /// per-chunk throttle re-resolves it live via [`stream::RateResolver`], so clearing or
    /// changing a rate cap takes effect mid-transfer, not only for future downloads.
    ///
    /// **FR-006 correctness**: `Semaphore`'s capacity is fixed at construction. If `rules`'
    /// resolved `max_concurrent` for this domain differs from the *originally declared* capacity
    /// of the existing semaphore for this resolved key (a user changed the setting since the last
    /// download to this domain), the stale semaphore is replaced with a freshly constructed one at
    /// the new capacity — never resized in place (`tokio::sync::Semaphore` has no such API), and
    /// never based on the misleading `available_permits()` (which legitimately dips below original
    /// capacity while downloads are in flight and must not be mistaken for a capacity change —
    /// see [`SizedSemaphore`]). Replacing the map entry doesn't affect a permit some other
    /// in-flight download already acquired from the old `Arc` (each holds its own clone, which
    /// stays valid independently of the map), so a settings change only ever governs downloads
    /// started after it, per FR-016.
    ///
    /// **Cancel-safe**: races the semaphore acquisition against `cancel` (`tokio::select!`) rather
    /// than a bare `.await`, so a user pressing Stop while this call is blocked waiting for a busy
    /// domain's permit actually takes effect immediately — previously `sem.acquire_owned().await`
    /// ignored `cancel` entirely, so `stop_one` recorded a successful cancellation
    /// (`CancellationToken::cancel()` itself can't fail) that the caller never actually observed
    /// until a permit eventually freed up and the rest of `download_one` got a chance to check
    /// `cancel.is_cancelled()` on its own. Returns `Err(DownloadError::Cancelled)` — the same
    /// variant every other cancellation point in this pipeline already produces — rather than a
    /// new error kind, so callers don't need a second cancellation check.
    pub async fn acquire(
        &self,
        hostname: &str,
        rules: &[DomainRule],
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<DownloadPermit, crate::download_manager::stream::DownloadError> {
        let resolved = resolve(rules, hostname);
        let key = resolved_key(rules, hostname);

        let permit = if let Some(max_concurrent) = resolved.max_concurrent {
            let max_concurrent = max_concurrent.max(1) as usize;
            let sem = {
                let mut semaphores = self.semaphores.lock().await;
                let entry = semaphores
                    .entry(key.clone())
                    .or_insert_with(|| SizedSemaphore {
                        semaphore: Arc::new(Semaphore::new(max_concurrent)),
                        capacity: max_concurrent,
                    });
                if entry.capacity != max_concurrent {
                    *entry = SizedSemaphore {
                        semaphore: Arc::new(Semaphore::new(max_concurrent)),
                        capacity: max_concurrent,
                    };
                }
                entry.semaphore.clone()
            };
            let acquired = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    return Err(crate::download_manager::stream::DownloadError::Cancelled);
                }
                permit = sem.acquire_owned() => permit.expect("semaphore is never closed"),
            };
            Some(acquired)
        } else {
            None
        };

        Ok(DownloadPermit {
            _permit: permit,
            max_bytes_per_sec: resolved.max_bytes_per_sec,
            matched_pattern: key,
        })
    }

    pub fn rate_limiters(&self) -> &RateLimiterMap {
        &self.rate_limiters
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain_rules::DomainRule;

    fn rule(pattern: &str, max_concurrent: u32) -> DomainRule {
        DomainRule {
            pattern: Some(pattern.to_string()),
            max_concurrent: Some(max_concurrent),
            max_bytes_per_sec: None,
            description: None,
        }
    }

    fn no_cancel() -> tokio_util::sync::CancellationToken {
        tokio_util::sync::CancellationToken::new()
    }

    #[tokio::test]
    async fn no_matching_rule_grants_an_unmanaged_permit_immediately() {
        let mgr = DownloadManager::new();
        let permit = mgr
            .acquire("unrelated.com", &[], &no_cancel())
            .await
            .unwrap();
        assert!(permit._permit.is_none());
    }

    #[tokio::test]
    async fn concurrency_limit_gates_simultaneous_acquisition() {
        let mgr = Arc::new(DownloadManager::new());
        let rules = vec![rule("example.com", 1)];

        // First permit acquires immediately.
        let first = mgr
            .acquire("example.com", &rules, &no_cancel())
            .await
            .unwrap();

        // A second acquire against the same (capacity-1) domain must not complete until the
        // first is dropped — proven by racing it against a short timeout.
        let mgr2 = mgr.clone();
        let rules2 = rules.clone();
        let second =
            tokio::spawn(async move { mgr2.acquire("example.com", &rules2, &no_cancel()).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !second.is_finished(),
            "second acquire must still be waiting while the first permit is held"
        );

        drop(first);
        let second = tokio::time::timeout(std::time::Duration::from_secs(1), second)
            .await
            .expect("second acquire must complete once the first permit is released")
            .expect("task must not panic")
            .unwrap();
        drop(second);
    }

    #[tokio::test]
    async fn capacity_change_takes_effect_for_subsequent_acquires_without_disturbing_in_flight_ones(
    ) {
        let mgr = DownloadManager::new();
        let narrow = vec![rule("example.com", 1)];
        let wide = vec![rule("example.com", 5)];

        // Acquire under the narrow (capacity-1) rule and hold it — simulates an in-flight
        // download that started before a settings change.
        let held = mgr
            .acquire("example.com", &narrow, &no_cancel())
            .await
            .unwrap();

        // A user now changes the setting to allow 5 concurrent downloads (FR-006). Multiple new
        // acquires against the *new* rule set must all succeed without waiting for `held` to be
        // dropped — proving the capacity change took effect for new downloads (FR-006) without
        // retroactively disturbing the one already in flight (FR-016).
        let mut new_permits = Vec::new();
        for _ in 0..4 {
            let permit = tokio::time::timeout(
                std::time::Duration::from_millis(200),
                mgr.acquire("example.com", &wide, &no_cancel()),
            )
            .await
            .expect("new capacity must admit additional concurrent downloads immediately")
            .unwrap();
            new_permits.push(permit);
        }

        drop(held);
        drop(new_permits);
    }

    #[tokio::test]
    async fn acquire_surfaces_matched_pattern_in_permit() {
        let mgr = DownloadManager::new();
        let rules = vec![DomainRule {
            pattern: Some("*.example.com".to_string()),
            max_concurrent: Some(2),
            max_bytes_per_sec: Some(1_048_576),
            description: None,
        }];
        let permit = mgr
            .acquire("cdn.example.com", &rules, &no_cancel())
            .await
            .unwrap();
        assert_eq!(permit.matched_pattern, "*.example.com");
        assert_eq!(permit.max_bytes_per_sec, Some(1_048_576));
    }

    #[tokio::test]
    async fn acquire_returns_asterisk_when_unmanaged() {
        let mgr = DownloadManager::new();
        let permit = mgr
            .acquire("unrelated.com", &[], &no_cancel())
            .await
            .unwrap();
        assert_eq!(permit.matched_pattern, "*");
        assert_eq!(permit.max_bytes_per_sec, None);
    }

    #[tokio::test]
    async fn acquire_is_cancel_safe_while_waiting_for_a_busy_semaphore() {
        let mgr = Arc::new(DownloadManager::new());
        let rules = vec![rule("example.com", 1)];

        // Hold the only permit so a second acquire has to actually wait.
        let held = mgr
            .acquire("example.com", &rules, &no_cancel())
            .await
            .unwrap();

        let cancel = tokio_util::sync::CancellationToken::new();
        let mgr2 = mgr.clone();
        let rules2 = rules.clone();
        let cancel2 = cancel.clone();
        let waiting =
            tokio::spawn(async move { mgr2.acquire("example.com", &rules2, &cancel2).await });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
            .await
            .expect("a cancelled acquire must return promptly, not wait for the permit")
            .expect("task must not panic");
        assert!(matches!(
            result,
            Err(crate::download_manager::stream::DownloadError::Cancelled)
        ));

        drop(held);
    }
}
