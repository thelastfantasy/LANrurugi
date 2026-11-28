//! Sampling driven by observers rather than by a timer.
//!
//! The requirement is blunt: with the settings panel closed there must be no sampling task at all
//! (spec FR-001). So the sampler is not started at boot — it is started by the first
//! [`ObserverLease`] and aborted when the last one drops. History lives in this struct too, so it
//! dies with the last observer instead of accumulating for the life of the process.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;

use crate::resources::model::{ResourceSnapshot, Sample, WatchSection};
use crate::AppState;

/// 30 minutes at 1 Hz (spec FR-015). Bounded because the panel's sparklines cannot show more than a
/// screenful of points, and unbounded history is a leak with a nice name.
pub const HISTORY_LIMIT: usize = 1800;

/// One subscription to the sampler. Dropping it is what stops sampling.
pub struct ObserverLease {
    watch: Arc<ResourceWatch>,
}

impl Drop for ObserverLease {
    fn drop(&mut self) {
        self.watch.release();
    }
}

pub struct ResourceWatch {
    observers: AtomicUsize,
    samples_taken: AtomicU64,
    /// Tasks the request path has registered as in flight, counted by the API layer. Not tokio's own
    /// metrics: those require `--cfg tokio_unstable`, a workspace-wide build flag this feature does
    /// not impose (research R10).
    tracked_tasks: AtomicUsize,
    interval: Duration,
    sampler: Mutex<Option<tokio::task::JoinHandle<()>>>,
    history: Mutex<VecDeque<Sample>>,
    last: Mutex<Option<ResourceSnapshot>>,
    broadcast: broadcast::Sender<Sample>,
}

impl ResourceWatch {
    pub fn new() -> Arc<Self> {
        Self::with_interval(Duration::from_secs(1))
    }

    fn with_interval(interval: Duration) -> Arc<Self> {
        let (broadcast, _) = broadcast::channel(64);
        Arc::new(Self {
            observers: AtomicUsize::new(0),
            samples_taken: AtomicU64::new(0),
            tracked_tasks: AtomicUsize::new(0),
            interval,
            sampler: Mutex::new(None),
            history: Mutex::new(VecDeque::with_capacity(HISTORY_LIMIT)),
            last: Mutex::new(None),
            broadcast,
        })
    }

