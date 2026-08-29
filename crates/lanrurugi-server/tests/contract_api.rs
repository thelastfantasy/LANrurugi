//! Contract-replay test suite (User Story 3, T055): asserts response *shapes* against the
//! recorded legacy request/response examples embedded in `~/LANraragi/tools/openapi.yaml`'s
//! `examples:`/`example:` blocks — those are real recorded outputs from the legacy system, not
//! invented fixtures, so replaying against them is a direct, objective check of constitution
//! Principle II ("the existing contract must not change") beyond what a general endpoint smoke
//! test would give.
//!
//! Requires a real Redis instance (`LANRURUGI_TEST_REDIS_URL`, bare `redis://host:port`, no DB
//! index — matching every other integration test in this workspace); skips gracefully if unset so
//! this doesn't fail unrelated `cargo test` runs in environments without Redis.

use std::path::PathBuf;
use std::sync::Arc;

use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::entities::Archive;
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::redis::RedisDbs;
use serde_json::Value;
use tower::ServiceExt;

async fn test_app() -> Option<(axum::Router, RedisDbs)> {
    let redis = lanrurugi_storage::test_support::test_redis_dbs().await?;
    seed_guestmode(&redis).await;
    let repos = Repositories::new(&redis);
    let plugin_options = std::sync::Arc::new(
        lanrurugi_storage::plugin_options::PluginOptionsRepository::new(redis.config.clone()),
    );
    let download_queue = std::sync::Arc::new(
        lanrurugi_storage::download_queue::DownloadQueueRepository::new(redis.config.clone()),
    );
    let recommend_cache = std::sync::Arc::new(
        lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(redis.config.clone()),
    );
    let ignored_group_suggestions = std::sync::Arc::new(
        lanrurugi_storage::ignored_group_suggestions::IgnoredGroupSuggestionsRepository::new(
            redis.config.clone(),
        ),
    );
    let compare_cache = std::sync::Arc::new(
        lanrurugi_storage::compare_cache::CompareCacheRepository::new(redis.config.clone()),
    );
    let bookmarks = std::sync::Arc::new(lanrurugi_storage::bookmarks::BookmarksRepository::new(
        redis.config.clone(),
    ));
    let refresh_tokens = std::sync::Arc::new(
        lanrurugi_storage::refresh_tokens::RefreshTokenRepository::new(redis.config.clone()),
    );
    let api_tokens = std::sync::Arc::new(lanrurugi_storage::api_tokens::ApiTokenRepository::new(
        redis.config.clone(),
    ));
    let activity = std::sync::Arc::new(lanrurugi_storage::activity::ActivityRepository::new(
        redis.config.clone(),
    ));
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    let state = AppState {
        redis: redis.clone(),
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            force_secure_cookies: false,
        },
        disable_update_check: true,
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
        plugin_options: plugin_options.clone(),
        plugin_options_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        download_queue: download_queue.clone(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        pending_generate_requests: Default::default(),
        filename_locks: Default::default(),
        download_queue_tx: None,
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
        activity,
        import_snapshots,
    };
    // `MockConnectInfo` — `require_api_key` extracts `ConnectInfo<SocketAddr>` unconditionally (for
    // the API-token last-used-IP field), which is normally supplied by
    // `into_make_service_with_connect_info` at the real `axum::serve` call site; `.oneshot()`-based
    // tests never go through that, so without this layer every request 500s on the missing
    // extension before `enable_pass` is even checked. See axum's own `ConnectInfo` docs for this
    // exact pattern.
    let app = lanrurugi_server::app::build_app(state, None, None).layer(
        axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            0,
        ))),
    );
    Some((app, redis))
}

