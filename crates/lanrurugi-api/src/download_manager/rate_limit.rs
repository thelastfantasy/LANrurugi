//! Per-domain rate limiting (spec FR-009/FR-010), built on `governor`'s token-bucket
//! `RateLimiter`, using N = bytes-per-chunk consumption (`until_n_ready`) rather than the default
//! one-token-per-call usage — see `specs/005-download-plugin-progress/research.md` §2.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;

use governor::{Quota, RateLimiter};
use tokio::sync::Mutex;

type ByteRateLimiter = RateLimiter<
    governor::state::NotKeyed,
    governor::state::InMemoryState,
    governor::clock::QuantaClock,
>;

/// The rate limiter's actual burst capacity, independent of the configured rate — `governor`'s
/// `Quota::per_second(rate)` defaults the burst size to `rate` itself (a full second's worth of
/// tokens available instantaneously to an idle limiter), which is *technically* spec-compliant
/// ("no more than `rate` bytes/sec, averaged") but produces a visibly wrong-looking one-time spike
/// at the very start of every download: measured directly against the raw `governor` crate
/// (issue #41 follow-up investigation), a fresh 1,000,000 bytes/sec limiter let ~983,040 bytes
/// through with sub-microsecond waits before the 16th call finally started blocking for the
/// expected ~65.5ms per 65,536-byte chunk — i.e. nearly a full second's allowance duped out in a
/// handful of iterations, right at the moment a user is watching the speed reading for the first
/// time. Capping the burst independently via `Quota::allow_burst` bounds that one-time spike to a
/// small, constant number of chunks' worth regardless of the configured rate, while leaving the
/// steady-state rate (and thus long-run averaged throughput) exactly as configured.
const MAX_BURST_BYTES: u64 = 262_144;

/// A rate limiter together with the bytes/sec cap it was actually constructed with — same
/// "governor exposes no way to read back a limiter's original quota" reasoning as
/// `download_manager::SizedSemaphore`'s own docs, so it's tracked alongside the limiter rather
/// than inferred from its runtime state.
struct SizedRateLimiter {
    limiter: Arc<ByteRateLimiter>,
    bytes_per_sec: u64,
}

/// Per-resolved-domain-key map of active rate limiters (keyed the same way as
/// `DownloadManager`'s own semaphore map — `domain_rules::resolved_key` — so a rule's
/// `max_bytes_per_sec` and `max_concurrent` are resolved together for the same domain grouping).
#[derive(Default)]
pub struct RateLimiterMap {
    limiters: Mutex<HashMap<String, SizedRateLimiter>>,
}

