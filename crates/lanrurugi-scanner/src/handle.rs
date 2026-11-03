//! Owns the watcher + ingestion-consumer background tasks as one process-in-process unit
//! (constitution Principle III — no separate Shinobu process), so the `/shinobu/*` endpoints have
//! something concrete to start/stop/restart.
//!
//! A supervisor keeps the watch alive by itself: a watcher error, the content directory being
//! removed or remounted, or the ingestion task ending all count as a failure. It records why,
//! retries with backoff, and after recovering runs one catch-up scan for files that arrived while
//! nothing was watching. A dropped-events notice runs the catch-up scan without restarting.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use deadpool_redis::Pool;
use lanrurugi_core::filename_lock::FilenameLocks;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_search::Equivalence;
use lanrurugi_storage::repository::ArchiveRepository;
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::watcher::WatcherSignal;

/// Everything the watcher and its catch-up scans need, kept so the supervisor can restart alone.
#[derive(Clone)]
pub struct ScannerConfig {
    pub archive_dir: PathBuf,
    pub thumb_dir: PathBuf,
    pub config_pool: Pool,
    pub search_pool: Pool,
    pub equivalence: Arc<Equivalence>,
    pub archives: ArchiveRepository,
    pub new_archive_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::events::IngestEvent>>,
    pub locks: FilenameLocks,
    pub jobs: JobRegistry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WatcherState {
    Running,
    /// Failed and waiting to retry; see `last_error`.
    Recovering,
    Stopped,
}

/// What the status endpoint reports.
#[derive(Debug, Clone, Serialize)]
pub struct WatcherStatus {
    pub state: WatcherState,
    pub last_error: Option<String>,
    /// Automatic recoveries since the watcher was last started.
    pub recoveries: u32,
}

impl Default for WatcherStatus {
    fn default() -> Self {
        Self {
            state: WatcherState::Stopped,
            last_error: None,
            recoveries: 0,
        }
    }
}

type SharedStatus = Arc<std::sync::Mutex<WatcherStatus>>;

#[derive(Clone, Default)]
pub struct ScannerHandle {
    supervisor: Arc<Mutex<Option<JoinHandle<()>>>>,
    status: SharedStatus,
}

impl ScannerHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn is_alive(&self) -> bool {
        self.status().state == WatcherState::Running
    }