/// 007-guest-restricted-access made password login unconditional (no more `enable_pass: false`
/// open-instance mode these tests originally relied on), so every request asserting a `200` from
/// a protected route needs a real session — log in with the default password and return the
/// session-cookie header value to attach.
async fn login_cookie(app: &axum::Router) -> String {
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/login")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(axum::body::Body::from("password=kamimamita"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        axum::http::StatusCode::OK,
        "contract tests must be able to log in with the default password"
    );
    let set_cookie = response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v))
        .collect::<Vec<_>>()
        .join("; ");
    assert!(!set_cookie.is_empty(), "login must set a session cookie");
    set_cookie
}

/// `auth::load` hard-errors when `guestmode` is absent from `LRR_CONFIG` — seed the shared
/// default the same way `auth_flow.rs`/`settings_toggles.rs` do, so this file's requests never
/// 500 just because it happened to be the first test binary to run against a fresh Redis.
async fn seed_guestmode(redis: &RedisDbs) {
    use deadpool_redis::redis::AsyncCommands;
    let mut conn = redis.config.get().await.unwrap();
    let _: bool = conn
        .hset_nx(lanrurugi_storage::keys::CONFIG_KEY, "guestmode", "0")
        .await
        .unwrap();
}

async fn get_json(
    app: &axum::Router,
    uri: &str,
    cookie: Option<&str>,
) -> (axum::http::StatusCode, Value) {
    let mut builder = axum::http::Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    let response = app
        .clone()
        .oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// Required fields on `ArchiveMetadataJson`, per `openapi.yaml`'s schema `required:` list —
/// verified directly from the spec file, not assumed.
const ARCHIVE_METADATA_REQUIRED_FIELDS: &[&str] = &[
    "arcid",
    "title",
    "filename",
    "tags",
    "isnew",
    "extension",
    "progress",
    "pagecount",
    "lastreadtime",
    "size",
];

#[tokio::test]
async fn get_archives_matches_recorded_archive_metadata_shape() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };

    let id = "c".repeat(40);
    let repo = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    repo.save(&Archive {
        id: lanrurugi_core::ids::ArchiveId(id.clone()),
        name: "Fate GO MEMO".to_string(),
        title: "Fate GO MEMO".to_string(),
        file: "/nonexistent/fate.zip".to_string(),
        tags: "parody:fate grand order, group:wadamemo, artist:wada rco, artbook, full color"
            .to_string(),
        summary: String::new(),
        arcsize: 1234567,
        pagecount: 34,
        isnew: false,
        lastreadpage: 3,
        lastreadtime: 1337038281,
        thumbhash: None,
        toc: vec![],
        stamp_ids: vec![],
        heal_failed_at: None,
        corrupted_pages: vec![],
        has_patch: false,
    })
    .await
    .unwrap();

    let cookie = login_cookie(&app).await;
    let (status, json) = get_json(&app, "/api/archives", Some(&cookie)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let arr = json.as_array().expect("recorded contract: array response");
    let entry = arr
        .iter()
        .find(|e| e["arcid"] == id)
        .expect("saved archive present in listing");

    for field in ARCHIVE_METADATA_REQUIRED_FIELDS {
        assert!(
            entry.get(field).is_some(),
            "recorded ArchiveMetadataJson contract requires field {field:?}, missing in response: {entry}"
        );
    }
    // Spot-check against the actual recorded example values (openapi.yaml's ArchiveMetadataJson
    // example), confirming types/semantics match, not just key presence.
    assert_eq!(entry["pagecount"], 34);
    assert_eq!(entry["progress"], 3);
    assert_eq!(entry["lastreadtime"], 1337038281);
    assert_eq!(entry["isnew"], false);
    assert_eq!(entry["extension"], "zip");

    repo.delete(&lanrurugi_core::ids::ArchiveId(id))
        .await
        .unwrap();
}

