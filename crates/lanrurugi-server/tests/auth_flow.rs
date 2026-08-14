//! Router-level integration test for the JWT-access-token + rotating-refresh-token login flow
//! (issue #44/#54): login issues both cookies, a protected endpoint accepts the access token,
//! `/token/refresh` rotates both, the rotated access token also works, and logout revokes the
//! whole refresh-token family so neither cookie works again afterward.
//!
//! Requires a real Redis instance (`LANRURUGI_TEST_REDIS_URL`), same convention as every other
//! integration test in this workspace — skips gracefully if unset.

use std::path::PathBuf;
use std::sync::Arc;

use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use lanrurugi_storage::redis::RedisDbs;
use tower::ServiceExt;

/// `enable_pass: true` — unlike every other integration test in this workspace (which run with
/// auth disabled to focus on the endpoint under test), this suite exists specifically to exercise
/// the auth flow itself.
async fn test_app() -> Option<(axum::Router, RedisDbs)> {
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
    let refresh_tokens = Arc::new(
        lanrurugi_storage::refresh_tokens::RefreshTokenRepository::new(redis.config.clone()),
    );
    let api_tokens = Arc::new(lanrurugi_storage::api_tokens::ApiTokenRepository::new(
        redis.config.clone(),
    ));
    let state = AppState {
        redis: redis.clone(),
        repos,
        jobs: JobRegistry::new(),
        auth: AuthConfig {
            enable_pass: true,
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
        recommender: Arc::new(lanrurugi_api::recommend::RecommendService::new()),
        new_archive_tx: tokio::sync::mpsc::unbounded_channel().0,
        download_cancellations: Default::default(),
        filename_locks: Default::default(),
        download_queue_tx: None,
        refresh_tokens,
        api_tokens,
        api_token_last_touch: Default::default(),
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

/// The full lifecycle in one test (rather than split across several) because each step's Redis
/// state depends on the previous step's cookies — splitting would just mean re-deriving the same
/// login/refresh chain repeatedly.
#[tokio::test]
async fn login_then_protected_request_then_refresh_then_logout_revokes_everything() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    purge_all_refresh_and_api_tokens(&redis).await;

    // No credentials at all: the protected endpoint must refuse.
    let resp = request(&app, "GET", "/api/info", None, None).await;
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
        "/api/info",
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
        "/api/info",
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
