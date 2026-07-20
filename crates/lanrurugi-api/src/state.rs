use std::collections::HashMap;
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
}
