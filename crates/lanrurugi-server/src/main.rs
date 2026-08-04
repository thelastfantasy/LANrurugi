use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
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

    /// Raw (non-base64) API key. Empty disables key-based auth entirely if `--no-pass` is also
    /// set; otherwise API requests without a matching key are rejected once a key is configured.
    #[arg(long, env = "LANRURUGI_API_KEY", default_value = "")]
    api_key: String,

    /// Disables password/API-key protection entirely (matches legacy `enable_pass = 0`).
    #[arg(long, env = "LANRURUGI_NO_PASS", default_value_t = false)]
    no_pass: bool,

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
    let plugins = Arc::new(PluginPool::new(
        args.deno_bin,
        dispatcher_path,
        plugins_dir.clone(),
    ));

    let redis = RedisDbs::connect(&args.redis_url)?;
    bootstrap(&redis, &args.library_path).await?;

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

    let state = AppState {
        redis: redis.clone(),
        repos: repos.clone(),
        jobs: jobs.clone(),
        auth: AuthConfig {
            api_key: args.api_key,
            enable_pass: !args.no_pass,
        },
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
        recommender: recommender.clone(),
        new_archive_tx: new_archive_tx.clone(),
        download_cancellations: Default::default(),
        filename_locks: filename_locks.clone(),
    };

    // A queue item left `Starting`/`Downloading` when the process last exited has no chance of
    // ever completing on its own — `JobRegistry` (the in-process progress tracker its `job_id`
    // pointed at) is purely in-memory and was just recreated empty above, so that job is gone for
    // good, but the persisted queue item's own `state` survived unchanged in Redis. Without this,
    // such an item is stuck showing "Starting…" forever (`useJobs()` never finds its `job_id`
    // again) with no retry affordance. Run synchronously here, before `build_app`/accepting
    // connections, rather than as a spawned background job — it's a handful of Redis round trips
    // at most, and every item should already be in its corrected state by the time the frontend's
    // first poll lands.
    match state.download_queue.list_all().await {
        Ok(items) => {
            for mut item in items {
                if matches!(
                    item.state,
                    lanrurugi_storage::download_queue::DownloadQueueState::Starting
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
                }
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to list download queue for restart-orphan cleanup");
        }
    }

    {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(id) = new_archive_rx.recv().await {
                lanrurugi_api::plugins::run_enabled_metadata_plugins_on_archive(&state, &id).await;
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
            // The first tick fires immediately (`tokio::time::interval`'s own documented
            // behavior) — deliberate, not skipped: a conflict staged just before a restart
            // shouldn't have to wait a full hour for the first sweep to even notice it's stale.
            loop {
                interval.tick().await;
                lanrurugi_api::download_manager::ingest::sweep_stale_pending_renames(&state).await;
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
    axum::serve(listener, app).await?;
    Ok(())
}

/// `lanrurugi rebuild-index` (User Story 6, T077): runs the same two-step repair
/// (`lanrurugi_storage::rebuild::rekey_all` + `lanrurugi_scanner::full_scan::full_scan`) as
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

    Ok(())
}
