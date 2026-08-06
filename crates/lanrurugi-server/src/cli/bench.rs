//! `lanrurugi bench` (T090): the single-command US8 workflow `quickstart.md` §8 describes —
//! generates a synthetic library at the requested scale, then boots this binary's own Axum app
//! in-process (on an ephemeral loopback port, against a scratch Redis URL) to serve as the "new"
//! system side of the comparison automatically.
//!
//! Standing up the *legacy* Perl system is deliberately **not** automated here — that's a whole
//! separate application stack (Docker image, its own bundled Redis, etc.) with no `cargo`-level
//! way to drive it, and inventing container-orchestration code inside this binary would be scope
//! creep beyond what US8 actually asks for. The operator points `--legacy-url` at an
//! already-running legacy instance (see `quickstart.md` §8) — matching how the standalone
//! `lanrurugi-bench-compare` binary already works, just with the "new" side automated away here
//! for convenience.

use std::path::PathBuf;
use std::sync::Arc;

use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_bench::compare::{run_full_comparison, CompareConfig, SystemEndpoint};
use lanrurugi_bench::synthetic_library::{generate, GenerateConfig};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::bootstrap::bootstrap;
use lanrurugi_storage::redis::RedisDbs;

pub struct BenchArgs {
    pub legacy_url: String,
    pub legacy_api_key: Option<String>,
    /// Must point at an empty/scratch Redis instance — the same expectation `rebuild-index`
    /// already places on `--redis-url` (this benchmark starts from a fresh library, not an
    /// existing one).
    pub redis_url: String,
    pub library_dir: PathBuf,
    pub library_scale: usize,
    pub pages_per_archive: usize,
    pub hardware_description: String,
    pub title_needle: String,
    pub interactive_load_iterations: usize,
}

pub async fn run(args: BenchArgs) -> anyhow::Result<()> {
    tracing::info!(scale = args.library_scale, dir = %args.library_dir.display(), "Generating synthetic library...");
    std::fs::create_dir_all(&args.library_dir)?;
    let gen_summary = generate(&GenerateConfig {
        output_dir: args.library_dir.clone(),
        archive_count: args.library_scale,
        pages_per_archive: args.pages_per_archive,
    })?;

    let thumb_dir = args.library_dir.join(".lanrurugi-bench-thumb");
    let temp_dir = args.library_dir.join(".lanrurugi-bench-temp");
    let plugins_dir = args.library_dir.join(".lanrurugi-bench-plugins");
    std::fs::create_dir_all(&thumb_dir)?;
    std::fs::create_dir_all(&temp_dir)?;
    std::fs::create_dir_all(&plugins_dir)?;

    let dispatcher_path = temp_dir.join("dispatcher.ts");
    std::fs::write(&dispatcher_path, lanrurugi_plugin::DISPATCHER_SCRIPT)?;
    let plugins = Arc::new(PluginPool::new(
        "deno".to_string(),
        dispatcher_path,
        plugins_dir.clone(),
    ));

    let redis = RedisDbs::connect(&args.redis_url)?;
    bootstrap(&redis, &args.library_dir).await?;
    let repos = Repositories::new(&redis);
    // No watcher started (Principle III's file-watching mode is irrelevant here): ingestion for
    // both the "full scan" and "reindex" comparison operations is driven entirely through
    // `POST /database/rebuild-index`, exactly like `measure_new_rebuild_index` expects.
    let scanner = ScannerHandle::new();
    let plugin_options = Arc::new(
        lanrurugi_storage::plugin_options::PluginOptionsRepository::new(redis.config.clone()),
    );
    let download_queue = Arc::new(
        lanrurugi_storage::download_queue::DownloadQueueRepository::new(redis.config.clone()),
    );
    let recommend_cache = Arc::new(
        lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(redis.config.clone()),
    );

    // Same long-lived "自动运行" auto-plugin consumer `serve`'s own `main.rs` wires up — kept
    // consistent here too (rather than a no-op sender) since `rebuild_index`'s handler (what this
    // bench tool drives its own scans through, per the comment above) always hands it a clone of
    // `state.new_archive_tx`, and legacy's own real deployment also runs auto-plugins during
    // ingestion, so silently skipping that here would make this comparison less representative,
    // not more.
    let (new_archive_tx, mut new_archive_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let state = AppState {
        redis,
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            api_key: String::new(),
            enable_pass: false,
        },
        library: LibraryPaths {
            archive_dir: args.library_dir.clone(),
            thumb_dir,
            temp_dir,
            log_dir: None,
        },
        scanner,
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
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx,
        download_cancellations: Default::default(),
        filename_locks: Default::default(),
    };

    {
        let state = state.clone();
        tokio::spawn(async move {
            while let Some(id) = new_archive_rx.recv().await {
                lanrurugi_api::plugins::run_enabled_metadata_plugins_on_archive(&state, &id).await;
            }
        });
    }

    let app = crate::app::build_app(state, None, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let new_addr = listener.local_addr()?;
    tracing::info!(%new_addr, "bench: new-system instance listening");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(error = %e, "bench: new-system server exited unexpectedly");
        }
    });

    let config = CompareConfig {
        legacy: SystemEndpoint {
            base_url: args.legacy_url,
            api_key: args.legacy_api_key,
        },
        new: SystemEndpoint {
            base_url: format!("http://{new_addr}"),
            api_key: None,
        },
        archive_count: gen_summary.archive_count as u64,
        total_size_bytes: gen_summary.total_size_bytes,
        hardware_description: args.hardware_description,
        poll_interval: std::time::Duration::from_millis(250),
        operation_timeout: std::time::Duration::from_secs(1800),
        title_needle: args.title_needle,
        interactive_load_iterations: args.interactive_load_iterations,
    };

    let report = run_full_comparison(&config).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
