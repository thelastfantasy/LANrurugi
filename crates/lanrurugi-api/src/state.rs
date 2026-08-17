use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use lanrurugi_core::filename_lock::{FilenameLockGuard, FilenameLocks};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::activity::ActivityRepository;
use lanrurugi_storage::api_tokens::ApiTokenRepository;
use lanrurugi_storage::compare_cache::CompareCacheRepository;
use lanrurugi_storage::download_queue::DownloadQueueRepository;
use lanrurugi_storage::ignored_group_suggestions::IgnoredGroupSuggestionsRepository;
use lanrurugi_storage::plugin_options::PluginOptionsRepository;
use lanrurugi_storage::recommend_cache::RecommendCacheRepository;
use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::refresh_tokens::RefreshTokenRepository;
use lanrurugi_storage::repository::{
    ArchiveRepository, CategoryRepository, GroupingRepository, StampRepository,
};
use tokio::sync::Mutex;

use crate::download_manager::DownloadManager;

/// Auth configuration not read live from Redis (unlike `enablepass`/token lifetimes, which
/// `lanrurugi_api::auth::LiveAuthConfig` re-reads on every request) — set once at startup from
/// CLI args/env and never changed after. No more `api_key` here (issue #54 replaced the legacy
/// single-fixed-key mechanism with `lanrurugi_storage::api_tokens`'s real multi-token system —
/// see `.specify/memory/constitution.md` Principle II's own annotation on this deliberate break).
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    /// Legacy "password protection" toggle. When `false`, every request is authorized
    /// regardless of credentials (an intentionally open instance) — matches
    /// `$c->LRR_CONF->enable_pass == 0` short-circuiting the legacy check.
    pub enable_pass: bool,
    /// Appends `; Secure` to both auth cookies when set — never inferred from a spoofable
    /// `X-Forwarded-Proto` header (this app has no trusted-proxy allowlist to validate one
    /// against); the operator opts in explicitly via `--force-secure-cookies` /
    /// `LANRURUGI_FORCE_SECURE_COOKIES` once actually deployed behind HTTPS. Defaults `false`
    /// for local-HTTP dev friendliness.
    pub force_secure_cookies: bool,
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
    /// Bumped on every successful plugin-options write (`PUT`/`DELETE
    /// /api/plugins/{namespace}/options`, `plugins.rs`) — the per-download live rate-limit
    /// resolver (`download_manager::live_rate::LiveRateResolver`) compares this against its own
    /// cached generation on every chunk read, so a rate-cap change takes effect mid-transfer with
    /// a single atomic load per chunk instead of a Redis read per chunk.
    pub plugin_options_generation: Arc<std::sync::atomic::AtomicU64>,
    /// Persistent, plugin-grouped download queue backing the Upload page's right-hand panel — so
    /// a queued/in-progress download survives a page refresh or a different browser tab. Also on
    /// the `config` logical DB, same placement as `plugin_options`.
    pub download_queue: Arc<DownloadQueueRepository>,
    /// Precomputed reader-recommendation embedding vectors and per-archive Top-N similar-archive
    /// lists (issue #70) — also on the `config` logical DB, same placement as `plugin_options`
    /// and `download_queue`. See `lanrurugi_storage::recommend_cache` module docs and
    /// `recommend_precompute.rs` for how these are written.
    pub recommend_cache: Arc<RecommendCacheRepository>,
    /// User-dismissed AI Tankoubon-grouping suggestions ("Don't suggest this again" —
    /// `tankoubon_grouping.rs`) — also on the `config` logical DB, same placement as
    /// `plugin_options`/`download_queue`/`recommend_cache`.
    pub ignored_group_suggestions: Arc<IgnoredGroupSuggestionsRepository>,
    /// Cached `POST /download_queue/{id}/compare` results (issue #77) — also on the `config`
    /// logical DB, same placement as the other additive caches above. See
    /// `lanrurugi_storage::compare_cache` module docs for the capacity/lifecycle policy.
    pub compare_cache: Arc<CompareCacheRepository>,
    /// Reader recommendation engine (ONNX embedding) — `None`-backed until the model download
    /// plus load completes at startup; the endpoint returns 503 `model_not_ready` meanwhile
    /// (see `recommend.rs`'s module docs).
    pub recommender: Arc<crate::recommend::RecommendService>,
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
    /// filename racing each other, *and* against the `notify` watcher
    /// (`lanrurugi_scanner::pipeline::run`) independently cataloguing the exact same freshly-
    /// renamed file at nearly the same instant — the actual real-world race this type exists to
    /// fix (a file renamed into the watched `archive_dir` triggers a filesystem event for that
    /// same path essentially simultaneously with this field's own caller cataloguing it). Lives in
    /// `lanrurugi-core` (not here) precisely so `lanrurugi-scanner`'s watcher-consumer loop can
    /// share the *same* lock instance without a circular crate dependency — `main.rs` constructs
    /// one `FilenameLocks` and hands a clone both to this field and to `scanner.start(...)`'s
    /// `locks` parameter; the two must be the same underlying lock, not two independent ones, or
    /// the watcher path is left unguarded again. See `lanrurugi_core::filename_lock` for the
    /// implementation and the full incident this fixes.
    pub filename_locks: FilenameLocks,
    /// SSE broadcast — every `update_queue_item_state` call sends a clone here after the Redis
    /// write succeeds, so `GET /download_queue/stream` subscribers see the change without polling.
    /// SSE broadcast — every `update_queue_item_state` call sends a clone here after the Redis
    /// write succeeds. `None` in test/bench contexts (no SSE subscribers); real `serve` always
    /// sets this.
    pub download_queue_tx: Option<tokio::sync::broadcast::Sender<serde_json::Value>>,
    /// The SPA login flow's revocable refresh-token half (`lanrurugi_core::session` mints the
    /// paired stateless JWT access token) — on the `config` logical DB, same placement as
    /// `plugin_options`/`download_queue`/etc. See `lanrurugi_storage::refresh_tokens` module docs.
    pub refresh_tokens: Arc<RefreshTokenRepository>,
    /// First-party API tokens (issue #54), replacing legacy's single fixed `apikey` mechanism —
    /// also on the `config` logical DB. See `lanrurugi_storage::api_tokens` module docs.
    pub api_tokens: Arc<ApiTokenRepository>,
    /// In-memory write-behind throttle for `ApiTokenRepository::touch_last_used` — keyed by
    /// token id, storing the unix-seconds timestamp of the last Redis write for that token.
    /// `middleware::auth::is_authorized` checks this before writing through on every
    /// API-token-authenticated request; without it, a client hammering the API would turn
    /// "record last-used time" into a Redis write on every single request. Mirrors the
    /// "cheap in-memory check gates an expensive path" shape `plugin_options_generation` already
    /// uses above, just keyed instead of a single counter.
    pub api_token_last_touch: Arc<Mutex<HashMap<String, i64>>>,
    /// Operator activity log (issue #87) — a structured, persisted record of who did what,
    /// distinct from the unstructured `tracing`-based request log
    /// (`procedure::trace_request`). Also on the `config` logical DB, same placement as
    /// `api_tokens`/`download_queue`/`compare_cache`. See `lanrurugi_storage::activity` module
    /// docs for the full data model/retention/query design.
    pub activity: Arc<ActivityRepository>,
}

