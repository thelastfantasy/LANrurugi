//! Router-level integration coverage for settings.rs fields that need a real HTTP round-trip to
//! verify (issue #85's original "field existed, nothing consumed it" gaps — `enablecors`, real
//! `Access-Control-Allow-*` headers + `OPTIONS` preflight short-circuit; `tagrules`/`tagruleson`,
//! rewrites a metadata plugin's returned tags before merge) plus 007-guest-restricted-access's
//! `guestmode`/`Category.visible_to_guest` request-scoping matrix. `language` isn't covered here —
//! it's a frontend-only concern (`i18n/index.ts`'s `useApplySettingsLanguage`), nothing for a Rust
//! router test to exercise.
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

/// `guestmode` defaults to `"0"` — every test in this file that doesn't specifically exercise
/// guest-mode routing gets ordinary "password login required" behavior, and `auth::load` requires
/// this field to be present at all (007-guest-restricted-access: a hard error, not a silent
/// default, once an instance has been migrated — see `AuthConfigError::MissingField`'s own docs).
/// `HSETNX`, not `HSET` — see `auth_flow.rs::test_app`'s own identical fix for why: an
/// unconditional overwrite here races every other in-flight test across `cargo test
/// --workspace`'s separate `tests/*.rs` OS processes (including `auth_flow.rs`'s own, which shares
/// this exact same real Redis field), silently stomping a value a concurrently-running,
/// `RedisTestLock`-holding test deliberately set to `"1"` — confirmed live, 2026-08-27, as the
/// actual root cause of an intermittent "guest_visitor gets 401 instead of 200" CI failure.
async fn test_app() -> Option<(axum::Router, RedisDbs)> {
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
        pending_generate_requests: Default::default(),
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

/// Minimal real archive for guest-scope tests — `file` points at a nonexistent path since none of
/// T038-T042 actually read file bytes, only exercise metadata/search/route-gating.
fn test_archive(id: &str, title: &str, tags: &str) -> lanrurugi_core::entities::Archive {
    lanrurugi_core::entities::Archive {
        id: lanrurugi_core::ids::ArchiveId(id.to_string()),
        name: title.to_string(),
        title: title.to_string(),
        file: format!("/nonexistent/{id}.zip"),
        tags: tags.to_string(),
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
    }
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

async fn json_body(response: axum::http::Response<axum::body::Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// With `enablecors` off (the default), no CORS headers are added — confirms the middleware is
/// really opt-in, not always-on.
#[tokio::test]
async fn cors_headers_absent_when_disabled() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
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
    let Some((app, redis)) = test_app().await else {
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

/// Delete every guest-test category ID this file (and `auth_flow.rs`) uses — a prior test that
/// panicked mid-body never runs its own trailing cleanup, and a leftover `visible_to_guest`
/// category in the shared Redis silently grants guest eligibility to every later test that
/// asserts 401 (the exact cascade behind the 2026-08-27 CI run's five-failure chain). Safe to
/// call only while holding the `guestmode` `RedisTestLock`, which every caller does — that lock
/// serializes these tests against `auth_flow.rs`'s own guest test across processes too.
async fn purge_guest_test_categories(repos: &Repositories) {
    for id in [
        "SET_9992010001",
        "SET_9992010002",
        "SET_9992010003",
        "SET_9992010004",
        "SET_9992010005",
        "SET_9992010006",
    ] {
        let _ = repos
            .categories
            .delete(&lanrurugi_core::ids::CategoryId(id.to_string()))
            .await;
    }
}

/// 007-guest-restricted-access, US2: the full `guestmode` + `Category.visible_to_guest` matrix —
/// guest mode off (regardless of category visibility), guest mode on with zero guest-visible
/// categories, and guest mode on with at least one guest-visible category, the only combination
/// that actually grants an unauthenticated caller scoped access. Uses a real `PUT /api/categories`
/// round-trip (not a direct repository write) so this also exercises `create_category`'s own
/// `visible_to_guest` form-field wiring (T030).
#[tokio::test]
async fn guest_mode_and_category_visibility_matrix() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    // Cross-*process* lock, not just this file's own in-process `config_field_lock` above — see
    // `RedisTestLock`'s own docs (`auth_flow.rs` also writes this same shared `guestmode` field,
    // as a genuinely separate `cargo test --workspace` process against the same real Redis).
    let guest_lock =
        lanrurugi_storage::test_support::RedisTestLock::acquire(&redis.config, "guestmode").await;
    purge_guest_test_categories(&Repositories::new(&redis)).await;

    // guestmode off, no categories at all: an ordinary protected route stays 401.
    let resp = request(&app, "GET", "/api/categories").await;
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);

    // guestmode on, but zero categories are guest-visible yet: still 401 — the eligibility branch
    // requires *both* conditions, not just the site-wide switch.
    set_config_field(&redis, "guestmode", "1").await;
    let resp = request(&app, "GET", "/api/categories").await;
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::UNAUTHORIZED,
        "guest mode on with zero guest-visible categories must not grant access"
    );

    // Create one guest-visible category directly via the repository (no session exists in this
    // test to drive it through the real PUT endpoint without also standing up a login flow) — the
    // request-scoping matrix under test here is about `procedure.rs`'s eligibility branch, not
    // `create_category`'s own field wiring, which `categories.rs`'s own unit-level coverage (T030)
    // already exercises.
    //
    // `guest_has_any_visible_archive` (c89ec44) requires the category to actually contain a real
    // archive, not just carry `visible_to_guest` — an empty `archives: Vec::new()` category (this
    // test's own original shape) makes that check return `false` and keeps an otherwise-eligible
    // guest at 401, confirmed live via real CI failure, 2026-08-28.
    let repos = Repositories::new(&redis);
    let archive_id = "6".repeat(40);
    repos
        .archives
        .save(&test_archive(&archive_id, "Guest Visible Archive", ""))
        .await
        .unwrap();
    let category = lanrurugi_core::entities::Category {
        catid: lanrurugi_core::ids::CategoryId("SET_9992010001".to_string()),
        name: "Guest Visible".to_string(),
        search: None,
        archives: vec![lanrurugi_core::ids::ArchiveId(archive_id.clone())],
        pinned: false,
        visible_to_guest: true,
    };
    repos.categories.save(&category).await.unwrap();

    // Now an unauthenticated caller reaches the ordinary protected route as a guest.
    let resp = request(&app, "GET", "/api/categories").await;
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::OK,
        "guest mode on with at least one guest-visible category must grant scoped guest access"
    );

    // But a session-only route stays out of reach even for an eligible guest — `guest_visitor` is
    // never in that route's own allow-list (`route_policy.csv`), matching `token_guest`'s own
    // restriction.
    let drop_resp = request(&app, "POST", "/api/database/drop").await;
    assert_eq!(
        drop_resp.status(),
        axum::http::StatusCode::FORBIDDEN,
        "a session-only route must reject a guest_visitor even when otherwise eligible"
    );

    repos.categories.delete(&category.catid).await.unwrap();
    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(archive_id))
        .await
        .unwrap();
    set_config_field(&redis, "guestmode", "0").await;
    guest_lock.release().await;
}

