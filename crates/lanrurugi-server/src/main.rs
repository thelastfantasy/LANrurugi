use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_server::{app, telemetry};
use lanrurugi_storage::bootstrap::bootstrap;
use lanrurugi_storage::redis::RedisDbs;

/// Default `redis_url` for every subcommand that takes one — the standard local Redis default
/// port, matching legacy's own out-of-the-box deployment assumption.
const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379";

/// Bumped whenever the one-time reader-recommendation-cache backfill (issue #70) needs to run
/// again for every existing instance — e.g. a future change to what gets embedded/cached that
/// isn't itself a precision-tier change (which already has its own trigger in
/// `settings::put_settings`). A pre-existing library's archives never re-trigger
/// `recommend_precompute::precompute_one` on their own (nothing about them changes after this
/// feature ships), so without this backfill they'd simply never get a cached recommendation entry.
const CURRENT_RECOMMEND_BACKFILL_VERSION: u32 = 1;

/// LANrurugi: a Rust + React rewrite of LANraragi. One binary, three modes (constitution
/// Principle III) — no separate watcher/worker processes.
#[derive(Parser)]
#[command(name = "lanrurugi", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the Axum server (HTTP API + static frontend + in-process scanner/plugin pool).
    Serve(ServeArgs),
    /// Recompute archive IDs with the size-aware algorithm, splitting any historically
    /// false-merged pairs (User Story 6).
    RebuildIndex(RebuildIndexArgs),
    /// Run the concurrency/throughput benchmark against a synthetic library (User Story 8 — not
    /// yet implemented).
    Bench(BenchArgs),
}

#[derive(clap::Args)]
struct ServeArgs {
    /// Bare `redis://host:port` (no database index — LANrurugi selects across the five legacy
    /// logical databases itself, see `lanrurugi_storage::redis`).
    #[arg(
        long,
        env = "LANRURUGI_REDIS_URL",
        default_value = DEFAULT_REDIS_URL
    )]
    redis_url: String,

    /// Path to the archive library folder, matching the legacy deployment's existing folder
    /// (Principle I — no destructive migration).
    #[arg(long, env = "LANRURUGI_LIBRARY_PATH", default_value = "./library")]
    library_path: PathBuf,

    /// Thumbnail cache directory. Matches legacy's `thumbdir` config default of `./thumb`
    /// (verified: `Model/Config.pm::get_thumbdir`).
    #[arg(long, env = "LANRURUGI_THUMB_DIR", default_value = "./thumb")]
    thumb_dir: PathBuf,

    /// Staging directory for uploads and other transient files.
    #[arg(long, env = "LANRURUGI_TEMP_DIR", default_value = "./temp")]
    temp_dir: PathBuf,

    /// Directory of installed plugin `.ts` files, one per namespace (constitution Principle IV).
    #[arg(long, env = "LANRURUGI_PLUGINS_DIR", default_value = "./plugins")]
    plugins_dir: PathBuf,

    /// Deno binary to invoke for plugin subprocesses. Defaults to resolving `deno` on `PATH`.
    #[arg(long, env = "LANRURUGI_DENO_BIN", default_value = "deno")]
    deno_bin: String,

    /// Address to bind the HTTP server to.
    #[arg(long, env = "LANRURUGI_BIND", default_value = "0.0.0.0:3000")]
    bind: String,

    /// Suppresses the frontend's GitHub-releases update check (007-guest-restricted-access —
    /// replaces the removed `devmode` Settings-page toggle, which had zero server-side behavior of
    /// its own; this is the one real effect it controlled). A deploy-time flag rather than a
    /// runtime setting since it's an operational concern, not something a guest/security-adjacent
    /// setting should expose to whoever can reach the Settings page.
    #[arg(long, env = "LANRURUGI_DISABLE_UPDATE_CHECK", default_value_t = false)]
    disable_update_check: bool,

    /// Appends `; Secure` to both auth cookies (JWT access token + refresh token) — only enable
    /// this once actually deployed behind HTTPS (a browser silently drops a `Secure` cookie over
    /// plain HTTP, breaking login entirely for local dev if left on by default). Not inferred
    /// from `X-Forwarded-Proto`: this app has no trusted-proxy allowlist to validate that header
    /// against, so trusting it here would let a request forge its way past a security-relevant
    /// cookie flag.
    #[arg(long, env = "LANRURUGI_FORCE_SECURE_COOKIES", default_value_t = false)]
    force_secure_cookies: bool,

    /// Disables the file watcher on startup (it can still be started later via `/shinobu/restart`).
    #[arg(long, env = "LANRURUGI_NO_WATCH", default_value_t = false)]
    no_watch: bool,

    /// Built frontend directory (`frontend/dist`) to serve as static assets + SPA shell. Unset by
    /// default for local dev (where Vite's own dev server handles the frontend instead); the
    /// Docker image sets this to the frontend it bundled at build time.
    #[arg(long, env = "LANRURUGI_STATIC_DIR")]
    static_dir: Option<PathBuf>,

    /// Pre-generated plugin-authoring SDK reference (`deno doc --html` output) to serve under
    /// `/docs`. Unset by default for local dev (run `mise run plugin-sdk-docs` and point this at
    /// `target/plugin-sdk-docs` to try it locally); the Docker image sets this to the copy it
    /// built fresh at image-build time.
    #[arg(long, env = "LANRURUGI_DOCS_DIR")]
    docs_dir: Option<PathBuf>,

    /// Directory for the categorized log files the Settings → Logs page reads (matches legacy's
    /// own default location, `Utils/Logging.pm::get_logdir`: a `log/` folder next to the app).
    #[arg(long, env = "LANRURUGI_LOG_DIRECTORY", default_value = "./log")]
    log_dir: PathBuf,
}

