//! Router-level integration test for the JWT-access-token + rotating-refresh-token login flow
//! (issue #44/#54): login issues both cookies, a protected endpoint accepts the access token,
//! `/token/refresh` rotates both, the rotated access token also works, and logout revokes the
//! whole refresh-token family so neither cookie works again afterward.
//!
//! Requires a real Redis instance (`LANRURUGI_TEST_REDIS_URL`), same convention as every other
//! integration test in this workspace — skips gracefully if unset.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::redis::RedisDbs;
use tower::ServiceExt;

/// Serializes every test in this file against the shared real Redis instance — both suites here
/// call `purge_all_refresh_and_api_tokens`, which wipes *every* refresh/API-token key regardless
/// of which test wrote it, so running two tests concurrently means one's purge can delete the
/// other's still-in-use tokens mid-flight. Same failure mode (and same fix) as
/// `serve_index.rs::theme_field_lock`/`settings_toggles.rs::config_field_lock` — confirmed live
/// here too once this file grew a second test function.
fn redis_state_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Minimal real archive for guest-scope tests — same shape as `settings_toggles.rs::test_archive`
/// (no shared crate home for it, each integration-test binary is its own compilation unit). `file`
/// points at a nonexistent path since this file's own guest tests never read file bytes, only
/// exercise route-gating.
fn test_archive(id: &str, title: &str) -> lanrurugi_core::entities::Archive {
    lanrurugi_core::entities::Archive {
        id: lanrurugi_core::ids::ArchiveId(id.to_string()),
        name: title.to_string(),
        title: title.to_string(),
        file: format!("/nonexistent/{id}.zip"),
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
    }
}

async fn test_app() -> Option<(axum::Router, RedisDbs)> {
    let redis = lanrurugi_storage::test_support::test_redis_dbs().await?;
    // `auth::load` hard-errors (`AuthConfigError::MissingField`) when `guestmode` is entirely
    // absent from `LRR_CONFIG` (007-guest-restricted-access: a migrated-instance assumption, not
    // a silent default) — every test in this file needs *some* value present even if it isn't
    // specifically exercising guest-mode behavior, matching `settings_toggles.rs::test_app`'s own
    // identical seeding. `HSETNX`, not `HSET` — `test_app()` is called by *every* test in this
    // file (including ones with no `RedisTestLock` of their own, since they never write this field
    // themselves), racing across `cargo test --workspace`'s separate OS processes for
    // `tests/*.rs`. An unconditional `HSET` here would silently stomp a concurrently-running
    // `guest_visitor_reaches_ordinary_routes_but_not_session_only_ones` in *this same file* (or
    // `settings_toggles.rs`'s own guest-mode tests) that's mid-`RedisTestLock` with `guestmode`
    // deliberately set to `"1"` — confirmed live, 2026-08-27, as the actual root cause of a
    // "guest_visitor gets 401 instead of 200" CI failure that survived adding the lock alone:
    // the lock protected the *write*, but not this unconditional default-seed happening
    // concurrently in a completely different, unlocked test run. `HSETNX` only writes when the
    // field doesn't exist yet, so it can never clobber a value some other in-flight test (locked
    // or not) is actively relying on.
    let _: bool = deadpool_redis::redis::AsyncCommands::hset_nx(
        &mut redis.config.get().await.unwrap(),
        lanrurugi_storage::keys::CONFIG_KEY,
        "guestmode",
        "0",
    )
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
    let import_snapshots = Arc::new(
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
        import_snapshots,
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

/// Cleans up every key this suite might have written, keyed by nothing narrower than "any
/// refresh/api token" since the suite always starts from a fresh login — safe because, like every
/// other integration test in this workspace, it shares one real Redis instance and must not leak
/// state into unrelated tests.
async fn purge_all_refresh_and_api_tokens(redis: &RedisDbs) {
    let mut conn = redis.config.get().await.unwrap();
    for pattern in [
        "LANRURUGI_REFRESH_TOKEN_*",
        "LANRURUGI_REFRESH_FAMILY_*",
        "LANRURUGI_API_TOKEN_*",
    ] {
        let keys: Vec<String> = deadpool_redis::redis::cmd("KEYS")
            .arg(pattern)
            .query_async(&mut conn)
            .await
            .unwrap_or_default();
        if !keys.is_empty() {
            let _: () = deadpool_redis::redis::cmd("DEL")
                .arg(keys)
                .query_async(&mut conn)
                .await
                .unwrap();
        }
    }
}

fn set_cookie_values(response: &axum::http::Response<axum::body::Body>) -> Vec<String> {
    response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).to_string())
        .collect()
}

