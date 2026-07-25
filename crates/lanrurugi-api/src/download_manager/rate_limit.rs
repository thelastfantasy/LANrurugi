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
    /// capacity (`Quota::per_second(rate)`'s burst size equals `rate`), rather than queueing and
    /// draining it across multiple refill periods. A single `reqwest` chunk read is routinely
    /// larger than a configured slow rate limit (e.g. a 64KB TCP read against a 10KB/sec cap) —
    /// passing the whole `chunk_len` through in one `until_n_ready` call would silently fail to
    /// throttle whenever that happens. Fixed by splitting `chunk_len` into sub-chunks no larger
    /// than `bytes_per_sec` itself (guaranteed to be within the limiter's own burst capacity) and
    /// awaiting each in turn.
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

        let burst_cap = bytes_per_sec.min(u32::MAX as u64).max(1);
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
}

fn new_limiter(bytes_per_sec: u64) -> ByteRateLimiter {
    let bytes_per_sec = NonZeroU32::new(bytes_per_sec.min(u32::MAX as u64) as u32)
        .unwrap_or_else(|| NonZeroU32::new(1).unwrap());
    RateLimiter::direct(Quota::per_second(bytes_per_sec))
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
}
