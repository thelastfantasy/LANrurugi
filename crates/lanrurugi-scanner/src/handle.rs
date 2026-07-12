//! Owns the watcher + ingestion-consumer background tasks as one process-in-process unit
//! (constitution Principle III — no separate Shinobu process), so the `/shinobu/*` endpoints have
//! something concrete to start/stop/restart.

use std::path::PathBuf;
use std::sync::Arc;

use deadpool_redis::Pool;
use lanrurugi_storage::repository::ArchiveRepository;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

struct Running {
    _watcher: notify::RecommendedWatcher,
    task: JoinHandle<()>,
}

#[derive(Clone, Default)]
pub struct ScannerHandle {
    running: Arc<Mutex<Option<Running>>>,
}

impl ScannerHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn is_alive(&self) -> bool {
        self.running.lock().await.is_some()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        archive_dir: PathBuf,
        thumb_dir: PathBuf,
        config_pool: Pool,
        search_pool: Pool,
        archives: ArchiveRepository,
    ) -> Result<(), crate::watcher::WatcherError> {
        let mut guard = self.running.lock().await;
        if guard.is_some() {
            return Ok(());
        }
        let (watcher, rx) = crate::watcher::watch(&archive_dir)?;
        let task = tokio::spawn(crate::pipeline::run(
            rx,
            archives,
            config_pool,
            search_pool,
            thumb_dir,
        ));
        *guard = Some(Running {
            _watcher: watcher,
            task,
        });
        Ok(())
    }

    pub async fn stop(&self) {
        if let Some(running) = self.running.lock().await.take() {
            running.task.abort();
        }
    }

    /// Stops then immediately restarts the watcher, matching legacy's `shinobu_restart`.
    #[allow(clippy::too_many_arguments)]
    pub async fn restart(
        &self,
        archive_dir: PathBuf,
        thumb_dir: PathBuf,
        config_pool: Pool,
        search_pool: Pool,
        archives: ArchiveRepository,
    ) -> Result<(), crate::watcher::WatcherError> {
        self.stop().await;
        self.start(archive_dir, thumb_dir, config_pool, search_pool, archives)
            .await
    }
}
