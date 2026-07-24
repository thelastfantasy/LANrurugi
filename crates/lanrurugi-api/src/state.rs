use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::download_queue::DownloadQueueRepository;
use lanrurugi_storage::plugin_options::PluginOptionsRepository;
use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::repository::{
    ArchiveRepository, CategoryRepository, GroupingRepository, StampRepository,
};
use tokio::sync::Mutex;

use crate::download_manager::DownloadManager;

/// API-key auth configuration. Mirrors legacy `LRR_CONF`'s `apikey`/`enable_pass` semantics
/// (verified: `~/LANraragi/lib/LANraragi/Utils/Login.pm::is_logged_in_api`) — see
/// `lanrurugi-server/src/middleware/auth.rs` for the actual check.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    /// Raw (not base64-encoded) API key. Empty means no key has ever been configured.
    pub api_key: String,
    /// Legacy "password protection" toggle. When `false`, every request is authorized
    /// regardless of `api_key` (an intentionally open instance) — matches
    /// `$c->LRR_CONF->enable_pass == 0` short-circuiting the legacy check.
    pub enable_pass: bool,
}

/// Filesystem locations LANrurugi reads/writes outside Redis. `thumb_dir` mirrors legacy's
/// `thumbdir` config (default `./thumb`, verified: `Model/Config.pm::get_thumbdir`).
#[derive(Debug, Clone)]
pub struct LibraryPaths {
    pub archive_dir: PathBuf,
    pub thumb_dir: PathBuf,
    /// Staging area for uploads and other transient files (legacy `Utils::TempFolder::get_temp`).
    pub temp_dir: PathBuf,
    /// Where the categorized log files (`lanrurugi_server::telemetry::CATEGORIES`) are written —
    /// `None` when running without file logging (e.g. under `cargo test`, where nothing sets it).
    pub log_dir: Option<PathBuf>,
}

/// Repository handles, all bound to the archive logical DB (constitution Principle I / verified
/// legacy DB layout — see `lanrurugi_storage::redis` module docs).
#[derive(Clone)]
pub struct Repositories {
    pub archives: Arc<ArchiveRepository>,
    pub categories: Arc<CategoryRepository>,
    pub groupings: Arc<GroupingRepository>,
    pub stamps: Arc<StampRepository>,
}

impl Repositories {
    pub fn new(redis: &RedisDbs) -> Self {
        Self {
            archives: Arc::new(ArchiveRepository::new(redis.archive.clone())),
            categories: Arc::new(CategoryRepository::new(redis.archive.clone())),
            groupings: Arc::new(GroupingRepository::new(redis.archive.clone())),
            stamps: Arc::new(StampRepository::new(redis.archive.clone())),
        }
    }
}