#[tokio::test]
async fn get_info_matches_recorded_serverinfo_shape() {
    let Some((app, _redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };

    let (status, json) = get_json(&app, "/api/info", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);

    // Every field from the recorded ServerInfo example in openapi.yaml must be present.
    for field in [
        "name",
        "motd",
        "has_password",
        "debug_mode",
        "archives_per_page",
        "server_resizes_images",
        "server_tracks_progress",
        "authenticated_progress",
        "total_archives",
        "cache_last_cleared",
    ] {
        assert!(
            json.get(field).is_some(),
            "missing recorded field {field:?}"
        );
    }
    assert!(json["has_password"].is_boolean());
    assert!(json["debug_mode"].is_boolean());
}

#[tokio::test]
async fn delete_archive_matches_recorded_response_shape() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let id = "d".repeat(40);
    let repo = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    repo.save(&Archive {
        id: lanrurugi_core::ids::ArchiveId(id.clone()),
        name: "big_chungus".to_string(),
        title: "big_chungus".to_string(),
        file: "/nonexistent/big_chungus.zip".to_string(),
        tags: String::new(),
        summary: String::new(),
        arcsize: 1,
        pagecount: 1,
        isnew: false,
        lastreadpage: 0,
        lastreadtime: 0,
        thumbhash: None,
        toc: vec![],
        stamp_ids: vec![],
        heal_failed_at: None,
        corrupted_pages: vec![],
        has_patch: false,
    })
    .await
    .unwrap();

    let cookie = login_cookie(&app).await;
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("DELETE")
                .uri(format!("/api/archives/{id}"))
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();

    // Recorded example: {operation: delete_archive, success: 1, id: ..., filename: ...}
    assert_eq!(json["operation"], "delete_archive");
    assert_eq!(json["success"], 1);
    assert_eq!(json["id"], id);
    assert_eq!(json["filename"], "big_chungus");
}

/// T096 regression guard: the Docker image builds the frontend and sets `LANRURUGI_STATIC_DIR`
/// expecting the server to actually serve it — verified missing entirely before this test existed
/// (`build_app` took no `static_dir` parameter at all). Covers the three behaviors that matter:
/// a real asset is served as-is, an unmatched client-side route falls back to `index.html` (SPA
/// pattern), and `/api/*` is never shadowed by the static fallback.
#[tokio::test]
async fn static_frontend_is_served_with_spa_fallback() {
    let Some(redis) = lanrurugi_storage::test_support::test_redis_dbs().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set or unreachable");
        return;
    };
    seed_guestmode(&redis).await;
    let repos = Repositories::new(&redis);
    let plugin_options = std::sync::Arc::new(
        lanrurugi_storage::plugin_options::PluginOptionsRepository::new(redis.config.clone()),
    );
    let download_queue = std::sync::Arc::new(
        lanrurugi_storage::download_queue::DownloadQueueRepository::new(redis.config.clone()),
    );
    let recommend_cache = std::sync::Arc::new(
        lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(redis.config.clone()),
    );
    let ignored_group_suggestions = std::sync::Arc::new(
        lanrurugi_storage::ignored_group_suggestions::IgnoredGroupSuggestionsRepository::new(
            redis.config.clone(),
        ),
    );
    let compare_cache = std::sync::Arc::new(
        lanrurugi_storage::compare_cache::CompareCacheRepository::new(redis.config.clone()),
    );
    let bookmarks = std::sync::Arc::new(lanrurugi_storage::bookmarks::BookmarksRepository::new(
        redis.config.clone(),
    ));
    let refresh_tokens = std::sync::Arc::new(
        lanrurugi_storage::refresh_tokens::RefreshTokenRepository::new(redis.config.clone()),
    );
    let api_tokens = std::sync::Arc::new(lanrurugi_storage::api_tokens::ApiTokenRepository::new(
        redis.config.clone(),
    ));
    let activity = std::sync::Arc::new(lanrurugi_storage::activity::ActivityRepository::new(
        redis.config.clone(),
    ));
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    let state = AppState {
        redis,
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            force_secure_cookies: false,
        },
        disable_update_check: true,
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
        plugin_options: plugin_options.clone(),
        plugin_options_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        download_queue: download_queue.clone(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        pending_generate_requests: Default::default(),
        filename_locks: Default::default(),
        download_queue_tx: None,
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
        activity,
        import_snapshots,
    };

    let static_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        static_dir.path().join("index.html"),
        "<html>spa shell</html>",
    )
    .unwrap();
    std::fs::create_dir_all(static_dir.path().join("assets")).unwrap();
    std::fs::write(
        static_dir.path().join("assets").join("app.js"),
        "console.log('hi')",
    )
    .unwrap();

    let app = lanrurugi_server::app::build_app(state, Some(static_dir.path().to_path_buf()), None)
        .layer(axum::extract::connect_info::MockConnectInfo(
            std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
        ));

    let get = |uri: &'static str| {
        let app = app.clone();
        async move {
            let response = app
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
    };

    let (status, body) = get("/").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("spa shell"));

    let (status, body) = get("/library/some/client-side/route").await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "unmatched routes should fall back to index.html"
    );
    assert!(body.contains("spa shell"));

    let (status, body) = get("/assets/app.js").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("console.log"));

    let (status, _) = get_json(&app, "/api/info", None).await;
    assert_eq!(
        status,
        axum::http::StatusCode::OK,
        "/api routes must not be shadowed by the static fallback"
    );
}