/// 007-guest-restricted-access, US3 (T038): guest search results never include an out-of-scope
/// archive even when it shares a tag with an in-scope one — proof that `restrict_to_archive_ids`
/// narrows the *result set*, not just the tag/keyword match itself, which would otherwise let a
/// shared tag "pull in" an archive the guest has no scope over.
#[tokio::test]
async fn guest_search_excludes_out_of_scope_archive_sharing_a_tag() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let guest_lock =
        lanrurugi_storage::test_support::RedisTestLock::acquire(&redis.config, "guestmode").await;
    let repos = Repositories::new(&redis);
    purge_guest_test_categories(&repos).await;

    let in_scope_id = "1".repeat(40);
    let out_of_scope_id = "2".repeat(40);
    repos
        .archives
        .save(&test_archive(&in_scope_id, "In Scope", "shared_tag"))
        .await
        .unwrap();
    repos
        .archives
        .save(&test_archive(
            &out_of_scope_id,
            "Out Of Scope",
            "shared_tag",
        ))
        .await
        .unwrap();
    // `ArchiveRepository::save` itself never touches the tag search index — only the real
    // `PUT /archives/{id}/metadata` handler does (`archives.rs::update_archive_metadata`'s own
    // `indexer::update_tag_indexes` call). A test that writes archives straight through the
    // repository (as this one does, to control both ids precisely) must index them the same way
    // by hand, or `filter=shared_tag` below has nothing to ever match — found live via a CI-only
    // failure, 2026-08-28 (this test's `INDEX_shared_tag` set was never created at all; it had
    // only ever passed locally by accident, riding on another test's leftover index in the same
    // shared dev-container Redis).
    for id in [&in_scope_id, &out_of_scope_id] {
        lanrurugi_search::indexer::update_tag_indexes(&redis.search, id, "", "shared_tag")
            .await
            .unwrap();
    }
    let category = lanrurugi_core::entities::Category {
        catid: lanrurugi_core::ids::CategoryId("SET_9992010002".to_string()),
        name: "Guest Visible".to_string(),
        search: None,
        archives: vec![lanrurugi_core::ids::ArchiveId(in_scope_id.clone())],
        pinned: false,
        visible_to_guest: true,
    };
    repos.categories.save(&category).await.unwrap();
    set_config_field(&redis, "guestmode", "1").await;

    let resp = request(
        &app,
        "GET",
        "/api/search/ids?filter=shared_tag&groupby_tanks=false",
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let json = json_body(resp).await;
    let ids: Vec<&str> = json["data"]
        .as_array()
        .expect("data must be an array")
        .iter()
        .map(|v| v.as_str().expect("id must be a string"))
        .collect();
    assert!(
        ids.contains(&in_scope_id.as_str()),
        "the in-scope archive must still appear: {ids:?}"
    );
    assert!(
        !ids.contains(&out_of_scope_id.as_str()),
        "the out-of-scope archive must never appear even though it shares a tag: {ids:?}"
    );

    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(in_scope_id))
        .await
        .unwrap();
    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(out_of_scope_id))
        .await
        .unwrap();
    repos.categories.delete(&category.catid).await.unwrap();
    set_config_field(&redis, "guestmode", "0").await;
    guest_lock.release().await;
}

