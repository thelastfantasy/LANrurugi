//! Covers `app::build_app`'s SPA-fallback `serve_index` handler (issue #58 follow-up): the
//! server-side `data-theme` attribute substitution that lets even a brand-new browser/device get
//! the correct theme applied before first paint, without waiting on a client-side
//! `/settings`/`/theme` round-trip first. Requires a real Redis instance
//! (`LANRURUGI_TEST_REDIS_URL`), same convention as every other integration test in this
//! workspace — skips gracefully if unset.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::redis::RedisDbs;
use tower::ServiceExt;

const CONFIG_KEY: &str = lanrurugi_storage::keys::CONFIG_KEY;

/// `cargo test` runs tests in this file concurrently by default, but both tests below read/write
/// the same real `LRR_CONFIG` `theme` field on a shared Redis instance (`RedisDbs::connect`'s own
/// `config` pool has no per-test-run isolation — see that struct's own docs, one fixed logical DB
/// index) — without this, they race and whichever writes/deletes the field second wins for both,
/// confirmed live (both orderings of the race reproduced as a real, flaky test failure before this
/// lock was added). A plain `tokio::sync::Mutex` held for a test's entire body serializes them.
fn theme_field_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Builds a real `build_app` router with `static_dir` pointed at a fresh temp directory containing
/// a minimal `index.html` carrying the same `id="theme-init" data-theme=""` attribute shape the
/// real `apps/frontend/index.html` does — enough for `serve_index` to exercise its actual
/// substitution logic without depending on a real frontend build being present in the test
/// environment.
async fn test_app_with_static_dir() -> Option<(axum::Router, RedisDbs, tempfile::TempDir)> {
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
    let state = AppState {
        redis: redis.clone(),
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            api_key: String::new(),
            enable_pass: false,
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
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        filename_locks: Default::default(),
    };

    let dir = tempfile::tempdir().ok()?;
    // `id="theme-init" data-theme=""` — the exact attribute shape `serve_index`'s own replace
    // anchors on. An earlier version of this fixture (and `serve_index`'s own substitution logic)
    // used a bare `__SERVER_THEME__` JS-string-literal placeholder instead, which a documentation
    // comment elsewhere in the real `index.html` happened to also spell out verbatim — a
    // first-match string replace silently rewrote the comment instead of the real assignment. An
    // HTML attribute value has no such ambiguity: nothing else in a well-formed document plausibly
    // contains the literal substring `data-theme=""`.
    tokio::fs::write(
        dir.path().join("index.html"),
        r#"<html><head><script id="theme-init" data-theme="">var theme = document.currentScript.dataset.theme</script></head></html>"#,
    )
    .await
    .ok()?;

    let app = lanrurugi_server::app::build_app(state, Some(dir.path().to_path_buf()), None);
    Some((app, redis, dir))
}

async fn get_body(app: &axum::Router, uri: &str) -> (axum::http::StatusCode, String) {
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
async fn serve_index_substitutes_the_real_theme_from_redis() {
    let _guard = theme_field_lock().lock().await;
    let Some((app, redis, _dir)) = test_app_with_static_dir().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let mut conn = redis.config.get().await.unwrap();
    let _: () = conn.hset(CONFIG_KEY, "theme", "ex.css").await.unwrap();

    // An unmatched SPA route (not a real static asset) — must fall through `ServeDir`'s own 404
    // to `serve_index`, exactly like a bookmarked client-side route (`/library`) would.
    let (status, body) = get_body(&app, "/some/client/route").await;

    // Cleaned up before any assertion — a real Redis instance is shared with *every* other test in
    // this workspace (not just this file), and an early `assert!` panic must not skip this and
    // leave `theme` = "ex.css" behind to leak into an unrelated test elsewhere (confirmed live:
    // this exact leak once failed `contract_api.rs`'s own
    // `settings_defaults_then_roundtrips_through_shared_config_hash`, which asserts the *default*
    // theme is "modern.css").
    let _: () = conn.hdel(CONFIG_KEY, "theme").await.unwrap();

    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.contains(r#"data-theme="ex.css""#),
        "expected the real theme substituted into the attribute, got: {body}"
    );
}

#[tokio::test]
async fn serve_index_falls_back_to_the_placeholder_when_theme_was_never_set() {
    let _guard = theme_field_lock().lock().await;
    let Some((app, redis, _dir)) = test_app_with_static_dir().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    // Explicit `hdel`, not just "no `hset` here" — this suite shares one real Redis instance with
    // every other test in it (and, if `LANRURUGI_TEST_REDIS_URL` points at a persistent instance
    // rather than a fresh container, potentially other test binaries too), so a `theme` value
    // another test already wrote (e.g. this file's own `..._from_redis` test) could otherwise
    // leak into this one depending on execution order.
    let mut conn = redis.config.get().await.unwrap();
    let _: () = conn.hdel(CONFIG_KEY, "theme").await.unwrap();

    // `fetch_theme` still succeeds (Redis is reachable), just returns the hardcoded "modern.css"
    // default (settings.rs's own `unwrap_or_else`) once the field is confirmed absent, so that's
    // what should land in the substituted attribute.
    let (status, body) = get_body(&app, "/").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.contains(r#"data-theme="modern.css""#),
        "expected the hardcoded default theme substituted when none was ever configured, got: {body}"
    );
}

/// A `theme` value outside `settings::KNOWN_THEME_FILES` — reachable via `PUT /settings`, which
/// never validates this field — must never reach the response verbatim, since `serve_index`
/// substitutes it directly into an HTML attribute value (issue #58 follow-up). Proven here with a
/// value that would break out of the attribute (and inject a new one) if it *did* survive
/// substitution unescaped.
#[tokio::test]
async fn serve_index_ignores_a_theme_value_outside_the_known_whitelist() {
    let _guard = theme_field_lock().lock().await;
    let Some((app, redis, _dir)) = test_app_with_static_dir().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let mut conn = redis.config.get().await.unwrap();
    let payload = r#""><script>alert(1)</script>"#;
    let _: () = conn.hset(CONFIG_KEY, "theme", payload).await.unwrap();

    let (status, body) = get_body(&app, "/").await;

    // Cleaned up before any assertion — see the sibling test's own comment on why an early
    // `assert!` panic must not skip this and leak a `theme` value into an unrelated test.
    let _: () = conn.hdel(CONFIG_KEY, "theme").await.unwrap();

    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        !body.contains("<script>alert(1)"),
        "an unrecognized theme value must never be substituted verbatim, got: {body}"
    );
    // Left with the attribute still empty, not actively rewritten to "modern.css" — `serve_index`
    // treats "the stored value isn't trustworthy" the same as "Redis was unreachable": leave the
    // file's own client-side script (which reads the empty attribute as "unset" and falls through
    // to its own `localStorage`-then-`modern.css` chain — see `index.html`) to sort it out, rather
    // than the server silently second-guessing what the *right* fallback theme should be.
    assert!(
        body.contains(r#"data-theme="""#),
        "an unrecognized theme value must leave the attribute empty for the client script's own fallback chain to handle, got: {body}"
    );
}
