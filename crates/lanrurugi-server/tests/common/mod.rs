//! The AppState harness the 017 sharing tests build on (copied from `settings_toggles.rs`).
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::keys::CONFIG_KEY;
use lanrurugi_storage::redis::RedisDbs;

pub async fn test_state() -> Option<(AppState, RedisDbs)> {
    let redis = lanrurugi_storage::test_support::test_redis_dbs().await?;
    let _: bool = redis
        .config
        .get()
        .await
        .unwrap()
        .hset_nx(CONFIG_KEY, "guestmode", "0")
        .await
        .unwrap();
    let repos = Repositories::new(&redis);
    let plugin_options = Arc::new(
        lanrurugi_storage::plugin_options::PluginOptionsRepository::new(redis.config.clone()),
    );
    let download_queue = Arc::new(
        lanrurugi_storage::download_queue::DownloadQueueRepository::new(redis.config.clone()),
    );
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
    let activity_dedup = Arc::new(lanrurugi_storage::activity_dedup::ActivityDedupGate::new(
        redis.config.clone(),
    ));
    let import_snapshots = Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    let state = AppState {
        subscriptions: Arc::new(
            lanrurugi_storage::subscriptions::SubscriptionRepository::new(redis.config.clone()),
        ),
        subscriptions_in_flight: Default::default(),
        equivalence: std::sync::Arc::new(lanrurugi_search::Equivalence::default()),
        discovery_singleflight: std::sync::Arc::new(
            lanrurugi_core::singleflight::Singleflight::new(8),
        ),
        redis: redis.clone(),
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            force_secure_cookies: false,
        },
        disable_update_check: true,
        disable_first_run_wizard: false,
        library: LibraryPaths {
            archive_dir: PathBuf::from("/tmp"),
            thumb_dir: PathBuf::from("/tmp"),
            temp_dir: PathBuf::from("/tmp"),
            log_dir: None,
        },
        scanner: ScannerHandle::new(),
        plugins: Arc::new(PluginPool::new(
            "deno",
            PathBuf::from("/tmp/dispatcher.ts"),
            PathBuf::from("/tmp/plugins"),
        )),
        plugins_dir: PathBuf::from("/tmp/plugins"),
        download_managers: Default::default(),
        thumbnail_singleflight: Arc::new(lanrurugi_core::singleflight::Singleflight::new(1)),
        page_singleflight: Arc::new(lanrurugi_core::singleflight::Singleflight::new(1)),
        plugin_options,
        plugin_options_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        download_queue,
        recommend_cache,
        ignored_group_suggestions,
        compare_cache,
        bookmarks,
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new(
            std::env::temp_dir().join("lanrurugi-test-models"),
        )),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        pending_generate_requests: Default::default(),
        split_progress_tx: Default::default(),
        translation_runtime: Default::default(),
        translation_scheduler: Default::default(),
        translation_telemetry: Default::default(),
        filename_locks: Default::default(),
        download_queue_tx: None,
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
        activity,
        activity_dedup,
        import_snapshots,
    };

    Some((state, redis))
}

/// The full router over `state`, with a loopback peer address for `ConnectInfo`.
pub fn app(state: AppState) -> axum::Router {
    lanrurugi_server::app::build_app(state, None, None).layer(
        axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            0,
        ))),
    )
}