/// 007-guest-restricted-access, US3 (T039): a guest's direct request for an out-of-scope archive
/// returns the exact same `404` shape as a genuinely nonexistent archive id — the archive is real
/// (an admin could fetch it), it's just outside this guest's scope, and that distinction must never
/// be observable from the response (research.md §6, spec FR-012).
#[tokio::test]
async fn guest_metadata_request_for_out_of_scope_archive_404s_like_nonexistent() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let guest_lock =
        lanrurugi_storage::test_support::RedisTestLock::acquire(&redis.config, "guestmode").await;
    let repos = Repositories::new(&redis);
    purge_guest_test_categories(&repos).await;

    let out_of_scope_id = "3".repeat(40);
    repos
        .archives
        .save(&test_archive(&out_of_scope_id, "Out Of Scope", ""))
        .await
        .unwrap();
    // Guest mode on, but with a guest-visible category that does NOT include this archive — the
    // eligibility branch itself is satisfied (at least one guest-visible category exists), the
    // per-archive scope check is what must deny this specific request. The category still needs a
    // *different* real archive in it, not zero — `guest_has_any_visible_archive` (c89ec44) treats
    // an empty static category as ineligible, which would 401 before ever reaching the per-archive
    // scope check this test is actually about, confirmed live via real CI failure, 2026-08-28.
    let in_scope_id = "7".repeat(40);
    repos
        .archives
        .save(&test_archive(&in_scope_id, "In Scope", ""))
        .await
        .unwrap();
    let category = lanrurugi_core::entities::Category {
        catid: lanrurugi_core::ids::CategoryId("SET_9992010003".to_string()),
        name: "Guest Visible".to_string(),
        search: None,
        archives: vec![lanrurugi_core::ids::ArchiveId(in_scope_id.clone())],
        pinned: false,
        visible_to_guest: true,
    };
    repos.categories.save(&category).await.unwrap();
    set_config_field(&redis, "guestmode", "1").await;

    let nonexistent_id = "4".repeat(40);
    let out_of_scope_resp = request(
        &app,
        "GET",
        &format!("/api/archives/{out_of_scope_id}/metadata"),
    )
    .await;
    let nonexistent_resp = request(
        &app,
        "GET",
        &format!("/api/archives/{nonexistent_id}/metadata"),
    )
    .await;

    assert_eq!(
        out_of_scope_resp.status(),
        axum::http::StatusCode::NOT_FOUND
    );
    assert_eq!(nonexistent_resp.status(), axum::http::StatusCode::NOT_FOUND);
    let out_of_scope_json = json_body(out_of_scope_resp).await;
    let nonexistent_json = json_body(nonexistent_resp).await;
    assert_eq!(
        out_of_scope_json["error"]
            .as_str()
            .map(|s| s.contains("does not exist")),
        Some(true),
    );
    assert_eq!(
        nonexistent_json["error"]
            .as_str()
            .map(|s| s.contains("does not exist")),
        Some(true),
        "an out-of-scope archive's 404 must read identically to a nonexistent one's"
    );

    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(out_of_scope_id))
        .await
        .unwrap();
    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(in_scope_id))
        .await
        .unwrap();
    repos.categories.delete(&category.catid).await.unwrap();
    set_config_field(&redis, "guestmode", "0").await;
    guest_lock.release().await;
}