impl AppState {
    /// Thin delegate to [`FilenameLocks::lock`] — kept so `download_manager::ingest`'s existing
    /// `state.lock_filename(filename)` call site didn't need to change when this lock moved into
    /// `lanrurugi-core` to become shared with the watcher path too.
    pub async fn lock_filename(&self, filename: &str) -> FilenameLockGuard {
        self.filename_locks.lock(filename).await
    }
}

/// See [`AppState::thumbnail_singleflight`].
pub type ThumbnailSingleflight =
    lanrurugi_core::singleflight::Singleflight<String, Option<(&'static str, bytes::Bytes)>>;

/// `fetch_page`'s result — `resized` marks the optimized-WebP variant, and `orig_*` carry the
/// original entry's own size/dimensions (surfaced as response headers so the reader's file-info
/// bar can show "current WebP vs original" without a second request or decode).
#[derive(Clone)]
pub struct FetchedPage {
    pub content_type: &'static str,
    pub bytes: bytes::Bytes,
    pub resized: bool,
    pub orig_size: u64,
    pub orig_width: u32,
    pub orig_height: u32,
}

/// See [`AppState::page_singleflight`].
pub type PageSingleflight =
    lanrurugi_core::singleflight::Singleflight<(String, String), Result<FetchedPage, String>>;
