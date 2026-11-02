//! Cross-crate filename reservation lock: serializes any caller's check-then-write cataloguing
//! sequence for a given destination *filename* against any other concurrent caller trying to
//! catalogue the same filename — regardless of which ingestion path (a downloaded file's rename
//! into the library, an upload, or the `notify` watcher's own live event stream) is doing the
//! cataloguing.
//!
//! Originally lived only in `lanrurugi-api::state` (guarding
//! `download_manager::ingest::catalogue_staged_file` against two concurrent downloads), lifted
//! here so `lanrurugi-scanner`'s watcher-driven `pipeline::run` can share the *same* lock instance
//! without `lanrurugi-scanner` depending on `lanrurugi-api` (which would be circular —
//! `lanrurugi-api` already depends on `lanrurugi-scanner`). Without this, a file renamed into the
//! watched archive directory by the download path triggers a `notify` event for the exact same
//! path essentially simultaneously, and both paths independently catalogue it with no mutual
//! exclusion between them — a real, observed data-corruption bug (a 331MB archive left with
//! `pagecount: 0` and an `arcsize` far under its real size after being ingested three times, two
//! of them racing `Rekeyed` outcomes against each other).

use std::collections::HashSet;
use std::sync::Arc;

use tokio::sync::Mutex;

/// Shared filename-reservation set. Cheap to `Clone` (an `Arc` underneath) — construct one
/// instance and hand the same clone to every ingestion entry point that must be mutually
/// exclusive with the others (currently: the download-ingest path and the filesystem watcher's
/// consumer loop).
#[derive(Clone, Default)]
pub struct FilenameLocks(Arc<Mutex<HashSet<String>>>);

impl FilenameLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserves `filename`, blocking (cooperatively, via polling with a short sleep; this is a
    /// rare, short-lived collision path, not a hot one, so a simple retry loop is preferable to
    /// pulling in a full keyed-mutex crate for this) until no other holder currently has it
    /// reserved. Returns a guard that releases the reservation on drop.
    pub async fn lock(&self, filename: &str) -> FilenameLockGuard {
        loop {
            {
                let mut locked = self.0.lock().await;
                if locked.insert(filename.to_string()) {
                    return FilenameLockGuard {
                        locks: self.0.clone(),
                        filename: filename.to_string(),
                    };
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

/// RAII guard returned by [`FilenameLocks::lock`] — releases the filename reservation when
/// dropped, including on an early `?`-propagated error from the guarded section. Holds its own
/// `Arc` clone of the lock set (not a borrow of `FilenameLocks`) so it isn't tied to the lock's
/// own lifetime/`Clone` semantics.
pub struct FilenameLockGuard {
    locks: Arc<Mutex<HashSet<String>>>,
    filename: String,
}

impl Drop for FilenameLockGuard {
    fn drop(&mut self) {
        let locks = self.locks.clone();
        let filename = std::mem::take(&mut self.filename);
        tokio::spawn(async move {
            locks.lock().await.remove(&filename);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_second_lock_on_the_same_filename_blocks_until_the_first_guard_drops() {
        let locks = FilenameLocks::new();
        let first = locks.lock("same-name.zip").await;

        // The second attempt must not resolve while `first` is still held — race it against a
        // short timeout rather than asserting on internal state, so this test exercises the same
        // polling/blocking behavior a real caller relies on.
        let second_attempt = tokio::time::timeout(std::time::Duration::from_millis(100), async {
            locks.lock("same-name.zip").await
        });
        assert!(
            second_attempt.await.is_err(),
            "a second lock on an already-held filename must not succeed while the first is held"
        );

        drop(first);

        // Now that the first guard has dropped, a fresh attempt must succeed within a reasonable
        // window (the guard's own `Drop` impl removes the reservation via a spawned task, so this
        // isn't instantaneous).
        let third_attempt = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            locks.lock("same-name.zip").await
        })
        .await;
        assert!(
            third_attempt.is_ok(),
            "a lock must become available again after the holding guard is dropped"
        );
    }

    #[tokio::test]
    async fn locks_on_different_filenames_never_block_each_other() {
        let locks = FilenameLocks::new();

        let _a = locks.lock("a.zip").await;
        // Must resolve immediately (well within a short timeout) since it's a different key.
        let b = tokio::time::timeout(std::time::Duration::from_millis(50), async {
            locks.lock("b.zip").await
        })
        .await;
        assert!(b.is_ok(), "locking a different filename must never block");
    }

    #[tokio::test]
    async fn simulated_concurrent_ingests_of_the_same_filename_are_fully_serialized() {
        // Mirrors the real bug this guards against: two concurrent "downloads" racing to
        // catalogue the same destination filename. Each task holds the lock across a simulated
        // check-then-write window (a short sleep) and appends to a shared log while holding it —
        // if the lock didn't actually serialize them, both tasks' "enter"/"exit" pairs would
        // interleave instead of nesting.
        let locks = FilenameLocks::new();
        let log: Arc<Mutex<Vec<&'static str>>> = Default::default();

        let run = |locks: FilenameLocks, log: Arc<Mutex<Vec<&'static str>>>, tag: &'static str| {
            tokio::spawn(async move {
                let _guard = locks.lock("colliding.zip").await;
                log.lock().await.push(tag);
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                log.lock().await.push(tag);
            })
        };

        let h1 = run(locks.clone(), log.clone(), "enter1-exit1");
        let h2 = run(locks.clone(), log.clone(), "enter2-exit2");
        h1.await.unwrap();
        h2.await.unwrap();

        let entries = log.lock().await.clone();
        assert_eq!(entries.len(), 4);
        // Whichever task ran first, its two entries must be adjacent (fully nested), never
        // interleaved with the other task's — that's what "serialized" means here.
        assert_eq!(entries[0], entries[1], "the two halves of the first-run task must be adjacent, not interleaved with the second task");
        assert_eq!(entries[2], entries[3], "the two halves of the second-run task must be adjacent, not interleaved with the first task");
    }

    /// The scenario this whole cross-crate lift exists for: a "download path" holds the lock
    /// across a simulated rename+catalogue window while a "watcher path" — using the *same*
    /// `FilenameLocks` instance, exactly as `lanrurugi-scanner::pipeline::run` will — tries to
    /// lock the identical filename. The watcher side must not proceed until the download side's
    /// guard drops.
    #[tokio::test]
    async fn a_watcher_style_caller_blocks_on_a_download_style_caller_holding_the_same_key() {
        let locks = FilenameLocks::new();
        let order: Arc<Mutex<Vec<&'static str>>> = Default::default();

        let download_locks = locks.clone();
        let download_order = order.clone();
        let download_task = tokio::spawn(async move {
            let _guard = download_locks.lock("book.zip").await;
            download_order.lock().await.push("download-start");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            download_order.lock().await.push("download-end");
        });

        // Give the "download" task a head start so it reliably wins the initial race.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let watcher_locks = locks.clone();
        let watcher_order = order.clone();
        let watcher_task = tokio::spawn(async move {
            let _guard = watcher_locks.lock("book.zip").await;
            watcher_order.lock().await.push("watcher-start");
        });

        download_task.await.unwrap();
        watcher_task.await.unwrap();

        let entries = order.lock().await.clone();
        assert_eq!(
            entries,
            vec!["download-start", "download-end", "watcher-start"],
            "the watcher-style caller must not observe the lock as free until the download-style \
             caller's guard has dropped"
        );
    }
}