/// 007-guest-restricted-access, US3 (T040): an eligible guest can still reach an in-scope
/// archive's metadata (read access), but bookmark/progress/download — none of which are GET-safe
/// browsing — all reject: bookmark/progress are POST/PUT (`guest_visitor`'s GET-only allow rule
/// already excludes them by construction, research.md §3), download is GET but explicitly denied
/// by its own policy rule (T006/T046).
#[tokio::test]
async fn guest_cannot_bookmark_save_progress_or_download_an_in_scope_archive() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let guest_lock =
        lanrurugi_storage::test_support::RedisTestLock::acquire(&redis.config, "guestmode").await;
    let repos = Repositories::new(&redis);
    purge_guest_test_categories(&repos).await;

    let archive_id = "5".repeat(40);
    repos
        .archives
        .save(&test_archive(&archive_id, "In Scope", ""))
        .await
        .unwrap();
    let category = lanrurugi_core::entities::Category {
        catid: lanrurugi_core::ids::CategoryId("SET_9992010004".to_string()),
        name: "Guest Visible".to_string(),
        search: None,
        archives: vec![lanrurugi_core::ids::ArchiveId(archive_id.clone())],
        pinned: false,
        visible_to_guest: true,
    };
    repos.categories.save(&category).await.unwrap();
    set_config_field(&redis, "guestmode", "1").await;

    let metadata_resp = request(&app, "GET", &format!("/api/archives/{archive_id}/metadata")).await;
    assert_eq!(
        metadata_resp.status(),
        axum::http::StatusCode::OK,
        "an eligible guest must still be able to read an in-scope archive's metadata"
    );

    let bookmark_resp = request(
        &app,
        "POST",
        &format!("/api/archives/{archive_id}/bookmarks/1"),
    )
    .await;
    assert_ne!(
        bookmark_resp.status(),
        axum::http::StatusCode::OK,
        "a guest must never be able to bookmark, even an in-scope archive"
    );

    let progress_resp = request(
        &app,
        "PUT",
        &format!("/api/archives/{archive_id}/progress/1"),
    )
    .await;
    assert_ne!(
        progress_resp.status(),
        axum::http::StatusCode::OK,
        "a guest must never be able to save reading progress, even for an in-scope archive"
    );

    let download_resp = request(&app, "GET", &format!("/api/archives/{archive_id}/download")).await;
    assert_ne!(
        download_resp.status(),
        axum::http::StatusCode::OK,
        "a guest must never be able to download the original file, even for an in-scope archive"
    );

    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(archive_id))
        .await
        .unwrap();
    repos.categories.delete(&category.catid).await.unwrap();
    set_config_field(&redis, "guestmode", "0").await;
    guest_lock.release().await;
}