/// Shared Axum application state: one instance is cloned (cheaply — everything inside is an
/// `Arc`-backed pool/registry) into every request.
#[derive(Clone)]
pub struct AppState {
    pub redis: RedisDbs,
    pub repos: Repositories,
    pub jobs: JobRegistry,
    pub auth: AuthConfig,
    pub library: LibraryPaths,
    pub scanner: ScannerHandle,
    pub plugins: Arc<PluginPool>,
    /// Directory of installed plugin `.ts` files (one per namespace), scanned by
    /// `GET /plugins/{type}` to discover what's available.
    pub plugins_dir: PathBuf,
    /// One [`DownloadManager`] per download-plugin namespace (`specs/005-download-plugin-progress`)
    /// — separate instances because different plugins' domain-concurrency/rate-limit rules are
    /// independent of one another (spec Assumptions: "settings changes apply per-plugin, not
    /// globally"); a shared instance would incorrectly pool e.g. two different plugins' downloads
    /// from the same CDN hostname under one concurrency limit. Created lazily on first use per
    /// namespace (`plugins::download_manager_for`).
    pub download_managers: Arc<Mutex<HashMap<String, Arc<DownloadManager>>>>,
    /// Persisted user overrides of a download plugin's `pluginOptions()` defaults
    /// (`specs/005-download-plugin-progress`), on the `config` logical DB alongside the rest of
    /// LANrurugi's own (non-legacy) settings.
    pub plugin_options: Arc<PluginOptionsRepository>,
    /// Persistent, plugin-grouped download queue backing the Upload page's right-hand panel — so
    /// a queued/in-progress download survives a page refresh or a different browser tab. Also on
    /// the `config` logical DB, same placement as `plugin_options`.
    pub download_queue: Arc<DownloadQueueRepository>,
    /// Collapses concurrent on-demand cover-thumbnail regenerations for the same archive ID onto
    /// one worker, and bounds how many *distinct* archive covers regenerate at once (see
    /// `lanrurugi_core::singleflight`'s own doc comment) — used by
    /// `archives::get_archive_thumbnail` when a thumbnail is missing from disk (e.g. `thumb_dir`
    /// was never mounted, or its contents were lost). Value is `Option<(content-type, bytes)>`
    /// (`None` when generation fails) rather than a `Response` directly since `Response` isn't
    /// `Clone` — each concurrent caller wraps its own clone of the shared bytes into its own
    /// response.
    pub thumbnail_singleflight: Arc<ThumbnailSingleflight>,
    /// Same mechanism as `thumbnail_singleflight`, keyed by `(archive_id, entry_path)` — used by
    /// `archives::get_page` (reader page fetch/prefetch) so several pages of the same or
    /// different archives requested at once don't each independently reopen and linearly scan
    /// the archive file. Value is `Result<(content-type, bytes), error message>` (an owned
    /// `String` error rather than propagating the original error type, so every waiter — not
    /// just the one that ran `work` — can reconstruct an error response).
    pub page_singleflight: Arc<PageSingleflight>,
    /// Sender half of the channel a long-lived task (spawned once in `lanrurugi-server::main`)
    /// drains to run every "自动运行"/enabled metadata plugin on each newly-catalogued archive id
    /// it receives (matching legacy's own `Shinobu.pm::add_new_file` →
    /// `exec_enabled_plugins_on_file`) — carried on `AppState` itself so every call site that
    /// starts/restarts the watcher or a full scan (`shinobu.rs`, `database.rs::rebuild_index`,
    /// `main.rs`'s own startup scan) can hand the *same* long-lived consumer a clone of this one
    /// sender, rather than each spawning its own short-lived, redundant consumer task.
    pub new_archive_tx: tokio::sync::mpsc::UnboundedSender<String>,
    /// One [`CancellationToken`] per in-flight download-queue item (keyed by queue item ID, not
    /// job ID — the queue item is the stable, user-facing identity a "stop" button acts on).
    /// Cooperative rather than `AbortHandle`-based: the download loop
    /// (`download_manager::stream::download_one`) observes this token via `tokio::select!` at the
    /// same point it already handles a real network error, reusing that same partial-file cleanup
    /// path — an `AbortHandle`-based cancellation would drop the task at an arbitrary `.await`
    /// point instead, mid-`write_all`, with no chance to run its own cleanup code, leaking a
    /// UUID-named staging file in `temp_dir` with no way to trace it back to the queue item that
    /// caused it. Entries are inserted right before the download task is spawned and removed once
    /// that task actually finishes (success, error, or real cancellation) — see
    /// `download_queue::stop_one` and `plugins::start_download`.
    pub download_cancellations: Arc<Mutex<HashMap<String, tokio_util::sync::CancellationToken>>>,
    /// Guards the check-then-write window in `download_manager::ingest::catalogue_staged_file`
    /// (filename-collision check against the catalog, through to the new archive record actually
    /// being saved) against two concurrent downloads that resolve to the *same* destination
    /// filename racing each other — without this, both could observe "no existing archive with
    /// this filename" before either finished cataloguing, so both proceed, and whichever renames
    /// its staged file into `archive_dir` last silently clobbers the other's bytes on disk while
    /// leaving two catalog records pointing at one (partially dangling) file. Keyed by the
    /// resolved destination filename (not archive ID, which doesn't exist yet at check time) —
    /// see `AppState::lock_filename` for how a caller actually acquires this.
    pub filename_locks: Arc<Mutex<HashSet<String>>>,
}