/// The raw `Set-Cookie` strings (full attributes, not just `name=value`) — used to check `Path`
/// scoping specifically, which `set_cookie_values`' own truncation throws away.
fn raw_set_cookie_headers(response: &axum::http::Response<axum::body::Body>) -> Vec<String> {
    response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(|v| v.to_string())
        .collect()
}

fn cookie_header(cookies: &[String]) -> String {
    cookies.join("; ")
}

/// Filters a cookie-value list down to just the access-token cookie — used to simulate what a
/// real browser actually sends to an endpoint the refresh cookie's own narrower `Path` excludes
/// it from (e.g. `/api/logout`), rather than this test's own `cookie_header` helper incidentally
/// forwarding both regardless of Path, which would mask whether `logout`'s fid-from-access-token
/// path (not the refresh cookie) is what's actually doing the work.
fn access_token_cookie_only(cookies: &[String]) -> String {
    cookies
        .iter()
        .find(|c| c.starts_with(lanrurugi_core::session::COOKIE_NAME))
        .cloned()
        .expect("login/refresh must always set the access-token cookie")
}

async fn request(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    form_body: Option<&str>,
) -> axum::http::Response<axum::body::Body> {
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(axum::http::header::COOKIE, cookie);
    }
    let body = if let Some(form_body) = form_body {
        builder = builder.header(
            axum::http::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        );
        axum::body::Body::from(form_body.to_string())
    } else {
        axum::body::Body::empty()
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

/// Same shape as [`request`], but for issue #91's own Casbin-route-policy tests: a `cookie` for
/// Session auth, an optional `bearer` for API-token auth (never both at once in these tests), and
/// a JSON body instead of form-encoded — `create_token`/`change_password` both expect
/// `application/json`, not the login form's `application/x-www-form-urlencoded`.
async fn request_json(
    app: &axum::Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    bearer: Option<&str>,
    json_body: Option<&str>,
) -> axum::http::Response<axum::body::Body> {
    let mut builder = axum::http::Request::builder().method(method).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(axum::http::header::COOKIE, cookie);
    }
    if let Some(bearer) = bearer {
        builder = builder.header(
            axum::http::header::AUTHORIZATION,
            format!("Bearer {bearer}"),
        );
    }
    let body = if let Some(json_body) = json_body {
        builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
        axum::body::Body::from(json_body.to_string())
    } else {
        axum::body::Body::empty()
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

/// The full lifecycle in one test (rather than split across several) because each step's Redis
/// state depends on the previous step's cookies — splitting would just mean re-deriving the same
/// login/refresh chain repeatedly.
#[tokio::test]
async fn login_then_protected_request_then_refresh_then_logout_revokes_everything() {
    let _guard = redis_state_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    purge_all_refresh_and_api_tokens(&redis).await;

    // No credentials at all: the protected endpoint must refuse.
    let resp = request(&app, "GET", "/api/settings", None, None).await;
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);

    // Legacy's own default password (`auth.rs::DEFAULT_PASSWORD_HASH` is its bcrypt hash) — no
    // config override needed for a fresh test Redis instance.
    let login_resp = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some("password=kamimamita"),
    )
    .await;
    assert_eq!(login_resp.status(), axum::http::StatusCode::OK);
    let login_cookies = set_cookie_values(&login_resp);
    assert_eq!(
        login_cookies.len(),
        2,
        "login must set both the access and refresh cookies, got: {login_cookies:?}"
    );
    assert!(login_cookies
        .iter()
        .any(|c| c.starts_with(lanrurugi_core::session::COOKIE_NAME)));
    assert!(login_cookies
        .iter()
        .any(|c| c.starts_with(lanrurugi_core::session::REFRESH_COOKIE_NAME)));

    // The refresh cookie must be scoped to only the one endpoint that reads it — a real browser
    // then never attaches it to any other request (including `/api/logout`, which deliberately
    // reads `fid` from the access token instead precisely so this scoping is possible — see that
    // handler's own docs).
    let refresh_set_cookie = raw_set_cookie_headers(&login_resp)
        .into_iter()
        .find(|c| c.starts_with(lanrurugi_core::session::REFRESH_COOKIE_NAME))
        .expect("refresh cookie must be set");
    assert!(
        refresh_set_cookie.contains("Path=/api/token/refresh"),
        "got: {refresh_set_cookie}"
    );
    let access_set_cookie = raw_set_cookie_headers(&login_resp)
        .into_iter()
        .find(|c| c.starts_with(lanrurugi_core::session::COOKIE_NAME))
        .expect("access cookie must be set");
    // `Path=/;` (with the trailing `;`), not just a `Path=/` prefix match — the refresh cookie's
    // own `Path=/api/token/refresh` would otherwise also satisfy a looser substring check.
    assert!(
        access_set_cookie.contains("Path=/;"),
        "got: {access_set_cookie}"
    );

    // The freshly issued access token authenticates a real protected endpoint.
    let protected_resp = request(
        &app,
        "GET",
        "/api/settings",
        Some(&cookie_header(&login_cookies)),
        None,
    )
    .await;
    assert_eq!(protected_resp.status(), axum::http::StatusCode::OK);

    // Refresh: rotates both cookies to new values.
    let refresh_resp = request(
        &app,
        "POST",
        "/api/token/refresh",
        Some(&cookie_header(&login_cookies)),
        None,
    )
    .await;
    assert_eq!(refresh_resp.status(), axum::http::StatusCode::OK);
    let rotated_cookies = set_cookie_values(&refresh_resp);
    assert_eq!(rotated_cookies.len(), 2);
    assert_ne!(
        rotated_cookies, login_cookies,
        "rotation must issue genuinely new cookie values, not repeat the old ones"
    );

    // The rotated access token also authenticates the protected endpoint.
    let protected_resp_2 = request(
        &app,
        "GET",
        "/api/settings",
        Some(&cookie_header(&rotated_cookies)),
        None,
    )
    .await;
    assert_eq!(protected_resp_2.status(), axum::http::StatusCode::OK);

    // Reuse detection: presenting the now-rotated-away original refresh cookie again must burn the
    // whole family, not just fail this one request.
    let reuse_resp = request(
        &app,
        "POST",
        "/api/token/refresh",
        Some(&cookie_header(&login_cookies)),
        None,
    )
    .await;
    assert_eq!(reuse_resp.status(), axum::http::StatusCode::UNAUTHORIZED);

    // The burn from reuse detection must have also invalidated the *rotated* refresh cookie (same
    // family) — the whole chain is dead, not just the token that was actually replayed.
    let refresh_after_burn = request(
        &app,
        "POST",
        "/api/token/refresh",
        Some(&cookie_header(&rotated_cookies)),
        None,
    )
    .await;
    assert_eq!(
        refresh_after_burn.status(),
        axum::http::StatusCode::UNAUTHORIZED,
        "reuse detection must burn the entire refresh-token family, not just the replayed token"
    );

    // A brand new login (independent of the burned family above) to verify logout's own explicit
    // revocation path, separately from reuse-detection's.
    let login_resp_2 = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some("password=kamimamita"),
    )
    .await;
    assert_eq!(login_resp_2.status(), axum::http::StatusCode::OK);
    let cookies_2 = set_cookie_values(&login_resp_2);

    // Only the access-token cookie — matching what a real browser actually sends to `/api/logout`
    // once the refresh cookie is `Path`-scoped away from it (see the `Path=/api/token/refresh`
    // assertion above). If `logout` still depended on reading the refresh cookie itself, this
    // would silently no-op instead of actually revoking anything.
    let logout_resp = request(
        &app,
        "POST",
        "/api/logout",
        Some(&access_token_cookie_only(&cookies_2)),
        None,
    )
    .await;
    assert_eq!(logout_resp.status(), axum::http::StatusCode::OK);

    // The now-logged-out refresh cookie must no longer be redeemable.
    let refresh_after_logout = request(
        &app,
        "POST",
        "/api/token/refresh",
        Some(&cookie_header(&cookies_2)),
        None,
    )
    .await;
    assert_eq!(
        refresh_after_logout.status(),
        axum::http::StatusCode::UNAUTHORIZED,
        "logout must actually revoke the refresh-token family, not just clear the browser's cookies"
    );

    purge_all_refresh_and_api_tokens(&redis).await;
}