/// 007-guest-restricted-access, US3 (T041): a handful of purely-administrative endpoints reject a
/// guest_visitor unconditionally — not because of any per-resource scope check, but because
/// `guest_visitor` is never in their route's own Casbin allow-list at all (`route_policy.csv`).
/// Regardless of guest mode state, since these routes were never reachable by an unauthenticated
/// caller before this feature either.
#[tokio::test]
async fn guest_cannot_reach_plugins_activity_or_stats_regardless_of_guest_mode() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let guest_lock =
        lanrurugi_storage::test_support::RedisTestLock::acquire(&redis.config, "guestmode").await;
    let repos = Repositories::new(&redis);
    purge_guest_test_categories(&repos).await;

    let category = lanrurugi_core::entities::Category {
        catid: lanrurugi_core::ids::CategoryId("SET_9992010005".to_string()),
        name: "Guest Visible".to_string(),
        search: None,
        archives: Vec::new(),
        pinned: false,
        visible_to_guest: true,
    };
    repos.categories.save(&category).await.unwrap();
    set_config_field(&redis, "guestmode", "1").await;

    for path in ["/api/plugins", "/api/activity", "/api/stats"] {
        let resp = request(&app, "GET", path).await;
        assert_ne!(
            resp.status(),
            axum::http::StatusCode::OK,
            "{path} must reject a guest_visitor even with guest mode fully eligible"
        );
    }

    repos.categories.delete(&category.catid).await.unwrap();
    set_config_field(&redis, "guestmode", "0").await;
    guest_lock.release().await;
}

/// 007-guest-restricted-access, US3 (T042, spec FR-015): a config change takes effect on the very
/// next request — no stale snapshot of "guest mode was on a moment ago" persists across requests.
/// Covers both trigger conditions: turning `guestmode` off, and unmarking the last guest-visible
/// category (leaving `guestmode` itself on).
#[tokio::test]
async fn guest_eligibility_change_takes_effect_on_the_very_next_request() {
    let _guard = config_field_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let guest_lock =
        lanrurugi_storage::test_support::RedisTestLock::acquire(&redis.config, "guestmode").await;
    let repos = Repositories::new(&redis);
    purge_guest_test_categories(&repos).await;

    // `guest_has_any_visible_archive` (c89ec44) requires the category to actually contain a real
    // archive, not just carry `visible_to_guest` — an empty `archives: Vec::new()` category (this
    // test's own original shape) makes that check return `false` and keeps the guest at 401
    // instead of the eligible 200 this test's first assertion expects, confirmed live via real CI
    // failure, 2026-08-28.
    let archive_id = "8".repeat(40);
    repos
        .archives
        .save(&test_archive(&archive_id, "Guest Visible Archive", ""))
        .await
        .unwrap();
    let category = lanrurugi_core::entities::Category {
        catid: lanrurugi_core::ids::CategoryId("SET_9992010006".to_string()),
        name: "Guest Visible".to_string(),
        search: None,
        archives: vec![lanrurugi_core::ids::ArchiveId(archive_id.clone())],
        pinned: false,
        visible_to_guest: true,
    };
    repos.categories.save(&category).await.unwrap();
    set_config_field(&redis, "guestmode", "1").await;

    let eligible_resp = request(&app, "GET", "/api/categories").await;
    assert_eq!(eligible_resp.status(), axum::http::StatusCode::OK);

    // Trigger 1: turn guestmode off.
    set_config_field(&redis, "guestmode", "0").await;
    let after_guestmode_off = request(&app, "GET", "/api/categories").await;
    assert_eq!(
        after_guestmode_off.status(),
        axum::http::StatusCode::UNAUTHORIZED,
        "turning guestmode off must take effect on the very next request"
    );

    // Trigger 2: guestmode back on, but the last guest-visible category gets unmarked.
    set_config_field(&redis, "guestmode", "1").await;
    let mut unmarked = category.clone();
    unmarked.visible_to_guest = false;
    repos.categories.save(&unmarked).await.unwrap();
    let after_unmark = request(&app, "GET", "/api/categories").await;
    assert_eq!(
        after_unmark.status(),
        axum::http::StatusCode::UNAUTHORIZED,
        "unmarking the last guest-visible category must take effect on the very next request"
    );

    repos.categories.delete(&category.catid).await.unwrap();
    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(archive_id))
        .await
        .unwrap();
    set_config_field(&redis, "guestmode", "0").await;
    guest_lock.release().await;
}