    pub fn global() -> &'static Arc<ResourceWatch> {
        static WATCH: std::sync::OnceLock<Arc<ResourceWatch>> = std::sync::OnceLock::new();
        WATCH.get_or_init(ResourceWatch::new)
    }

    /// Registers an observer and, if this is the first one, starts the sampler.
    pub async fn attach(self: &Arc<Self>, state: &AppState) -> ObserverLease {
        let lease = self.acquire();
        if self.observers.load(Ordering::SeqCst) == 1 {
            self.start_sampler(state);
        }
        lease
    }

    /// Counting half of [`attach`], separated so the lifecycle can be tested without a state.
    pub fn acquire(self: &Arc<Self>) -> ObserverLease {
        self.observers.fetch_add(1, Ordering::SeqCst);
        ObserverLease {
            watch: self.clone(),
        }
    }

    fn start_sampler(self: &Arc<Self>, state: &AppState) {
        let mut slot = self.sampler.lock().unwrap_or_else(|e| e.into_inner());
        if slot.as_ref().is_some_and(|handle| !handle.is_finished()) {
            return;
        }
        let watch = self.clone();
        let state = state.clone();
        let interval = self.interval;
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                if watch.observers.load(Ordering::SeqCst) == 0 {
                    break;
                }
                let snapshot = crate::resources::collect(&state).await;
                for alert in
                    crate::resources::alerts::Alerts::global().evaluate(&snapshot, snapshot.at)
                {
                    crate::resources::alerts::report(&alert);
                }
                watch.record(snapshot);
            }
        });
        *slot = Some(handle);
    }

    fn release(&self) {
        if self.observers.fetch_sub(1, Ordering::SeqCst) == 1 {
            // Aborting is immediate and cannot block: the sampler only awaits the tick and the
            // collection, both of which are cancellable.
            if let Some(handle) = self
                .sampler
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                handle.abort();
            }
            self.history
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            *self.last.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    pub fn record(&self, snapshot: ResourceSnapshot) {
        let sample = snapshot.sample();
        self.samples_taken.fetch_add(1, Ordering::SeqCst);
        {
            let mut history = self.history.lock().unwrap_or_else(|e| e.into_inner());
            history.push_back(sample.clone());
            while history.len() > HISTORY_LIMIT {
                history.pop_front();
            }
        }
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(snapshot);
        // No receivers is fine — the history above is the durable copy.
        let _ = self.broadcast.send(sample);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Sample> {
        self.broadcast.subscribe()
    }

    pub fn history(&self) -> Vec<Sample> {
        self.history
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn last(&self) -> Option<ResourceSnapshot> {
        self.last.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The newest snapshot, but only if it is younger than `max_age`. Synchronous on purpose: a
    /// reporter attaches it from a panic path, where there is no runtime to await on.
    pub fn last_within(&self, max_age: Duration) -> Option<ResourceSnapshot> {
        let snapshot = self.last()?;
        let age = crate::diagnostics::now_ms() - snapshot.at;
        (0 <= age && age < max_age.as_millis() as i64).then_some(snapshot)
    }

    pub fn observers(&self) -> usize {
        self.observers.load(Ordering::SeqCst)
    }

    pub fn samples_taken(&self) -> u64 {
        self.samples_taken.load(Ordering::SeqCst)
    }

    pub fn tracked_tasks(&self) -> u64 {
        self.tracked_tasks.load(Ordering::SeqCst) as u64
    }

    pub fn section(&self) -> WatchSection {
        WatchSection {
            observers: self.observers(),
            samples_taken: self.samples_taken(),
            sampler_running: self
                .sampler
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(|handle| !handle.is_finished()),
        }
    }

    /// Counts one in-flight API request. Dropping the guard decrements it, so a panic or an early
    /// return cannot leave the count stuck.
    pub fn track_task(self: &Arc<Self>) -> TrackedTask {
        self.tracked_tasks.fetch_add(1, Ordering::SeqCst);
        TrackedTask {
            watch: self.clone(),
        }
    }
}

impl Default for ResourceWatch {
    fn default() -> Self {
        // Only used through `Arc`; this exists so `ResourceWatch::default()` cannot panic.
        let (broadcast, _) = broadcast::channel(64);
        Self {
            observers: AtomicUsize::new(0),
            samples_taken: AtomicU64::new(0),
            tracked_tasks: AtomicUsize::new(0),
            interval: Duration::from_secs(1),
            sampler: Mutex::new(None),
            history: Mutex::new(VecDeque::new()),
            last: Mutex::new(None),
            broadcast,
        }
    }
}

pub struct TrackedTask {
    watch: Arc<ResourceWatch>,
}

impl Drop for TrackedTask {
    fn drop(&mut self) {
        self.watch.tracked_tasks.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(at: i64) -> ResourceSnapshot {
        let mut snapshot = crate::resources::model::ResourceSnapshot {
            at,
            monotonic_ms: at as u64,
            window_ms: 0,
            version: "test".into(),
            release: "test".into(),
            platform: "test/test".into(),
            build: Default::default(),
            server: Default::default(),
            cgroup: None,
            cgroup_unavailable: None,
            runtime: Default::default(),
            plugins: Default::default(),
            processes: Default::default(),
            models: Default::default(),
            queues: Default::default(),
            redis: None,
            redis_unavailable: None,
            network: Default::default(),
            caches: Default::default(),
            library: Default::default(),
            storage: Default::default(),
            gpu: None,
            gpu_unavailable: None,
            watch: Default::default(),
            truncations: Vec::new(),
        };
        snapshot.server.pid = 1;
        snapshot
    }

    /// V3, the counting half: the last observer leaving is what empties the watch.
    #[test]
    fn the_last_observer_leaving_clears_the_history() {
        let watch = ResourceWatch::new();
        let first = watch.acquire();
        let second = watch.acquire();
        assert_eq!(watch.observers(), 2);
        watch.record(snapshot(1));
        assert_eq!(watch.history().len(), 1);

        drop(first);
        assert_eq!(watch.observers(), 1);
        assert_eq!(watch.history().len(), 1, "history outlives one observer");

        drop(second);
        assert_eq!(watch.observers(), 0);
        assert!(watch.history().is_empty());
        assert!(watch.last().is_none());
    }

    /// V6: the ring buffer is bounded, and it drops the oldest points first.
    #[test]
    fn the_history_keeps_the_newest_points_and_never_grows_past_the_limit() {
        let watch = ResourceWatch::new();
        let lease = watch.acquire();
        for at in 0..(HISTORY_LIMIT as i64 + 25) {
            watch.record(snapshot(at));
        }
        let history = watch.history();
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history.first().map(|s| s.at), Some(25));
        assert_eq!(
            history.last().map(|s| s.at),
            Some(HISTORY_LIMIT as i64 + 24)
        );
        drop(lease);
    }

    #[test]
    fn a_recorded_snapshot_is_also_the_latest_one_and_reaches_subscribers() {
        let watch = ResourceWatch::new();
        let lease = watch.acquire();
        let mut receiver = watch.subscribe();
        watch.record(snapshot(7));
        assert_eq!(watch.last().map(|s| s.at), Some(7));
        assert_eq!(watch.samples_taken(), 1);
        assert!(receiver.try_recv().is_ok());
        drop(lease);
    }

    /// The tracked-task counter is what the panel shows instead of tokio internals; it must go back
    /// to zero when the guard drops, including on an early return.
    #[test]
    fn a_tracked_task_is_uncounted_when_its_guard_drops() {
        let watch = ResourceWatch::new();
        {
            let _task = watch.track_task();
            assert_eq!(watch.tracked_tasks(), 1);
        }
        assert_eq!(watch.tracked_tasks(), 0);
    }

    #[test]
    fn the_section_reports_observers_and_whether_the_sampler_runs() {
        let watch = ResourceWatch::new();
        let lease = watch.acquire();
        let section = watch.section();
        assert_eq!(section.observers, 1);
        assert!(!section.sampler_running, "acquire alone starts nothing");
        drop(lease);
    }
}