#[derive(clap::Args)]
struct RebuildIndexArgs {
    #[arg(
        long,
        env = "LANRURUGI_REDIS_URL",
        default_value = DEFAULT_REDIS_URL
    )]
    redis_url: String,

    #[arg(long, env = "LANRURUGI_LIBRARY_PATH", default_value = "./library")]
    library_path: PathBuf,

    #[arg(long, env = "LANRURUGI_THUMB_DIR", default_value = "./thumb")]
    thumb_dir: PathBuf,
}

#[derive(clap::Args)]
struct BenchArgs {
    /// Base URL of an already-running legacy LANraragi instance to compare against (see
    /// `quickstart.md` §8 — standing up the legacy system itself is out of scope for this CLI).
    #[arg(long)]
    legacy_url: String,

    #[arg(long)]
    legacy_api_key: Option<String>,

    /// Must point at an empty/scratch Redis instance, matching `rebuild-index`'s own
    /// `--redis-url` expectation.
    #[arg(
        long,
        env = "LANRURUGI_BENCH_REDIS_URL",
        default_value = DEFAULT_REDIS_URL
    )]
    redis_url: String,

    #[arg(long, default_value = "./bench-library")]
    library_dir: PathBuf,

    /// SC-008's target scale; pass a smaller value for a fast local run.
    #[arg(long, default_value_t = 100_000)]
    library_scale: usize,

    #[arg(long, default_value_t = 20)]
    pages_per_archive: usize,

    #[arg(long, default_value = "unspecified")]
    hardware_description: String,

    #[arg(long, default_value = "Synthetic")]
    title_needle: String,

    #[arg(long, default_value_t = 20)]
    interactive_load_iterations: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::Serve(args) => telemetry::init(Some(&args.log_dir)),
        _ => telemetry::init(None),
    }

    match cli.command {
        Command::Serve(args) => serve(args).await,
        Command::RebuildIndex(args) => rebuild_index(args).await,
        Command::Bench(args) => {
            lanrurugi_server::cli::bench::run(lanrurugi_server::cli::bench::BenchArgs {
                legacy_url: args.legacy_url,
                legacy_api_key: args.legacy_api_key,
                redis_url: args.redis_url,
                library_dir: args.library_dir,
                library_scale: args.library_scale,
                pages_per_archive: args.pages_per_archive,
                hardware_description: args.hardware_description,
                title_needle: args.title_needle,
                interactive_load_iterations: args.interactive_load_iterations,
            })
            .await
        }
    }
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    std::fs::create_dir_all(&args.library_path)?;
    std::fs::create_dir_all(&args.thumb_dir)?;
    std::fs::create_dir_all(&args.temp_dir)?;
    std::fs::create_dir_all(&args.plugins_dir)?;

    // `dispatcher.ts` (`crates/lanrurugi-plugin/dispatcher/dispatcher.ts`) builds a `file://` URL
    // by string-concatenating this path directly (`import(\`file://${pluginsDir}/${namespace}.ts\`)`)
    // — a relative path there produces an invalid URL Deno's own `import()` rejects outright
    // (`file://./plugins/login/nhentai.ts` — confirmed live: every download/login-type plugin
    // failed to load, silently, whenever this arg was left at its own relative default `./plugins`
    // instead of being overridden with an absolute path, as every real deployment/dev-container
    // path already does — see `--plugins-dir`'s own CLI doc comment). Canonicalizing once here,
    // right after the directory is guaranteed to exist, fixes it at the source for every caller
    // (including the Rust side's own `plugins_dir.join(...)` in `lanrurugi-plugin::pool::Worker`)
    // rather than requiring every invocation of this binary to remember to pass an absolute path.
    let plugins_dir = std::fs::canonicalize(&args.plugins_dir)?;

    let dispatcher_path = args.temp_dir.join("dispatcher.ts");
    std::fs::write(&dispatcher_path, lanrurugi_plugin::DISPATCHER_SCRIPT)?;
    std::fs::write(
        args.temp_dir.join("plugin-sdk.ts"),
        lanrurugi_plugin::PLUGIN_SDK_SCRIPT,
    )?;
    let plugins = Arc::new(PluginPool::new(
        args.deno_bin,
        dispatcher_path,
        plugins_dir.clone(),
    ));

    let redis = RedisDbs::connect(&args.redis_url)?;
    bootstrap(&redis, &args.library_path).await?;

    // Auto-persist DEEPSEEK_API_KEY to Redis: if the env var is set but the config hash's
    // `llm_api_key` is empty/absent, write it now so the Settings page always shows the
    // (password-masked) value rather than an empty field (the user's constraint: "检查到环境
    // 变量的key就自动写入redis"). A user can still clear it and save to overwrite the env
    // var copy — this is a one-time seed, not an every-startup enforced sync.
    {
        let mut conn = redis.config.get().await?;
        let stored: Option<String> = conn
            .hget(lanrurugi_storage::keys::CONFIG_KEY, "llm_api_key")
            .await
            .ok()
            .flatten();
        if stored.as_ref().map(|s| s.trim().is_empty()).unwrap_or(true) {
            if let Ok(env_key) = std::env::var("DEEPSEEK_API_KEY") {
                if !env_key.trim().is_empty() {
                    let _: () = conn
                        .hset(lanrurugi_storage::keys::CONFIG_KEY, "llm_api_key", &env_key)
                        .await?;
                    tracing::info!("llm_api_key auto-persisted from DEEPSEEK_API_KEY env var");
                }
            }
        }
    }

    let repos = Repositories::new(&redis);
    let scanner = ScannerHandle::new();
    let jobs = JobRegistry::new();
    // Shared with `scanner.start(...)` below (same `Arc`, not two independent locks) so the
    // download-ingest path (`AppState::lock_filename`) and the watcher's own consumer loop
    // (`pipeline::run`) are mutually exclusive over the same filename — see
    // `lanrurugi_core::filename_lock`'s own docs for the corruption bug this fixes.
    let filename_locks = lanrurugi_core::filename_lock::FilenameLocks::new();

    let plugin_options = Arc::new(
        lanrurugi_storage::plugin_options::PluginOptionsRepository::new(redis.config.clone()),
    );
    let download_queue = Arc::new(
        lanrurugi_storage::download_queue::DownloadQueueRepository::new(redis.config.clone()),
    );
    match download_queue.backfill_created_at().await {
        Ok(0) => {}
        Ok(n) => tracing::info!(
            count = n,
            "download queue: backfilled created_at for stale items"
        ),
        Err(e) => tracing::warn!(error = %e, "download queue: created_at backfill skipped"),
    }
    // Issue #67: same "run it every startup, cheap no-op once already backfilled" pattern as
    // `download_queue.backfill_created_at()` right above — a deploy that already had
    // Categories/Tankoubons before this reverse index existed would otherwise stay permanently
    // unindexed until someone thought to run a manual `rebuild-index`. `backfill_reverse_indexes`
    // itself is safe to call unconditionally on every boot (each `save()` diffs against whatever's
    // already indexed and only writes what's missing — see that function's own docs).
    if let Err(e) =
        lanrurugi_storage::rebuild::backfill_reverse_indexes(&repos.categories, &repos.groupings)
            .await
    {
        tracing::warn!(error = %e, "archive-to-category/tankoubon reverse-index backfill skipped");
    }
    let recommend_cache = Arc::new(
        lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(redis.config.clone()),
    );
    let ignored_group_suggestions = Arc::new(
        lanrurugi_storage::ignored_group_suggestions::IgnoredGroupSuggestionsRepository::new(
            redis.config.clone(),
        ),
    );
    let compare_cache = Arc::new(
        lanrurugi_storage::compare_cache::CompareCacheRepository::new(redis.config.clone()),
    );
    let bookmarks = Arc::new(lanrurugi_storage::bookmarks::BookmarksRepository::new(
        redis.config.clone(),
    ));
    let refresh_tokens = Arc::new(
        lanrurugi_storage::refresh_tokens::RefreshTokenRepository::new(redis.config.clone()),
    );
    let api_tokens = Arc::new(lanrurugi_storage::api_tokens::ApiTokenRepository::new(
        redis.config.clone(),
    ));
    let activity = Arc::new(lanrurugi_storage::activity::ActivityRepository::new(
        redis.config.clone(),
    ));
    let import_snapshots = Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    // Same "run it every startup, cheap no-op once already backfilled" pattern as
    // `backfill_reverse_indexes` above — `outcome` didn't always exist on `ActivityEntry`, so an
    // instance with pre-existing activity history has entries `append` never indexed by outcome at
    // write time (see `backfill_outcome_index`'s own docs for the live-confirmed symptom: filtering
    // by outcome returned zero results despite unfiltered `GET /activity` showing plenty of
    // entries).
    match activity.backfill_outcome_index().await {
        Ok(n) => tracing::info!(count = n, "activity: backfilled outcome index"),
        Err(e) => tracing::warn!(error = %e, "activity: outcome-index backfill skipped"),
    }

    // Constructed *before* the watcher/startup-scan below (which used to run first) so both can
    // be given a live `AppState` clone — needed to run every "自动运行"/enabled metadata plugin on
    // each newly-discovered archive, matching legacy's own `Shinobu.pm::add_new_file`, which does
    // exactly this right after cataloguing a genuinely new file (verified against source; the
    // watcher's own *rekey* path, `update_filemap_entry`, deliberately does not — see
    // `pipeline::run`'s own docs). This mechanism was entirely missing from this port until now.
    // The channel's sender lives on `AppState` itself (`new_archive_tx`) so every other call site
    // that starts/restarts the watcher or a full scan (`shinobu.rs`, `database.rs::rebuild_index`)
    // can hand this same long-lived consumer a clone, instead of each spawning its own.
    let (new_archive_tx, mut new_archive_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    // Reader recommendation engine: the model downloads in the background (ETag-cached, see
    // `lanrurugi_recommend::model_download`) and `install_embedder` flips the service ready once
    // loaded; until then the recommendations endpoint returns 503 `model_not_ready`. The models
    // dir sits next to the thumb dir (`<lanrurugi>/models` — both compose files mount
    // `./data/models` there).
    let recommender = Arc::new(lanrurugi_api::recommend::RecommendService::new());
    {
        let recommender = recommender.clone();
        let models_dir = args
            .thumb_dir
            .parent()
            .unwrap_or(&args.thumb_dir)
            .join("models");
        tokio::spawn(async move {
            match lanrurugi_recommend::model_download::acquire_models(&models_dir).await {
                Ok((model_path, tokenizer_path)) => {
                    match lanrurugi_recommend::embedding::Embedder::load(
                        &model_path,
                        &tokenizer_path,
                        lanrurugi_api::recommend_precompute::precompute_worker_budget(),
                    ) {
                        Ok(embedder) => {
                            recommender.install_embedder(embedder);
                            tracing::info!("recommendation model ready");
                        }
                        Err(e) => {
                            tracing::warn!(error = %e, "failed to load recommendation model — recommendations disabled")
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to acquire recommendation model — recommendations disabled")
                }
            }
        });
    }

    // Eager, not lazy-on-first-request — a malformed checked-in policy/model file (see
    // `authz::route_enforcer`'s own docs on why that `expect` is the right call there) should
    // fail server boot outright, not the first real request that happens to hit `require_session`.
    lanrurugi_api::authz::Authz::get().await;

    let state = AppState {
        redis: redis.clone(),
        repos: repos.clone(),
        jobs: jobs.clone(),
        auth: AuthConfig {
            force_secure_cookies: args.force_secure_cookies,
        },
        disable_update_check: args.disable_update_check,
        library: LibraryPaths {
            archive_dir: args.library_path.clone(),
            thumb_dir: args.thumb_dir.clone(),
            temp_dir: args.temp_dir,
            log_dir: Some(args.log_dir),
        },
        scanner: scanner.clone(),
        plugins,
        plugins_dir,
        download_managers: Default::default(),
        thumbnail_singleflight: Arc::new(lanrurugi_core::singleflight::Singleflight::new(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
        )),
        page_singleflight: Arc::new(lanrurugi_core::singleflight::Singleflight::new(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
        )),
        plugin_options,
        plugin_options_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        download_queue,
        recommend_cache,
        ignored_group_suggestions,
        compare_cache,
        bookmarks,
        recommender: recommender.clone(),
        new_archive_tx: new_archive_tx.clone(),
        download_cancellations: Default::default(),
        pending_generate_requests: Default::default(),
        filename_locks: filename_locks.clone(),
        download_queue_tx: Some(tokio::sync::broadcast::channel(64).0),
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
        activity,
        import_snapshots,
    };

    // A queue item left `Starting`/`Downloading` when the process last exited has no chance of
    // ever completing on its own — `JobRegistry` (the in-process progress tracker its `job_id`
    // pointed at) is purely in-memory and was just recreated empty above, so that job is gone for
    // good, but the persisted queue item's own `state` survived unchanged in Redis. Without this,
    // such an item is stuck showing "Starting…" forever (`useJobs()` never finds its `job_id`
    // again) with no retry affordance.
    //
    // Same treatment for a `pending_filename_conflict` whose staged `temp_path` no longer exists —
    // `temp_dir` isn't a persistent volume, so a full container recreate (not just a process
    // restart) wipes it, but the queue item's `pending_filename_conflict` field survived unchanged
    // in Redis just like `state` does above. Left uncorrected, the frontend keeps offering
    // Overwrite/Rename/AI-Compare for bytes that are already gone, and every one of those actions
    // fails with a confusing "file not found" instead of a clear "please retry the download".
    // Distinct from `sweep_stale_pending_renames` (spawned further below on an hourly timer): that
    // sweep is age-based (`PENDING_RENAME_MAX_AGE`, 24h) and only reclaims files that are still
    // *present but old*, so it never notices a file that's simply *missing outright* — exactly
    // this restart/recreate case. Run synchronously here, before `build_app`/accepting connections,
    // rather than as a spawned background job — it's a handful of Redis round trips and stat calls
    // at most, and every item should already be in its corrected state by the time the frontend's
    // first poll lands.
    match state.download_queue.list_all().await {
        Ok(items) => {
            for mut item in items {
                if matches!(
                    item.state,
                    lanrurugi_storage::download_queue::DownloadQueueState::Starting
                        | lanrurugi_storage::download_queue::DownloadQueueState::Waiting
                        | lanrurugi_storage::download_queue::DownloadQueueState::Downloading
                ) {
                    item.state = lanrurugi_storage::download_queue::DownloadQueueState::Error;
                    item.error = Some(lanrurugi_core::queue_error::QueueError::StaleAfterRestart);
                    if let Err(e) = state.download_queue.update(&item).await {
                        tracing::warn!(
                            item_id = %item.id,
                            error = %e,
                            "failed to mark a restart-orphaned download queue item as errored"
                        );
                    }
                    continue;
                }
                if let Some(conflict) = &item.pending_filename_conflict {
                    let temp_path_exists = tokio::fs::try_exists(&conflict.temp_path)
                        .await
                        .unwrap_or(false);
                    if !temp_path_exists {
                        tracing::info!(
                            item_id = %item.id,
                            temp_path = %conflict.temp_path,
                            "clearing pending filename conflict whose staged file vanished across a restart"
                        );
                        item.pending_filename_conflict = None;
                        item.state = lanrurugi_storage::download_queue::DownloadQueueState::Error;
                        item.error =
                            Some(lanrurugi_core::queue_error::QueueError::StaleAfterRestart);
                        if let Err(e) = state.download_queue.update(&item).await {
                            tracing::warn!(
                                item_id = %item.id,
                                error = %e,
                                "failed to clear a restart-orphaned pending filename conflict"
                            );
                        }
                        // Redis (unlike `temp_dir`) survives a container recreate, so a cached AI
                        // comparison result for this same item — its sample-page images read live
                        // from the very `temp_path` that just vanished — would otherwise keep
                        // being served as a "successful" cached comparison whose images 404.
                        if let Err(e) = state.compare_cache.delete(&item.id).await {
                            tracing::warn!(
                                item_id = %item.id,
                                error = %e,
                                "failed to clear compare_cache for a restart-orphaned pending filename conflict"
                            );
                        }
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to list download queue for restart-orphan cleanup");
        }
    }

    // Resolves any `archives.rs::rename_archive` left mid-flight by a crash — before the generic
    // zombie repair below, since this one knows exactly which old/new path pair was in progress
    // (no guessing from `archive.name`, which may itself not have been saved yet).
    resolve_pending_renames(&state).await;

    // Same "converge any crash-intermediate state before accepting connections" reasoning as the
    // queue-orphan cleanup above — archive records whose `file` is empty/points at temp/vanished
    // get repaired or dropped so hash-based dedup stops matching ghost bytes.
    repair_zombie_archives(&state).await;

    // One-time reader-recommendation-cache backfill (issue #70) — a pre-existing library's
    // archives never trigger `precompute_one` on their own (nothing about them changes after
    // this feature ships), so without this they'd never get a cached recommendation entry at
    // all. Polls `recommender.ready()` rather than awaiting the model-load task directly (that
    // task is fire-and-forget above, with no handle to join) — cheap (a bool read behind a
    // `Mutex`), and the model load itself only happens once at startup, so a short poll interval
    // costs nothing. Guarded the same way a precision-tier change is (`settings::put_settings`):
    // skip if a rebuild is already queued/active, so a slow model load racing a user's own tier
    // change during the same startup window doesn't double-queue a rebuild. Only advances
    // `backfill_version` once the job actually finishes successfully — an interrupted rebuild
    // (process restarted mid-backfill) is retried on the next startup rather than silently
    // skipped, since `spawn_full_precompute_job`'s own generation-tagged resumability means a
    // retry is cheap (already-current entries are skipped).
    {
        let state = state.clone();
        tokio::spawn(async move {
            loop {
                if state.recommender.ready() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            let current_version = state
                .recommend_cache
                .get_backfill_version()
                .await
                .unwrap_or(0);
            if current_version >= CURRENT_RECOMMEND_BACKFILL_VERSION {
                return;
            }
            let already_running =
                state
                    .jobs
                    .by_name("recommend_precompute")
                    .await
                    .iter()
                    .any(|j| {
                        matches!(
                            j.state,
                            lanrurugi_core::jobs::JobState::Queued
                                | lanrurugi_core::jobs::JobState::Active
                        )
                    });
            if already_running {
                return;
            }
            let job_id = lanrurugi_api::recommend_precompute::spawn_full_precompute_job(
                &state,
                "first-time recommendation cache backfill",
            )
            .await;
            // Poll the job's own status (no completion channel exists on `JobRegistry`) so the
            // version marker is only written once the rebuild actually finished — see this
            // block's own doc comment above for why a partial/interrupted run must not advance it.
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let Some(job) = state.jobs.get(&job_id).await else {
                    break;
                };
                match job.state {
                    lanrurugi_core::jobs::JobState::Finished => {
                        if let Err(e) = state
                            .recommend_cache
                            .put_backfill_version(CURRENT_RECOMMEND_BACKFILL_VERSION)
                            .await
                        {
                            tracing::warn!(error = %e, "failed to persist recommend-cache backfill version");
                        }
                        break;
                    }
                    lanrurugi_core::jobs::JobState::Failed => break,
                    _ => continue,
                }
            }
        });
    }

    {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(id) = new_archive_rx.recv().await {
                let label = state
                    .repos
                    .archives
                    .get(&lanrurugi_core::ids::ArchiveId(id.clone()))
                    .await
                    .ok()
                    .flatten()
                    .map(|a| a.title);
                let target = lanrurugi_storage::activity::ActivityTarget {
                    id: Some(id.clone()),
                    label: label.clone(),
                    kind: Some("archive".to_string()),
                };
                let scanner_entry_id = lanrurugi_api::activity::record_automatic(
                    &state,
                    "scanner",
                    lanrurugi_storage::activity::action_types::SCANNER_INGEST,
                    target.clone(),
                    lanrurugi_storage::activity::Outcome::Success,
                    None,
                )
                .await;

                // Recommendation-cache precompute for the final, plugin-enriched title happens
                // inside this call itself (once, after every enabled plugin has run) — see its
                // own doc comment.
                lanrurugi_api::plugins::run_enabled_metadata_plugins_on_archive(&state, &id).await;

                lanrurugi_api::activity::record_automatic(
                    &state,
                    "metadata_plugin",
                    lanrurugi_storage::activity::action_types::METADATA_PLUGIN_AUTORUN,
                    target,
                    lanrurugi_storage::activity::Outcome::Success,
                    Some(lanrurugi_storage::activity::CausedBy {
                        reason: "scanner_ingest".to_string(),
                        source_entry_id: scanner_entry_id,
                        description: "Ran automatically after scanner ingestion".to_string(),
                    }),
                )
                .await;
            }
        });
    }

    // Every 10 minutes — detect queue items still `Downloading`/`Starting` whose job is gone
    // (e.g. Redis timed out during the state-update right after ingest failed — see
    // `update_queue_item_state`'s own retry logic). The startup-time check above catches items
    // orphaned by a restart; this sweep catches items orphaned by a transient Redis blip during
    // the current uptime.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(10 * 60));
            loop {
                interval.tick().await;
                sweep_stale_queue_items(&state).await;
            }
        });
    }

    // Hourly, not once every `PENDING_RENAME_MAX_AGE` itself — a coarser interval would let a
    // conflict sit stale for up to another full 24h past its actual cutoff, depending on how the
    // sweep's own schedule happens to line up against when it was staged.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
            loop {
                interval.tick().await;
                lanrurugi_api::download_manager::ingest::sweep_stale_pending_renames(&state).await;
            }
        });
    }

    // Every 15 minutes — enforce the `tempmaxsize` setting against the reader's WebP resize cache
    // (`temp_dir/resize_page/`), the only unbounded self-regenerating content under `temp_dir`. A
    // finer interval than the pending-rename sweep since this cache can grow continuously under
    // heavy reading traffic, not just accumulate from occasional abandoned conflicts.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15 * 60));
            loop {
                interval.tick().await;
                lanrurugi_api::download_manager::ingest::sweep_resize_cache_size(&state).await;
            }
        });
    }

    if !args.no_watch {
        if let Err(e) = scanner
            .start(
                args.library_path.clone(),
                args.thumb_dir.clone(),
                redis.config.clone(),
                redis.search.clone(),
                (*repos.archives).clone(),
                Some(new_archive_tx.clone()),
                filename_locks.clone(),
            )
            .await
        {
            tracing::warn!(error = %e, "failed to start file watcher");
        }
    }

    // A cold start's `library_path` may already contain files the notify-based watcher above
    // will never see on its own — it only reacts to filesystem events from this point forward,
    // matching legacy's own reactive Shinobu. Legacy *also* does exactly this one-time
    // `update_filemap` walk on boot (`~/LANraragi/lib/Shinobu.pm::initialize_from_new_process`),
    // which this rewrite was missing entirely (previously only reachable via the explicit
    // `rebuild-index` CLI subcommand or `POST /database/rebuild-index` — an admin had to remember
    // to run one of those by hand after every restart with new files already in place, e.g. a
    // freshly-mounted volume). Deliberately just `full_scan`, not `rebuild_index`'s full
    // `rekey_all` + `full_scan` pair — `rekey_all` re-hashes *every already-tracked* archive to
    // detect silent content changes, a Rust-rewrite-only consistency check (User Story 6) with no
    // legacy equivalent at all; legacy's own startup scan never re-validates already-tracked
    // paths, matching `full_scan`'s own cheap path-only pre-filter (see that module's docs) —
    // running `rekey_all` unconditionally on every boot too would reintroduce the exact
    // whole-library-rehash cost this is meant to avoid. Spawned as a background job rather than
    // awaited here, so a large existing library doesn't delay this process from accepting
    // connections.
    {
        let job_id = jobs.create("startup_scan").await;
        let jobs = jobs.clone();
        let repos = repos.clone();
        let redis = redis.clone();
        let library_path = args.library_path.clone();
        let thumb_dir = args.thumb_dir.clone();
        let new_archive_tx = new_archive_tx.clone();
        tokio::spawn(async move {
            jobs.mark_active(&job_id).await;
            let scan_summary = lanrurugi_scanner::full_scan::full_scan(
                &library_path,
                &repos.archives,
                &redis.config,
                &redis.search,
                &thumb_dir,
                &jobs,
                &job_id,
                Some(new_archive_tx),
            )
            .await;
            tracing::info!(
                scanned = scan_summary.scanned,
                already_known = scan_summary.already_known,
                newly_catalogued = scan_summary.catalogued,
                errors = scan_summary.errors,
                "startup scan complete"
            );
            let heal_summary = lanrurugi_scanner::full_scan::heal_pagecounts(&repos.archives).await;
            jobs.finish(
                &job_id,
                serde_json::json!({
                    "scanned": scan_summary.scanned,
                    "already_known": scan_summary.already_known,
                    "newly_catalogued": scan_summary.catalogued,
                    "errors": scan_summary.errors,
                    "pagecount_heal": {
                        "checked": heal_summary.checked,
                        "healed": heal_summary.healed,
                        "failed": heal_summary.failed,
                        "skipped_known_failed": heal_summary.skipped_known_failed,
                        "details": heal_summary.details,
                    },
                }),
            )
            .await;
        });
    }

    let app = app::build_app(state, args.static_dir, args.docs_dir);
    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    tracing::info!(bind = %args.bind, "lanrurugi serve listening");
    // `with_connect_info` — needed so `middleware::auth`'s API-token last-used-IP extraction can
    // fall back to the raw peer address when `X-Forwarded-For` is absent (a direct connection,
    // not behind a reverse proxy). See that extraction helper's own docs on why this is
    // display-only, never a security decision.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;
    Ok(())
}