/// `/docs` (the plugin-authoring SDK reference — `docs-builder`'s `deno doc --html` output in the
/// real Docker image) must be served from `docs_dir` itself, not swallowed by the SPA's own
/// catch-all `static_dir` fallback (a `nest`-ed route, matched before that fallback ever runs —
/// see `build_app`'s own docs for why ordering matters here).
#[tokio::test]
async fn docs_dir_is_served_under_docs_and_not_shadowed_by_the_spa_fallback() {
    let Some(redis) = lanrurugi_storage::test_support::test_redis_dbs().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set or unreachable");
        return;
    };
    seed_guestmode(&redis).await;
    let repos = Repositories::new(&redis);
    let plugin_options = std::sync::Arc::new(
        lanrurugi_storage::plugin_options::PluginOptionsRepository::new(redis.config.clone()),
    );
    let download_queue = std::sync::Arc::new(
        lanrurugi_storage::download_queue::DownloadQueueRepository::new(redis.config.clone()),
    );
    let recommend_cache = std::sync::Arc::new(
        lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(redis.config.clone()),
    );
    let ignored_group_suggestions = std::sync::Arc::new(
        lanrurugi_storage::ignored_group_suggestions::IgnoredGroupSuggestionsRepository::new(
            redis.config.clone(),
        ),
    );
    let compare_cache = std::sync::Arc::new(
        lanrurugi_storage::compare_cache::CompareCacheRepository::new(redis.config.clone()),
    );
    let bookmarks = std::sync::Arc::new(lanrurugi_storage::bookmarks::BookmarksRepository::new(
        redis.config.clone(),
    ));
    let refresh_tokens = std::sync::Arc::new(
        lanrurugi_storage::refresh_tokens::RefreshTokenRepository::new(redis.config.clone()),
    );
    let api_tokens = std::sync::Arc::new(lanrurugi_storage::api_tokens::ApiTokenRepository::new(
        redis.config.clone(),
    ));
    let activity = std::sync::Arc::new(lanrurugi_storage::activity::ActivityRepository::new(
        redis.config.clone(),
    ));
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    let state = AppState {
        redis,
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            force_secure_cookies: false,
        },
        disable_update_check: true,
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
        plugin_options: plugin_options.clone(),
        plugin_options_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        download_queue: download_queue.clone(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        pending_generate_requests: Default::default(),
        filename_locks: Default::default(),
        download_queue_tx: None,
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
        activity,
        import_snapshots,
    };

    let static_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        static_dir.path().join("index.html"),
        "<html>spa shell</html>",
    )
    .unwrap();
    let docs_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        docs_dir.path().join("index.html"),
        "<html>plugin sdk docs</html>",
    )
    .unwrap();

    let app = lanrurugi_server::app::build_app(
        state,
        Some(static_dir.path().to_path_buf()),
        Some(docs_dir.path().to_path_buf()),
    )
    .layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from(([127, 0, 0, 1], 0)),
    ));

    let get = |uri: &'static str| {
        let app = app.clone();
        async move {
            let response = app
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
    };

    let (status, body) = get("/docs/").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        body.contains("plugin sdk docs"),
        "/docs must be served from docs_dir, not the SPA's index.html fallback"
    );

    let (status, body) = get("/").await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(body.contains("spa shell"), "/ must still be the SPA shell");
}

