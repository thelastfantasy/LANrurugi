//! Generic "collapse concurrent requests for the same key onto one worker, bound total
//! concurrency across all keys" primitive (Go's `singleflight` package is the canonical prior
//! art for this shape). Extracted from the cover-thumbnail on-demand regeneration use case
//! (`lanrurugi-api::archives::get_archive_thumbnail`) since the same shape applies anywhere a
//! resource load can be requested redundantly and concurrently by multiple callers — e.g. reader
//! page prefetch requesting several pages of the same archive at once, or several browser
//! tabs/cards referencing the same missing thumbnail.
//!
//! Two throttles, for two different problems:
//! - Same-key dedup: N concurrent callers asking for the *same* key only do the (potentially
//!   expensive, e.g. decompress-and-decode-an-image) work once; every caller — the one that
//!   actually ran `work`, and every other one that arrived while it was running — gets a clone of
//!   the same computed value, rather than each redundantly repeating the work or racing to write
//!   the same output path.
//! - A bounded [`tokio::sync::Semaphore`] caps how many *distinct* keys are being worked on at
//!   once, since same-key dedup alone doesn't stop a burst of requests for many *different* keys
//!   (e.g. a homepage requesting dozens of different covers, or a reader prefetching several
//!   different pages) from each spawning unbounded concurrent heavy work.
//!
//! Unlike Go's `singleflight`, the per-key memoized value is *not* kept around after every
//! caller in the overlapping group has received it — a later, non-overlapping call always runs
//! `work` fresh. This is deliberate: `work`'s real durable cache (if any) is the caller's own
//! concern (e.g. a file on disk); this primitive only collapses *concurrent* duplicate work, it
//! is not itself a cache.

use std::collections::HashMap;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;

use tokio::sync::{Mutex, OnceCell, Semaphore};

/// Coordinates concurrent callers requesting work keyed by `K`, producing a cheaply-cloneable
/// `V`. Construct once (typically held in shared app state) and share via `Arc`/`Clone`.
pub struct Singleflight<K, V> {
    inflight: Mutex<HashMap<K, Arc<OnceCell<V>>>>,
    semaphore: Arc<Semaphore>,
}

impl<K, V> Singleflight<K, V>
where
    K: Eq + Hash + Clone + Send,
    V: Clone + Send + Sync,
{
    /// `max_concurrency` bounds how many distinct keys are worked on at once — callers beyond
    /// that limit simply wait on the semaphore (a natural queue), not spawn unbounded work.
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            inflight: Mutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }

    /// Runs `work` for `key`, or waits for and returns a clone of the result of an already-running
    /// call for the same `key` (`work` itself only ever runs once per non-overlapping burst of
    /// callers).
    pub async fn run<Fut>(&self, key: K, work: impl FnOnce() -> Fut + Send) -> V
    where
        Fut: Future<Output = V> + Send,
    {
        let (cell, is_owner) = {
            let mut inflight = self.inflight.lock().await;
            match inflight.get(&key) {
                Some(existing) => (existing.clone(), false),
                None => {
                    let cell = Arc::new(OnceCell::new());
                    inflight.insert(key.clone(), cell.clone());
                    (cell, true)
                }
            }
        };

        // Only the owner (the caller that inserted the cell above) acquires a concurrency
        // permit and actually runs `work` — every other concurrent caller for this key just
        // awaits the same `OnceCell`, which resolves once the owner's `work` finishes.
        let value = cell
            .get_or_init(|| async {
                let _permit = self.semaphore.acquire().await;
                work().await
            })
            .await
            .clone();

        // Only the owner removes the entry — once the whole overlapping group has read the
        // value (this line runs after `get_or_init` resolves for *every* caller, owner and
        // joiners alike, so joiners have already cloned their copy above by the time the owner
        // gets here), a later, non-overlapping call for the same key starts fresh rather than
        // reusing a permanently-memoized value.
        if is_owner {
            self.inflight.lock().await.remove(&key);
        }

        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_same_key_calls_run_work_once() {
        // `tokio::spawn` (not a sequential-await loop) so these 10 callers genuinely race each
        // other — the whole point under test is that concurrent, not sequential, requests for the
        // same key collapse onto one worker.
        let sf: Arc<Singleflight<&str, u32>> = Arc::new(Singleflight::new(4));
        let call_count = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for _ in 0..10 {
            let sf = sf.clone();
            let call_count = call_count.clone();
            handles.push(tokio::spawn(async move {
                sf.run("same-key", || async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    42
                })
                .await
            }));
        }
        let mut results = vec![];
        for h in handles {
            results.push(h.await.unwrap());
        }

        assert_eq!(call_count.load(Ordering::SeqCst), 1);
        assert!(results.iter().all(|&r| r == 42));
    }

    #[tokio::test]
    async fn different_keys_run_independently() {
        let sf: Arc<Singleflight<u32, u32>> = Arc::new(Singleflight::new(4));
        let call_count = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for key in 0..5u32 {
            let sf = sf.clone();
            let call_count = call_count.clone();
            handles.push(tokio::spawn(async move {
                sf.run(key, || async move {
                    call_count.fetch_add(1, Ordering::SeqCst);
                    key
                })
                .await
            }));
        }
        let mut results = vec![];
        for h in handles {
            results.push(h.await.unwrap());
        }

        assert_eq!(call_count.load(Ordering::SeqCst), 5);
        results.sort();
        assert_eq!(results, vec![0, 1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn semaphore_bounds_concurrent_distinct_keys() {
        let sf: Arc<Singleflight<u32, u32>> = Arc::new(Singleflight::new(2));
        let concurrent = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let mut handles = vec![];
        for key in 0..8u32 {
            let sf = sf.clone();
            let concurrent = concurrent.clone();
            let max_seen = max_seen.clone();
            handles.push(tokio::spawn(async move {
                sf.run(key, || async move {
                    let now = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    concurrent.fetch_sub(1, Ordering::SeqCst);
                    key
                })
                .await
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        assert!(
            max_seen.load(Ordering::SeqCst) <= 2,
            "expected at most 2 concurrent distinct-key workers, saw {}",
            max_seen.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn a_later_non_overlapping_call_runs_work_again() {
        let sf: Arc<Singleflight<&str, u32>> = Arc::new(Singleflight::new(4));
        let call_count = Arc::new(AtomicUsize::new(0));

        sf.run("key", || async {
            call_count.fetch_add(1, Ordering::SeqCst);
            1
        })
        .await;
        sf.run("key", || async {
            call_count.fetch_add(1, Ordering::SeqCst);
            2
        })
        .await;

        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "a call starting after the previous one fully finished should not reuse its result"
        );
    }
}