/// `lanrurugi rebuild-index` (User Story 6, T077): runs the same two-step repair
/// (`lanrurugi_storage::rebuild::rekey_all` + `lanrurugi_scanner::full_scan::full_scan`) as
/// Marks queue items still in `Starting`/`Downloading` whose `job_id` points to nothing alive
/// as `Error(StaleAfterRestart)` — catches items orphaned by a transient Redis failure during
/// the state-update right after ingest (where `update_queue_item_state`'s own retry already
/// covers the common sub-second case; this sweep is the backstop for the rare multi-second blip).
async fn sweep_stale_queue_items(state: &AppState) {
    let items = match state.download_queue.list_all().await {
        Ok(items) => items,
        Err(e) => {
            tracing::warn!(error = %e, "sweep_stale_queue_items: failed to list queue");
            return;
        }
    };
    for mut item in items {
        if !matches!(
            item.state,
            lanrurugi_storage::download_queue::DownloadQueueState::Starting
                | lanrurugi_storage::download_queue::DownloadQueueState::Waiting
                | lanrurugi_storage::download_queue::DownloadQueueState::Downloading
        ) {
            continue;
        }
        // Only mark as stale when the job is genuinely gone: no `job_id` at all, or a tracked job
        // that's already reached a terminal state (`Finished`/`Failed` — `is_terminal()`). NOT
        // simply "isn't `Active` yet" — a freshly-`start_one`'d item is written to Redis as
        // `Starting` synchronously, *before* `plugins::start_download`'s spawned task has gotten
        // around to calling `jobs.mark_active()`, so a real, normal new download sits at
        // `JobState::Queued` (not yet `Active`) for a brief window. The previous "not Active"
        // check treated that completely normal window as orphaned — this sweep runs every 10
        // minutes on a fixed schedule (including once immediately at startup), so on the rare
        // tick that happens to land in that window, a real, healthy, just-started download got
        // killed before it ever got to transfer a single byte, with no progress bar ever having a
        // chance to render and the Stop button already 409-ing since the item was no longer in a
        // stoppable state — confirmed live, 2026-08-25.
        let genuinely_gone = match &item.job_id {
            Some(jid) => state
                .jobs
                .get(jid)
                .await
                .map(|j| j.state.is_terminal())
                .unwrap_or(true),
            None => true,
        };
        if !genuinely_gone {
            continue;
        }
        tracing::info!(
            item_id = %item.id,
            "sweep: marking orphaned queue item as stale (job no longer active)"
        );
        item.state = lanrurugi_storage::download_queue::DownloadQueueState::Error;
        item.error = Some(lanrurugi_core::queue_error::QueueError::StaleAfterRestart);
        if let Err(e) = state.download_queue.update(&item).await {
            tracing::warn!(item_id = %item.id, error = %e, "sweep: failed to update stale item");
        }
    }
}