impl RateLimiterMap {
    /// Throttles the caller to `bytes_per_sec` (if `Some`) for `key`, waiting until enough tokens
    /// are available to "spend" `chunk_len` bytes just read from a download's byte stream. A
    /// `None` cap (spec FR-010: no rate limit configured) is a fast-path no-op — the caller
    /// proceeds at full, unrestricted speed exactly as it does today, with no `governor` call at
    /// all.
    ///
    /// **Correctness note**: `governor`'s `until_n_ready(n)` returns `Err(InsufficientCapacity)`
    /// — immediately, without waiting at all — whenever `n` exceeds the limiter's own burst
    /// capacity ([`MAX_BURST_BYTES`], not the configured rate — see that constant's own docs on
    /// why the two are deliberately decoupled), rather than queueing and draining it across
    /// multiple refill periods. A single `reqwest` chunk read can exceed that cap even at a
    /// generous rate limit — passing the whole `chunk_len` through in one `until_n_ready` call
    /// would silently fail to throttle whenever that happens. Fixed by splitting `chunk_len` into
    /// sub-chunks no larger than the burst cap and awaiting each in turn.
    ///
    /// Same capacity-change handling as `DownloadManager::acquire`: if `bytes_per_sec` for `key`
    /// differs from the tracked limiter's own originally-declared rate, the stale limiter is
    /// replaced with a freshly constructed one at the new rate (never mutated in place — `governor`
    /// has no API for that either), affecting only chunks read after the change (FR-016).
    pub async fn throttle(&self, key: &str, bytes_per_sec: Option<u64>, chunk_len: u64) {
        let Some(bytes_per_sec) = bytes_per_sec.filter(|&b| b > 0) else {
            return;
        };
        if chunk_len == 0 {
            return;
        }

        let limiter = {
            let mut limiters = self.limiters.lock().await;
            let entry = limiters
                .entry(key.to_string())
                .or_insert_with(|| SizedRateLimiter {
                    limiter: Arc::new(new_limiter(bytes_per_sec)),
                    bytes_per_sec,
                });
            if entry.bytes_per_sec != bytes_per_sec {
                *entry = SizedRateLimiter {
                    limiter: Arc::new(new_limiter(bytes_per_sec)),
                    bytes_per_sec,
                };
            }
            entry.limiter.clone()
        };

        let burst_cap = bytes_per_sec
            .min(MAX_BURST_BYTES)
            .min(u32::MAX as u64)
            .max(1);
        let mut remaining = chunk_len;
        while remaining > 0 {
            let this_slice = remaining.min(burst_cap);
            remaining -= this_slice;
            let n =
                NonZeroU32::new(this_slice as u32).unwrap_or_else(|| NonZeroU32::new(1).unwrap());
            // Within the burst cap by construction, so `InsufficientCapacity` cannot occur here —
            // any error at this point would mean the limiter's capacity and `burst_cap` above have
            // diverged, which would itself be a bug in this function, not a real runtime condition
            // to recover from.
            limiter
                .until_n_ready(n)
                .await
                .expect("chunk slice is always within the limiter's own burst capacity");
        }
    }

    /// Drops `key`'s entry, if any. Callers using a per-download-unique key (issue #41: each
    /// download gets its own token bucket rather than sharing one per domain-rule pattern, so a
    /// concurrent sibling can't "borrow" another download's unused headroom) must call this once
    /// their download finishes — otherwise this map grows by one entry per download for the life
    /// of the process, since entries are never otherwise evicted.
    pub async fn release(&self, key: &str) {
        self.limiters.lock().await.remove(key);
    }

    #[cfg(test)]
    pub async fn tracked_entry_count(&self) -> usize {
        self.limiters.lock().await.len()
    }
}

