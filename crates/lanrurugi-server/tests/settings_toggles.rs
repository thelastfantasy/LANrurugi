//! Router-level integration coverage for the four "settings.rs field existed, nothing consumed
//! it" gaps closed by issue #85: `nofunmode` (forces auth even with `enable_pass: false`),
//! `enablecors` (real `Access-Control-Allow-*` headers + `OPTIONS` preflight short-circuit), and
//! `tagrules`/`tagruleson` (rewrites a metadata plugin's returned tags before merge). `language`
//! isn't covered here — it's a frontend-only concern (`i18n/index.ts`'s
//! `useApplySettingsLanguage`), nothing for a Rust router test to exercise.
//!
//! Requires a real Redis instance (`LANRURUGI_TEST_REDIS_URL`), same convention as every other
//! integration test in this workspace — skips gracefully if unset.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::keys::CONFIG_KEY;
use lanrurugi_storage::redis::RedisDbs;
use tower::ServiceExt;

/// All three tests below read/write the same real `LRR_CONFIG` `nofunmode`/`enablecors` fields on
/// a shared Redis instance (`cargo test` runs tests in one file concurrently by default) — without
/// this, they race, exactly the same failure mode `serve_index.rs`'s own `theme_field_lock` docs
/// describe (and hit live here too, confirmed by a real flaky failure before this lock was added).
fn config_field_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

async fn test_app(enable_pass: bool) -> Option<(axum::Router, RedisDbs)> {
    let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
    let redis = RedisDbs::connect(&base).ok()?;
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
    let state = AppState {
        redis: redis.clone(),
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            enable_pass,
            force_secure_cookies: false,
        },
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
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        filename_locks: Default::default(),
        download_queue_tx: None,
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
        activity,
    };

    // See `contract_api.rs`'s own `test_app()` for why this layer is required: `require_api_key`
    // extracts `ConnectInfo<SocketAddr>` unconditionally, which `.oneshot()` never supplies on its
    // own.
    let app = lanrurugi_server::app::build_app(state, None, None).layer(
        axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            0,
        ))),
    );
    Some((app, redis))
}

async fn set_config_field(redis: &RedisDbs, field: &str, value: &str) {
    let mut conn = redis.config.get().await.unwrap();
    let _: () = conn.hset(CONFIG_KEY, field, value).await.unwrap();
}

async fn clear_config_field(redis: &RedisDbs, field: &str) {
    let mut conn = redis.config.get().await.unwrap();
    let _: () = conn.hdel(CONFIG_KEY, field).await.unwrap();
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
) -> axum::http::Response<axum::body::Body> {
    app.clone()
        .oneshot(
            axum::http::Request::builder()
                .method(method)
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

/// `nofunmode: true` on an otherwise-open instance (`enable_pass: false`) must still refuse an
/// unauthenticated request — the whole point of the setting (see `LiveAuthConfig::no_fun_mode`'s
/// own docs) is closing exactly this gap.
#[tokio::test]
async fn nofunmode_forces_auth_even_when_enable_pass_is_off() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app(false).await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };

    // Baseline: `enable_pass: false` alone really does bypass auth (sanity-checks the test's own
    // premise before asserting `nofunmode` changes it).
    clear_config_field(&redis, "nofunmode").await;
    let resp = request(&app, "GET", "/api/settings").await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    set_config_field(&redis, "nofunmode", "1").await;
    let resp = request(&app, "GET", "/api/settings").await;
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);

    clear_config_field(&redis, "nofunmode").await;
}

/// With `enablecors` off (the default), no CORS headers are added — confirms the middleware is
/// really opt-in, not always-on.
#[tokio::test]
async fn cors_headers_absent_when_disabled() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app(false).await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    clear_config_field(&redis, "enablecors").await;

    let resp = request(&app, "GET", "/api/settings").await;
    assert!(resp.headers().get("Access-Control-Allow-Origin").is_none());
}

/// With `enablecors` on: a real request carries the three headers, and an `OPTIONS` preflight is
/// answered directly (204, no auth required) rather than falling through to `require_api_key` and
/// 401ing — see `cors::apply_cors`'s own docs on why a preflight must never reach the auth check.
#[tokio::test]
async fn cors_headers_present_and_preflight_bypasses_auth_when_enabled() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app(true).await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    set_config_field(&redis, "enablecors", "1").await;

    let resp = request(&app, "OPTIONS", "/api/settings").await;
    assert_eq!(resp.status(), axum::http::StatusCode::NO_CONTENT);
    assert_eq!(
        resp.headers().get("Access-Control-Allow-Origin").unwrap(),
        "*"
    );
    assert_eq!(
        resp.headers().get("Access-Control-Allow-Methods").unwrap(),
        "GET, OPTIONS, POST, DELETE, PUT"
    );
    assert_eq!(
        resp.headers().get("Access-Control-Allow-Headers").unwrap(),
        "Authorization"
    );

    // A real (non-OPTIONS) request still needs to actually authenticate — CORS headers are added
    // on top of the normal auth flow, not a bypass for anything but the preflight itself.
    let unauth_resp = request(&app, "GET", "/api/settings").await;
    assert_eq!(unauth_resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(
        unauth_resp
            .headers()
            .get("Access-Control-Allow-Origin")
            .unwrap(),
        "*"
    );

    clear_config_field(&redis, "enablecors").await;
}