/// Startup counterpart to `archives.rs::rename_archive`'s `PENDING_RENAME_KEY` journal (see that
/// key's own docs) — finishes or discards whatever rename was mid-flight when the process last
/// stopped, using the exact old/new path pair the journal recorded rather than guessing. Three
/// possible disk states for a given entry, each with an unambiguous resolution:
/// - `new_path` exists, `old_path` doesn't: the disk rename completed but the DB save didn't —
///   apply it now (same field updates `rename_archive` itself would have made).
/// - `old_path` exists, `new_path` doesn't: the crash happened *before* the disk rename ran —
///   nothing to finish; just clear the journal entry.
/// - Neither exists (or the archive record itself is already gone): unrecoverable from this
///   entry alone — clear it and let `repair_zombie_archives` (which runs right after this) apply
///   its own broader "does an `archive_dir` file match this record's `name`" fallback.
async fn resolve_pending_renames(state: &AppState) {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "resolve_pending_renames: failed to reach redis");
            return;
        }
    };
    let entries: std::collections::HashMap<String, String> = conn
        .hgetall(lanrurugi_storage::keys::PENDING_RENAME_KEY)
        .await
        .unwrap_or_default();
    if entries.is_empty() {
        return;
    }

    for (archive_id, paths) in entries {
        let Some((old_path, new_path)) = paths.split_once('\n') else {
            tracing::warn!(%archive_id, "resolve_pending_renames: malformed journal entry, discarding");
            let _: Result<(), _> = conn
                .hdel(lanrurugi_storage::keys::PENDING_RENAME_KEY, &archive_id)
                .await;
            continue;
        };
        let id = lanrurugi_core::ids::ArchiveId(archive_id.clone());
        let new_exists = tokio::fs::try_exists(new_path).await.unwrap_or(false);
        let old_exists = tokio::fs::try_exists(old_path).await.unwrap_or(false);

        if new_exists {
            if let Ok(Some(mut archive)) = state.repos.archives.get(&id).await {
                archive.file = new_path.to_string();
                if let Some(new_name) = std::path::Path::new(new_path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                {
                    archive.name = new_name.to_string();
                }
                if let Err(e) = state.repos.archives.save(&archive).await {
                    tracing::warn!(%archive_id, error = %e, "resolve_pending_renames: failed to save recovered rename, will retry next startup");
                    continue;
                }
                let _: Result<(), _> = conn
                    .hdel(lanrurugi_storage::keys::FILEMAP_KEY, old_path)
                    .await;
                let _: Result<(), _> = conn
                    .hset(
                        lanrurugi_storage::keys::FILEMAP_KEY,
                        new_path,
                        archive_id.as_str(),
                    )
                    .await;
                tracing::info!(%archive_id, old_path, new_path, "resolve_pending_renames: completed a rename interrupted by restart");
            }
        } else if old_exists {
            tracing::info!(%archive_id, old_path, new_path, "resolve_pending_renames: disk rename never ran, discarding stale journal entry");
        } else {
            tracing::warn!(%archive_id, old_path, new_path, "resolve_pending_renames: neither old nor new path exists, deferring to repair_zombie_archives");
        }
        let _: Result<(), _> = conn
            .hdel(lanrurugi_storage::keys::PENDING_RENAME_KEY, &archive_id)
            .await;
    }
}