/// `settings` is additive (no legacy REST contract) but must read/write the *same* `LRR_CONFIG`
/// hash legacy itself uses, so a migrated instance's already-set `theme` is visible with zero
/// conversion step (Principle I).
#[tokio::test]
async fn settings_defaults_then_roundtrips_through_shared_config_hash() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    // Explicit `hdel` before asserting the default — this shares one real Redis instance with
    // every other test binary in the workspace (`LANRURUGI_TEST_REDIS_URL`, run concurrently by
    // `cargo test`), so a `theme` value another test wrote and didn't clean up in time (a real,
    // observed race with `serve_index.rs`'s own theme-substitution tests) could otherwise leak in
    // and fail this test's own "confirms the true default" assertion for reasons that have nothing
    // to do with what this test is actually checking.
    {
        use deadpool_redis::redis::AsyncCommands;
        let mut conn = redis.config.get().await.unwrap();
        let _: () = conn.hdel("LRR_CONFIG", "theme").await.unwrap();
    }

    let cookie = login_cookie(&app).await;
    let (status, json) = get_json(&app, "/api/settings", Some(&cookie)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        json["theme"], "modern.css",
        "legacy's own default (Config.pm::get_style)"
    );

    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("PUT")
                .uri("/api/settings")
                .header("cookie", &cookie)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(r#"{"theme":"modern_red.css"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let (status, json) = get_json(&app, "/api/settings", Some(&cookie)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["theme"], "modern_red.css");

    // Confirm it landed in the exact hash/field legacy itself reads, not a LANrurugi-private key.
    let mut conn = redis.config.get().await.unwrap();
    use deadpool_redis::redis::AsyncCommands;
    let theme: String = conn.hget("LRR_CONFIG", "theme").await.unwrap();
    assert_eq!(theme, "modern_red.css");
    let _: () = conn.hdel("LRR_CONFIG", "theme").await.unwrap();
}

// The old `nhentai_source_converter_rewrites_short_numeric_source_tags_only` test that used to
// live here asserted against `POST /api/database/scripts/nhentai-source-converter` — a native
// Rust endpoint that no longer exists (`nHentaiSourceConverter.pm` was migrated to a real
// `script`-type plugin, `plugins/script/nhentaisourceconverter.ts`, run through `/plugins/use`
// like every other plugin — see that file's own doc comment). It had been silently 404ing on
// every run for some time (nobody noticed because `LANRURUGI_TEST_REDIS_URL` had never actually
// been wired into the containerized test flow, so it — like every other Redis-gated test here —
// was reported as `ok` while actually just skipping). Replaced by a real end-to-end test that
// calls the actual plugin through the real Deno dispatcher:
// `lanrurugi_api::plugins::tests::nhentai_source_converter_rewrites_short_numeric_source_tags_only`.