impl AppState {
    /// Serializes `download_manager::ingest::catalogue_staged_file`'s check-then-write sequence
    /// per destination `filename` — blocks until no other in-flight download is currently
    /// cataloguing the same filename, then reserves it until the returned guard drops. Thin
    /// wrapper around [`lock_filename_in`] (split out so the actual locking logic is testable
    /// against a bare `Arc<Mutex<HashSet<String>>>`, without needing a full `AppState`).
    pub async fn lock_filename(&self, filename: &str) -> FilenameLockGuard {
        lock_filename_in(&self.filename_locks, filename).await
    }
}

/// Reserves `filename` in `locks`, blocking (cooperatively, via polling with a short sleep; this
/// is a rare, short-lived collision path, not a hot one, so a simple retry loop is preferable to
/// pulling in a full keyed-mutex crate for one call site) until no other holder currently has it
/// reserved. See [`AppState::filename_locks`] for why this exists at all.
async fn lock_filename_in(
    locks: &Arc<Mutex<HashSet<String>>>,
    filename: &str,
) -> FilenameLockGuard {
    loop {
        {
            let mut locked = locks.lock().await;
            if locked.insert(filename.to_string()) {
                return FilenameLockGuard {
                    locks: locks.clone(),
                    filename: filename.to_string(),
                };
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// RAII guard returned by [`AppState::lock_filename`] — releases the filename reservation when
/// dropped, including on an early `?`-propagated error from the guarded section (no explicit
/// unlock call needed at any of `catalogue_staged_file`'s several early-return points). Holds its
/// own `Arc` clone of the lock map (not a borrow of `AppState`) so it isn't tied to `AppState`'s
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

/// See [`AppState::thumbnail_singleflight`].
pub type ThumbnailSingleflight =
    lanrurugi_core::singleflight::Singleflight<String, Option<(&'static str, bytes::Bytes)>>;

/// See [`AppState::page_singleflight`].
pub type PageSingleflight = lanrurugi_core::singleflight::Singleflight<
    (String, String),
    Result<(&'static str, bytes::Bytes), String>,
>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_second_lock_on_the_same_filename_blocks_until_the_first_guard_drops() {
        let locks: Arc<Mutex<HashSet<String>>> = Default::default();

        let first = lock_filename_in(&locks, "same-name.zip").await;

        // The second attempt must not resolve while `first` is still held — race it against a
        // short timeout rather than asserting on internal state, so this test exercises the same
        // polling/blocking behavior a real caller relies on.
        let second_attempt = tokio::time::timeout(std::time::Duration::from_millis(100), async {
            lock_filename_in(&locks, "same-name.zip").await
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
            lock_filename_in(&locks, "same-name.zip").await
        })
        .await;
        assert!(
            third_attempt.is_ok(),
            "a lock must become available again after the holding guard is dropped"
        );
    }

    #[tokio::test]
    async fn locks_on_different_filenames_never_block_each_other() {
        let locks: Arc<Mutex<HashSet<String>>> = Default::default();

        let _a = lock_filename_in(&locks, "a.zip").await;
        // Must resolve immediately (well within a short timeout) since it's a different key.
        let b = tokio::time::timeout(std::time::Duration::from_millis(50), async {
            lock_filename_in(&locks, "b.zip").await
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
        let locks: Arc<Mutex<HashSet<String>>> = Default::default();
        let log: Arc<Mutex<Vec<&'static str>>> = Default::default();

        let run = |locks: Arc<Mutex<HashSet<String>>>,
                   log: Arc<Mutex<Vec<&'static str>>>,
                   tag: &'static str| {
            tokio::spawn(async move {
                let _guard = lock_filename_in(&locks, "colliding.zip").await;
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
}