/// Startup zombie-archive repair: converges every record whose `file` is empty (born under
/// `defer_file_path` and the fixup never ran — crash window), points into `temp_dir` (old-style
/// zombie), or simply doesn't exist on disk anymore. Repair first (an archive-dir file matching
/// the record's `name` exists → point `file` at it); otherwise delete the record + its search
/// index so a hash-based dedup stops matching a record whose bytes are gone. A real on-disk file
/// left behind either way gets re-catalogued by the watcher/startup scan — data isn't lost, the
/// record just gets a fresh, correct one.
async fn repair_zombie_archives(state: &AppState) {
    let archives = match state.repos.archives.list_all().await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "repair_zombie_archives: failed to list archives");
            return;
        }
    };
    let temp_dir = state.library.temp_dir.to_string_lossy().to_string();
    for mut archive in archives {
        let file_exists = !archive.file.is_empty()
            && tokio::fs::try_exists(std::path::Path::new(&archive.file))
                .await
                .unwrap_or(false);
        let stale = archive.file.is_empty()
            || archive.file.starts_with(&temp_dir)
            || archive.file.starts_with("./temp")
            || !file_exists;
        if !stale {
            continue;
        }

        // Repair attempt: the record's `name` is the real destination-facing filename — if a
        // file of exactly that name sits in `archive_dir`, that's almost certainly this
        // archive's own bytes (the crash-after-rename case).
        let candidate = state.library.archive_dir.join(&archive.name);
        if tokio::fs::try_exists(&candidate).await.unwrap_or(false) {
            tracing::info!(
                id = %archive.id,
                file = %candidate.to_string_lossy(),
                "repairing zombie archive: file path now points at archive_dir match"
            );
            let old_file = std::mem::take(&mut archive.file);
            archive.file = candidate.to_string_lossy().to_string();
            if let Err(e) = state.repos.archives.save(&archive).await {
                tracing::warn!(id = %archive.id, error = %e, "failed to save repaired archive file path");
                continue;
            }
            if !old_file.is_empty() {
                if let Ok(mut conn) = state.redis.config.get().await {
                    use deadpool_redis::redis::AsyncCommands;
                    use lanrurugi_storage::keys::FILEMAP_KEY;
                    let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &old_file).await;
                    let _: Result<(), _> = conn
                        .hset(FILEMAP_KEY, &archive.file, archive.id.as_str())
                        .await;
                }
            }
            continue;
        }

        // Not repairable — drop the record and its search index. The bytes (if any survive
        // under a name we can't derive) will be re-catalogued by the watcher/startup scan.
        tracing::info!(
            id = %archive.id,
            name = %archive.name,
            file = %archive.file,
            "deleting zombie archive record whose file is gone"
        );
        if let Err(e) = lanrurugi_search::indexer::remove_archive_index(
            &state.redis.search,
            archive.id.as_str(),
            &archive.title,
            &archive.tags,
        )
        .await
        {
            tracing::warn!(id = %archive.id, error = %e, "failed to remove zombie archive from search index");
        }
        if let Err(e) = state.repos.archives.delete(&archive.id).await {
            tracing::warn!(id = %archive.id, error = %e, "failed to delete zombie archive record");
        }
        if !archive.file.is_empty() {
            if let Ok(mut conn) = state.redis.config.get().await {
                use deadpool_redis::redis::AsyncCommands;
                use lanrurugi_storage::keys::FILEMAP_KEY;
                let _: Result<(), _> = conn.hdel(FILEMAP_KEY, &archive.file).await;
            }
        }
    }
}