/// Issue #91's own consolidation of `require_api_key` + `require_session` into one Casbin-backed
/// check — end-to-end coverage that a real Session can reach a Session-only route
/// (`GET /api/tokens`, chosen since it's Session-only, needs no request body to reach the auth
/// check, *and* has no side effect once reached — `POST /database/drop`, this test's original
/// choice, actually ran a real `FLUSHDB` against every logical DB once the session's request got
/// past the auth check, since the point of this test is proving the request *reaches the
/// handler*, not stopping short of it. Under `cargo test --workspace`, that wiped
/// `settings_toggles.rs`'s own in-flight archive/tag-index fixtures in a separate OS process
/// against the same real Redis, causing its `guest_search_excludes_out_of_scope_archive_sharing_a_tag`
/// to intermittently see an empty search result — confirmed live via CI log inspection,
/// 2026-08-28) while a real Admin-role API token (created through the live `POST /tokens`
/// endpoint, not hand-built) gets `403 Forbidden`, mirroring
/// `authz::tests::admin_token_may_not_call_any_session_only_route`'s own unit-level coverage but
/// through the actual HTTP router this time — the exact case that regressed once already this
/// session (`require_api_key`'s `enable_pass: false` short-circuit briefly stopped routing
/// through `check_route` at all before this test existed).
#[tokio::test]
async fn session_only_route_rejects_a_real_admin_token_but_accepts_a_real_session() {
    let _guard = redis_state_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    purge_all_refresh_and_api_tokens(&redis).await;

    let login_resp = request(
        &app,
        "POST",
        "/api/login",
        None,
        Some("password=kamimamita"),
    )
    .await;
    assert_eq!(login_resp.status(), axum::http::StatusCode::OK);
    let login_cookies = set_cookie_values(&login_resp);
    let cookie = cookie_header(&login_cookies);

    // A real Admin-role token, issued through the live endpoint (not fabricated) — the whole
    // point is exercising the exact same code path a real client would.
    let create_resp = request_json(
        &app,
        "POST",
        "/api/tokens",
        Some(&cookie),
        None,
        Some(r#"{"name":"authz-test-admin","role":"admin"}"#),
    )
    .await;
    assert_eq!(create_resp.status(), axum::http::StatusCode::OK);
    let create_bytes = axum::body::to_bytes(create_resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let create_json: serde_json::Value = serde_json::from_slice(&create_bytes).unwrap();
    let raw_token = create_json["data"]["token"]
        .as_str()
        .expect("create_token must return the raw token value")
        .to_string();

    // The Admin token must be rejected — `GET /api/tokens` is Session-only regardless of role.
    let tokens_via_token =
        request_json(&app, "GET", "/api/tokens", None, Some(&raw_token), None).await;
    assert_eq!(
        tokens_via_token.status(),
        axum::http::StatusCode::FORBIDDEN,
        "an Admin-role API token must never reach a Session-only route"
    );

    // The real Session, by contrast, must be let through to the handler itself — asserting
    // anything other than 403 here (a real `200` from `list_tokens`, or even a `500` from the
    // handler's own logic) is enough to prove `check_route` didn't block it; this test isn't
    // about `list_tokens`'s own behavior once reached.
    let tokens_via_session =
        request_json(&app, "GET", "/api/tokens", Some(&cookie), None, None).await;
    assert_ne!(
        tokens_via_session.status(),
        axum::http::StatusCode::FORBIDDEN,
        "a real Session must not be blocked from a route it's explicitly allowed to reach"
    );

    // `GET /api/theme` — merged in *before* `require_api_key`'s layer (see `app.rs::build_app`'s
    // own docs) — must stay reachable with zero credentials at all, unaffected by anything above.
    let theme_resp = request(&app, "GET", "/api/theme", None, None).await;
    assert_eq!(
        theme_resp.status(),
        axum::http::StatusCode::OK,
        "an unauthenticated request to the public /theme endpoint must never be blocked by the \
         protected router's own Casbin check"
    );

    purge_all_refresh_and_api_tokens(&redis).await;
}

/// 007-guest-restricted-access, US1: with guest mode off (the default — no `guestmode` field ever
/// set on this fresh test Redis instance), an unauthenticated caller must still be rejected from
/// both an ordinary protected endpoint and a session-only one — password login has no way to be
/// disabled, and guest mode being off means there's no eligibility branch for this request to fall
/// into either.
#[tokio::test]
async fn unauthenticated_request_is_rejected_when_guest_mode_is_off() {
    let _guard = redis_state_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    purge_all_refresh_and_api_tokens(&redis).await;

    let settings_resp = request(&app, "GET", "/api/settings", None, None).await;
    assert_eq!(
        settings_resp.status(),
        axum::http::StatusCode::UNAUTHORIZED,
        "an ordinary protected endpoint must reject an unauthenticated caller when guest mode is off"
    );

    let drop_resp = request(&app, "POST", "/api/database/drop", None, None).await;
    assert_eq!(
        drop_resp.status(),
        axum::http::StatusCode::UNAUTHORIZED,
        "a session-only endpoint must also reject an unauthenticated caller when guest mode is off"
    );
}

/// 007-guest-restricted-access, US2: once guest mode is genuinely on (a real `guestmode=1` config
/// field plus a real guest-visible category, not a unit-level `AuthContext` built by hand — see
/// `authz::tests::guest_visitor_is_read_only_and_cannot_download_raw_archives` for that narrower
/// coverage), an eligible unauthenticated caller reaches an ordinary read route but is still
/// rejected — `403`, not `401` — from a Session-only route, the same restriction `token_guest`
/// already has. Exercises the real end-to-end router (`procedure.rs`'s guest-eligibility branch +
/// `route_policy.csv`'s `guest_visitor` deny rules together), not just one half in isolation.
#[tokio::test]
async fn guest_visitor_reaches_ordinary_routes_but_not_session_only_ones() {
    let _guard = redis_state_lock().lock().await;
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    // Cross-*process* lock, not just this file's own in-process `redis_state_lock` above — see
    // `RedisTestLock`'s own docs. `cargo test --workspace` runs `settings_toggles.rs` (which also
    // writes the shared `guestmode` field) as a genuinely separate OS process against the exact
    // same real Redis instance; an in-process `Mutex` here is invisible to that other process,
    // confirmed live as a real intermittent CI failure, 2026-08-27.
    let guest_lock =
        lanrurugi_storage::test_support::RedisTestLock::acquire(&redis.config, "guestmode").await;
    purge_all_refresh_and_api_tokens(&redis).await;

    let mut conn = redis.config.get().await.unwrap();
    let _: () = deadpool_redis::redis::AsyncCommands::hset(
        &mut conn,
        lanrurugi_storage::keys::CONFIG_KEY,
        "guestmode",
        "1",
    )
    .await
    .unwrap();

    // `guest_has_any_visible_archive` (`lanrurugi-api::search`, c89ec44) requires a guest-visible
    // *static* category to actually contain a real archive, not just carry the flag — an empty
    // `archives: Vec::new()` category (this test's own original shape) makes that check return
    // `false` and 401 an otherwise-eligible guest, confirmed live via real CI failure, 2026-08-28.
    // `"9"` specifically — `settings_toggles.rs` (a separate OS process against the same real
    // Redis under `cargo test --workspace`) already claims `"1"`-`"8"` for its own archive-id
    // fixtures; reusing `"1"` here collided with its `in_scope_id` and caused a real, intermittent
    // CI-only failure (this test's own `archives.delete` racing that other file's still-in-flight
    // assertion against the same id), confirmed live via CI log inspection, 2026-08-28.
    let archive_id = "9".repeat(40);
    let repos = lanrurugi_api::Repositories::new(&redis);
    repos
        .archives
        .save(&test_archive(&archive_id, "Guest Visible Archive"))
        .await
        .unwrap();

    let category = lanrurugi_core::entities::Category {
        // `CategoryRepository::list_all()` matches only `SET_??????????` (exactly 10 chars after
        // the prefix, real categories are `SET_<10-digit-unix-timestamp>` — see repository.rs's
        // own doc comment) — a non-conforming id here is silently invisible to `list_all()`,
        // which is exactly what made `require_api_key`'s own `categories.list_all()` guest-
        // eligibility check see zero categories and 401 an otherwise-eligible guest (found via
        // real CI log inspection, 2026-08-28, not a lock/timing issue as previously suspected).
        catid: lanrurugi_core::ids::CategoryId("SET_9992010001".to_string()),
        name: "Guest Visible".to_string(),
        search: None,
        archives: vec![lanrurugi_core::ids::ArchiveId(archive_id.clone())],
        pinned: false,
        visible_to_guest: true,
    };
    repos.categories.save(&category).await.unwrap();

    let categories_resp = request(&app, "GET", "/api/categories", None, None).await;
    assert_eq!(
        categories_resp.status(),
        axum::http::StatusCode::OK,
        "an eligible guest_visitor must reach an ordinary read route"
    );

    let drop_resp = request(&app, "POST", "/api/database/drop", None, None).await;
    assert_eq!(
        drop_resp.status(),
        axum::http::StatusCode::FORBIDDEN,
        "a guest_visitor must never reach a Session-only route, even while otherwise eligible"
    );

    repos.categories.delete(&category.catid).await.unwrap();
    repos
        .archives
        .delete(&lanrurugi_core::ids::ArchiveId(archive_id))
        .await
        .unwrap();
    // Reset to `"0"`, not `HDEL` — this still runs under `guest_lock` above, but the field
    // becoming entirely absent (even briefly) risks a concurrently-running, *unlocked* test's own
    // `app` (a different `axum::Router` sharing this same real Redis) hitting `auth::load`'s hard
    // `MissingField` error via an unrelated request mid-flight. Restoring the same `"0"` default
    // `test_app()`'s own `HSETNX` would otherwise seed is a strictly safer no-op for every other
    // test than a window where the field doesn't exist at all.
    let _: () = deadpool_redis::redis::AsyncCommands::hset(
        &mut conn,
        lanrurugi_storage::keys::CONFIG_KEY,
        "guestmode",
        "0",
    )
    .await
    .unwrap();
    purge_all_refresh_and_api_tokens(&redis).await;
    guest_lock.release().await;
}