    pub fn status(&self) -> WatcherStatus {
        self.status
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Starts watching unless already started. `catch_up` runs a catch-up scan once the watch is
    /// up — off at boot, where the startup scan already covers it.
    pub async fn start(
        &self,
        config: ScannerConfig,
        catch_up: bool,
    ) -> Result<(), crate::watcher::WatcherError> {
        let mut guard = self.supervisor.lock().await;
        if guard.as_ref().is_some_and(|h| !h.is_finished()) {
            return Ok(());
        }
        // The first attempt runs here so a bad path is still an error for the caller.
        let first = crate::watcher::watch(&config.archive_dir)?;
        set(&self.status, |s| {
            *s = WatcherStatus {
                state: WatcherState::Running,
                ..Default::default()
            }
        });
        *guard = Some(tokio::spawn(supervise(
            config,
            self.status.clone(),
            first,
            catch_up,
        )));
        Ok(())
    }

    pub async fn stop(&self) {
        if let Some(supervisor) = self.supervisor.lock().await.take() {
            // Dropping the supervisor's future drops its watcher and, through `AbortOnDrop`, its
            // ingestion task.
            supervisor.abort();
        }
        set(&self.status, |s| s.state = WatcherState::Stopped);
    }

    /// Stops then restarts the watcher, with a catch-up scan — the manual fallback for whatever
    /// the supervisor could not fix by itself.
    pub async fn restart(&self, config: ScannerConfig) -> Result<(), crate::watcher::WatcherError> {
        self.stop().await;
        self.start(config, true).await
    }
}

fn set(status: &SharedStatus, f: impl FnOnce(&mut WatcherStatus)) {
    f(&mut status.lock().unwrap_or_else(|e| e.into_inner()));
}

/// Aborts a task when the owner goes away, so stopping the supervisor stops its children.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

const BACKOFF_START: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// A watch that held this long before failing restarts the backoff from the beginning.
const HEALTHY_RUN: Duration = Duration::from_secs(120);
/// How often the supervisor checks the content directory is still the one being watched.
const ROOT_CHECK_INTERVAL: Duration = Duration::from_secs(30);

type Watch = (
    notify::RecommendedWatcher,
    tokio::sync::mpsc::UnboundedReceiver<PathBuf>,
    tokio::sync::mpsc::UnboundedReceiver<WatcherSignal>,
);

async fn supervise(
    config: ScannerConfig,
    status: SharedStatus,
    first: Watch,
    catch_up_first: bool,
) {
    let mut pending = Some(first);
    let mut catch_up = catch_up_first;
    let mut backoff = BACKOFF_START;
    loop {
        let watch = match pending.take() {
            Some(watch) => Ok(watch),
            None => crate::watcher::watch(&config.archive_dir),
        };
        let failure = match watch {
            Err(e) => e.to_string(),
            Ok((watcher, rx, signals)) => {
                set(&status, |s| s.state = WatcherState::Running);
                if catch_up {
                    run_catch_up(&config, "watch restarted").await;
                }
                let started = Instant::now();
                let failure = watch_until_failure(&config, rx, signals).await;
                drop(watcher);
                if started.elapsed() >= HEALTHY_RUN {
                    backoff = BACKOFF_START;
                }
                failure
            }
        };
        tracing::warn!(error = %failure, retry_in = ?backoff, "file watcher failed; recovering");
        set(&status, |s| {
            s.state = WatcherState::Recovering;
            s.last_error = Some(failure);
            s.recoveries += 1;
        });
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
        catch_up = true;
    }
}

/// Runs the ingestion task for one watch until something makes the watch unreliable; returns why.
async fn watch_until_failure(
    config: &ScannerConfig,
    rx: tokio::sync::mpsc::UnboundedReceiver<PathBuf>,
    mut signals: tokio::sync::mpsc::UnboundedReceiver<WatcherSignal>,
) -> String {
    let mut ingest = AbortOnDrop(tokio::spawn(crate::pipeline::run(
        rx,
        config.archives.clone(),
        config.config_pool.clone(),
        config.search_pool.clone(),
        config.equivalence.clone(),
        config.thumb_dir.clone(),
        config.new_archive_tx.clone(),
        config.locks.clone(),
    )));
    let root = dir_identity(&config.archive_dir);
    let mut root_check = tokio::time::interval(ROOT_CHECK_INTERVAL);
    root_check.tick().await;
    loop {
        tokio::select! {
            ended = &mut ingest.0 => {
                return match ended {
                    Err(e) if e.is_panic() => "the ingestion task panicked".to_string(),
                    _ => "the ingestion task ended".to_string(),
                };
            }
            signal = signals.recv() => match signal {
                Some(WatcherSignal::Failed(reason)) => return reason,
                Some(WatcherSignal::Rescan) => run_catch_up(config, "watcher dropped events").await,
                None => return "the watcher stopped reporting".to_string(),
            },
            _ = root_check.tick() => {
                if dir_identity(&config.archive_dir) != root {
                    return "the content directory was replaced or remounted".to_string();
                }
            }
        }
    }
}

/// Ingests whatever is on disk but not yet in the library — the same scan the server runs at boot,
/// recorded as its own job.
async fn run_catch_up(config: &ScannerConfig, reason: &str) {
    let job_id = config.jobs.create("watcher_catch_up_scan").await;
    config.jobs.mark_active(&job_id).await;
    let summary = crate::full_scan::full_scan(
        &config.archive_dir,
        &config.archives,
        &config.config_pool,
        &config.search_pool,
        config.equivalence.clone(),
        &config.thumb_dir,
        &config.jobs,
        &job_id,
        config.new_archive_tx.clone(),
    )
    .await;
    tracing::info!(
        reason,
        scanned = summary.scanned,
        newly_catalogued = summary.catalogued,
        errors = summary.errors,
        "watcher catch-up scan complete"
    );
    config
        .jobs
        .finish(
            &job_id,
            serde_json::json!({
                "reason": reason,
                "scanned": summary.scanned,
                "already_known": summary.already_known,
                "newly_catalogued": summary.catalogued,
                "errors": summary.errors,
            }),
        )
        .await;
}

/// The directory's device and inode, so a remount (same path, different filesystem) is noticed;
/// `None` when it is missing. Platforms without inodes compare existence only.
fn dir_identity(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        Some((0, 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_or_replaced_directory_changes_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("content");
        std::fs::create_dir(&path).unwrap();
        let before = dir_identity(&path);
        assert!(before.is_some());
        // A different directory moved into place (what a remount or a replaced volume looks
        // like). Created while the original still exists, so it cannot reuse its inode.
        let other = dir.path().join("other");
        std::fs::create_dir(&other).unwrap();
        std::fs::remove_dir(&path).unwrap();
        assert_eq!(dir_identity(&path), None);
        std::fs::rename(&other, &path).unwrap();
        #[cfg(unix)]
        assert_ne!(dir_identity(&path), before);
    }

    #[test]
    fn status_starts_stopped() {
        let handle = ScannerHandle::new();
        assert_eq!(handle.status().state, WatcherState::Stopped);
    }
}
