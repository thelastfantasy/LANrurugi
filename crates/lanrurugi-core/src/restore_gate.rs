//! A process-wide gate that pauses Redis writes while a full database restore replaces the
//! dataset.
//!
//! `DEBUG RELOAD` swaps the whole keyspace at once. A write that lands inside that window either
//! disappears with the old dataset or is applied afterwards to the restored (older) one — a new
//! thumbnail row for an archive that no longer exists, a grouping label on a different library.
//! So writes are refused instead of replayed, and long jobs stop at their next checkpoint.
//!
//! Reads stay open: the UI has to keep polling the restore job's own progress.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Notify;

/// The message every paused write gets, shared by the HTTP gate and the job checkpoints so the
/// frontend can key off one string.
pub const RESTORE_IN_PROGRESS_REASON: &str = "restore_in_progress";
pub const RESTORE_IN_PROGRESS_MESSAGE: &str =
    "A database restore is in progress; writes are paused.";
/// What a long job writes into its own job status when a checkpoint stops it.
pub const RESTORE_ABORTED_MESSAGE: &str =
    "The database is being restored, so this job stopped before writing further. Run it again once \
     the restore has finished.";

#[derive(Debug, Default)]
struct Inner {
    closed: AtomicBool,
    in_flight: AtomicUsize,
    idle: Notify,
}

/// Cheap to clone; every clone shares one gate.
#[derive(Debug, Clone, Default)]
pub struct RestoreGate {
    inner: Arc<Inner>,
}

impl RestoreGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// Shuts the gate. Already-admitted writes keep running; see [`Self::wait_for_writes`].
    pub fn close(&self) {
        self.inner.closed.store(true, Ordering::Release);
    }

    pub fn open(&self) {
        self.inner.closed.store(false, Ordering::Release);
        self.inner.idle.notify_waiters();
    }

    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::Acquire)
    }

    /// Registers one in-flight write. The caller must call [`Self::leave_write`] exactly once.
    pub fn enter_write(&self) {
        self.inner.in_flight.fetch_add(1, Ordering::AcqRel);
    }

    pub fn leave_write(&self) {
        if self.inner.in_flight.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.inner.idle.notify_waiters();
        }
    }

    /// Waits for admitted writes to finish, returning how many were still running at `timeout`
    /// (zero when it drained). A restore proceeds either way: the operator asked for it, and the
    /// pre-restore backup is the fallback, so the caller only logs the stragglers.
    pub async fn wait_for_writes(&self, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = self.in_flight();
            if remaining == 0 || Instant::now() >= deadline {
                return remaining;
            }
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Re-check after registering: a decrement between the check above and `enable` would
            // otherwise park us until the timeout.
            if self.in_flight() == 0 {
                return 0;
            }
            let _ = tokio::time::timeout_at(deadline.into(), notified).await;
        }
    }
}

/// RAII form of [`RestoreGate::enter_write`]/[`RestoreGate::leave_write`].
pub struct WriteGuard {
    gate: RestoreGate,
}

impl WriteGuard {
    pub fn new(gate: &RestoreGate) -> Self {
        gate.enter_write();
        Self { gate: gate.clone() }
    }
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        self.gate.leave_write();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn close_waits_for_admitted_writes() {
        let gate = RestoreGate::new();
        let guard = WriteGuard::new(&gate);
        gate.close();
        assert!(gate.closed());
        assert_eq!(gate.in_flight(), 1);
        let waiter = {
            let gate = gate.clone();
            tokio::spawn(async move { gate.wait_for_writes(Duration::from_secs(5)).await })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(guard);
        assert_eq!(waiter.await.unwrap(), 0);
    }

    #[tokio::test]
    async fn wait_times_out_and_reports_the_remainder() {
        let gate = RestoreGate::new();
        let _guard = WriteGuard::new(&gate);
        gate.close();
        assert_eq!(gate.wait_for_writes(Duration::from_millis(30)).await, 1);
        gate.open();
        assert!(!gate.closed());
    }
}