fn new_limiter(bytes_per_sec: u64) -> ByteRateLimiter {
    let rate = NonZeroU32::new(bytes_per_sec.min(u32::MAX as u64) as u32)
        .unwrap_or_else(|| NonZeroU32::new(1).unwrap());
    // Burst capped independently of the rate (see `MAX_BURST_BYTES`'s own docs) — without this, a
    // fresh/idle limiter's burst size defaults to a full second's worth of the configured rate,
    // letting nearly all of it through instantaneously the moment a download starts.
    let burst = NonZeroU32::new(bytes_per_sec.min(MAX_BURST_BYTES).min(u32::MAX as u64) as u32)
        .unwrap_or_else(|| NonZeroU32::new(1).unwrap());
    RateLimiter::direct(Quota::per_second(rate).allow_burst(burst))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn no_cap_is_a_fast_path_noop() {
        let map = RateLimiterMap::default();
        let start = Instant::now();
        map.throttle("example.com", None, 10_000_000).await;
        assert!(
            start.elapsed().as_millis() < 50,
            "no configured rate limit must not throttle at all"
        );
    }

    #[tokio::test]
    async fn zero_or_missing_chunk_is_a_noop() {
        let map = RateLimiterMap::default();
        let start = Instant::now();
        map.throttle("example.com", Some(1), 0).await;
        assert!(start.elapsed().as_millis() < 50);
    }

    /// Issue #41 follow-up: `governor`'s default burst size (a full second's worth of the
    /// configured rate) let a fresh/idle limiter release ~98% of a whole second's allowance
    /// instantaneously the moment a download started, before converging to the real configured
    /// rate — this reproduced even after the shared-pool fix (per-download unique keys), since it
    /// happens on a *single* limiter regardless of sharing. Proven here by driving one limiter
    /// continuously for 5 seconds and asserting the average throughput stays close to the
    /// configured rate rather than sitting ~20-30% over it the way the unbounded-burst version did.
    #[tokio::test]
    async fn sustained_throughput_stays_close_to_the_configured_rate() {
        let map = RateLimiterMap::default();
        let bytes_per_sec: u64 = 1_000_000;
        let chunk: u64 = 65536;
        let start = Instant::now();
        let mut sent: u64 = 0;
        while start.elapsed().as_secs() < 5 {
            map.throttle("example.com", Some(bytes_per_sec), chunk)
                .await;
            sent += chunk;
        }
        let avg_rate = sent as f64 / start.elapsed().as_secs_f64();
        assert!(
            avg_rate < bytes_per_sec as f64 * 1.15,
            "average throughput over 5s must stay close to the configured rate, got {avg_rate:.0} B/s against a {bytes_per_sec} B/s cap"
        );
    }

    /// The one-time startup burst must be bounded to [`MAX_BURST_BYTES`], not the full configured
    /// rate — proven by confirming a limiter starts blocking well before a full second's worth of
    /// the configured rate has been spent.
    #[tokio::test]
    async fn startup_burst_is_bounded_independent_of_the_configured_rate() {
        use std::num::NonZeroU32;
        let bytes_per_sec = 1_000_000;
        let limiter = new_limiter(bytes_per_sec);
        let n = NonZeroU32::new(65536).unwrap();
        let mut spent_before_blocking: u64 = 0;
        loop {
            let call_start = Instant::now();
            limiter.until_n_ready(n).await.unwrap();
            if call_start.elapsed().as_millis() > 10 {
                break;
            }
            spent_before_blocking += 65536;
        }
        assert!(
            spent_before_blocking < bytes_per_sec / 2,
            "startup burst must be well under a full second's allowance, got {spent_before_blocking} bytes"
        );
    }

    #[tokio::test]
    async fn throttles_when_cap_is_exceeded() {
        let map = RateLimiterMap::default();
        // 100 bytes/sec cap; ask for 300 bytes in one go — must take meaningfully longer than
        // an unthrottled call would (a fast-path no-op finishes in low single-digit milliseconds).
        let start = Instant::now();
        map.throttle("example.com", Some(100), 300).await;
        assert!(
            start.elapsed().as_millis() > 500,
            "a 300-byte request against a 100 bytes/sec cap must wait, got {:?}",
            start.elapsed()
        );
    }

    /// Issue #41: two concurrent downloads under the same domain-rule pattern must each get the
    /// full configured rate, not split a shared pool — proven by throttling the *same* byte count
    /// under two distinct keys back-to-back and confirming the second call isn't slowed down by
    /// the first having already spent its (previously shared) token bucket.
    #[tokio::test]
    async fn distinct_keys_each_get_their_own_independent_quota() {
        let map = RateLimiterMap::default();
        // Exhausts a 100 bytes/sec bucket for "download-a" (burst cap == rate, so 100 bytes is the
        // most a fresh bucket allows instantaneously).
        map.throttle("rule#download-a", Some(100), 100).await;

        // A second, distinct key under the same matched rule must still get its own full burst
        // capacity immediately — if this were the old shared-pool behavior (one bucket per rule
        // pattern), this call would have to wait for "download-a"'s spend to refill.
        let start = Instant::now();
        map.throttle("rule#download-b", Some(100), 100).await;
        assert!(
            start.elapsed().as_millis() < 50,
            "a distinct download key must not be throttled by another download's own spend, got {:?}",
            start.elapsed()
        );
    }

    #[tokio::test]
    async fn release_removes_a_keys_tracked_limiter() {
        let map = RateLimiterMap::default();
        map.throttle("example.com", Some(100), 10).await;
        assert_eq!(map.tracked_entry_count().await, 1);
        map.release("example.com").await;
        assert_eq!(map.tracked_entry_count().await, 0);
    }

    #[tokio::test]
    async fn release_of_an_unknown_key_is_a_noop() {
        let map = RateLimiterMap::default();
        map.release("never-throttled.example.com").await;
    }
}