/// Regression guard: `subfolders_to_categories` (`FolderToCat.pm` port) once generated catids as
/// `SET_<timestamp>_<index>`, which doesn't match `CategoryRepository::list_all`'s
/// `SET_??????????` key-discovery glob (exactly a 10-digit timestamp) — the category was created
/// correctly and directly `GET`-able by id, but invisible to `GET /categories` and everything else
/// that lists categories. This exercises the real discovery path, not just direct lookup.
#[tokio::test]
async fn subfolders_to_categories_creates_a_category_visible_in_list_all() {
    let Some(redis) = lanrurugi_storage::test_support::test_redis_dbs().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set or unreachable");
        return;
    };
    seed_guestmode(&redis).await;
    let repos = Repositories::new(&redis);
    let plugin_options = std::sync::Arc::new(
        lanrurugi_storage::plugin_options::PluginOptionsRepository::new(redis.config.clone()),
    );
    let download_queue = std::sync::Arc::new(
        lanrurugi_storage::download_queue::DownloadQueueRepository::new(redis.config.clone()),
    );
    let recommend_cache = std::sync::Arc::new(
        lanrurugi_storage::recommend_cache::RecommendCacheRepository::new(redis.config.clone()),
    );
    let ignored_group_suggestions = std::sync::Arc::new(
        lanrurugi_storage::ignored_group_suggestions::IgnoredGroupSuggestionsRepository::new(
            redis.config.clone(),
        ),
    );
    let compare_cache = std::sync::Arc::new(
        lanrurugi_storage::compare_cache::CompareCacheRepository::new(redis.config.clone()),
    );
    let bookmarks = std::sync::Arc::new(lanrurugi_storage::bookmarks::BookmarksRepository::new(
        redis.config.clone(),
    ));
    let refresh_tokens = std::sync::Arc::new(
        lanrurugi_storage::refresh_tokens::RefreshTokenRepository::new(redis.config.clone()),
    );
    let api_tokens = std::sync::Arc::new(lanrurugi_storage::api_tokens::ApiTokenRepository::new(
        redis.config.clone(),
    ));
    let activity = std::sync::Arc::new(lanrurugi_storage::activity::ActivityRepository::new(
        redis.config.clone(),
    ));
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );

    let library_dir = tempfile::tempdir().unwrap();
    let subfolder = library_dir.path().join("My Series");
    std::fs::create_dir_all(&subfolder).unwrap();
    let archive_path = subfolder.join("Volume 1.zip");
    std::fs::write(&archive_path, b"fake archive bytes").unwrap();

    let id = "f".repeat(40);
    let archive_repo = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    archive_repo
        .save(&Archive {
            id: lanrurugi_core::ids::ArchiveId(id.clone()),
            name: "Volume 1".to_string(),
            title: "Volume 1".to_string(),
            file: archive_path.to_string_lossy().to_string(),
            tags: String::new(),
            summary: String::new(),
            arcsize: 1,
            pagecount: 1,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: false,
        })
        .await
        .unwrap();

    let state = AppState {
        redis: redis.clone(),
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            force_secure_cookies: false,
        },
        disable_update_check: true,
        library: LibraryPaths {
            archive_dir: library_dir.path().to_path_buf(),
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
        plugin_options: plugin_options.clone(),
        plugin_options_generation: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        download_queue: download_queue.clone(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        pending_generate_requests: Default::default(),
        filename_locks: Default::default(),
        download_queue_tx: None,
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
        activity,
        import_snapshots,
    };
    let app = lanrurugi_server::app::build_app(state, None, None).layer(
        axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            0,
        ))),
    );

    let cookie = login_cookie(&app).await;
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/database/scripts/subfolders-to-categories")
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    let (status, categories) = get_json(&app, "/api/categories", Some(&cookie)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let categories = categories.as_array().unwrap();
    let created = categories
        .iter()
        .find(|c| c["name"] == "My Series")
        .expect("'My Series' category must appear in the category list, not just be creatable");
    assert_eq!(created["archives"].as_array().unwrap().len(), 1);

    let category_repo =
        lanrurugi_storage::repository::CategoryRepository::new(redis.archive.clone());
    category_repo
        .delete(&lanrurugi_core::ids::CategoryId(
            created["id"].as_str().unwrap().to_string(),
        ))
        .await
        .unwrap();
    archive_repo
        .delete(&lanrurugi_core::ids::ArchiveId(id))
        .await
        .unwrap();
}