/// `POST /database/rebuild-index`, synchronously to completion rather than as a background job —
/// appropriate for a one-shot CLI invocation (e.g. run once after upgrading from legacy, or from
/// a maintenance cron, without needing an HTTP client to poll a job ID).
async fn rebuild_index(args: RebuildIndexArgs) -> anyhow::Result<()> {
    let redis = RedisDbs::connect(&args.redis_url)?;
    let repos = Repositories::new(&redis);
    let jobs = JobRegistry::new();
    let job_id = jobs.create("rebuild_index").await;

    tracing::info!("Recomputing archive IDs...");
    let rekey_summary = lanrurugi_storage::rebuild::rekey_all(
        &repos.archives,
        &repos.categories,
        &repos.groupings,
        &repos.stamps,
        &jobs,
        &job_id,
    )
    .await?;
    tracing::info!(
        rekeyed = rekey_summary.rekeyed.len(),
        unchanged = rekey_summary.unchanged,
        missing_file = rekey_summary.missing_file,
        "Re-key pass complete"
    );

    tracing::info!("Scanning library for previously-invisible files...");
    // `None`: this is the standalone `rebuild-index` CLI subcommand, a one-shot maintenance tool
    // with no `AppState`/Deno plugin pool constructed at all (unlike `serve`'s own startup scan,
    // which has a live one to thread through) — nothing here could run a metadata plugin even if
    // asked to.
    let scan_summary = lanrurugi_scanner::full_scan::full_scan(
        &args.library_path,
        &repos.archives,
        &redis.config,
        &redis.search,
        &args.thumb_dir,
        &jobs,
        &job_id,
        None,
    )
    .await;
    tracing::info!(
        scanned = scan_summary.scanned,
        newly_catalogued = scan_summary.catalogued,
        errors = scan_summary.errors,
        "Full scan complete"
    );

    tracing::info!("Healing pagecount/arcsize for archives left corrupted by past ingest races...");
    let heal_summary = lanrurugi_scanner::full_scan::heal_pagecounts(&repos.archives).await;
    tracing::info!(
        checked = heal_summary.checked,
        healed = heal_summary.healed,
        failed = heal_summary.failed,
        skipped_known_failed = heal_summary.skipped_known_failed,
        "Pagecount heal complete"
    );

    // Issue #67: same unconditional backfill as `POST /database/rebuild-index` — see that
    // handler's own comment on why this doesn't gate behind `rekey_summary.rekeyed` being
    // non-empty.
    tracing::info!("Backfilling archive-to-category/tankoubon reverse indexes...");
    lanrurugi_storage::rebuild::backfill_reverse_indexes(&repos.categories, &repos.groupings)
        .await?;
    tracing::info!("Reverse-index backfill complete");

    Ok(())
}
