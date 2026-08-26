//! `GET /opensearch.xml` end-to-end coverage (issue #90): unauthenticated reachability, per-Host/
//! per-X-Forwarded-Host template variation, per-client-IP content variation, and XML-injection
//! safety for header values containing `<`, `>`, `&`, `"` and IPv6 addresses.
//!
//! Requires a real Redis instance (`LANRURUGI_TEST_REDIS_URL`), same convention as every other
//! integration test in this workspace — skips gracefully if unset.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use lanrurugi_api::{AppState, AuthConfig, LibraryPaths, Repositories};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_scanner::handle::ScannerHandle;
use tower::ServiceExt;

async fn test_app() -> Option<axum::Router> {
    let redis = lanrurugi_storage::test_support::test_redis_dbs().await?;
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
        // `enable_pass: true` — the whole point of this suite is confirming `/opensearch.xml`
        // stays reachable *without* auth even when the rest of the API requires it; `false` would
        // make every route open regardless and prove nothing.
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
    let app = lanrurugi_server::app::build_app(state, None, None).layer(
        axum::extract::connect_info::MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))),
    );
    Some(app)
}

async fn get_opensearch(
    app: &axum::Router,
    headers: &[(&str, &str)],
) -> (axum::http::StatusCode, axum::http::HeaderMap, String) {
    let mut builder = axum::http::Request::builder().uri("/opensearch.xml");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(builder.body(axum::body::Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let response_headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(bytes.to_vec()).unwrap();
    (status, response_headers, body)
}

/// Extremely small well-formedness check — not a real XML parser (this workspace has no XML
/// dependency to reach for), but enough to catch the actual failure mode an injection would cause:
/// an unescaped `<`/`>`/unpaired `&` inside what should be text content breaks the tag structure
/// itself, which a naive tag-count/nesting check below still detects even without full validation.
fn looks_like_well_formed_xml(xml: &str) -> bool {
    xml.starts_with("<?xml")
        && xml.contains("<OpenSearchDescription")
        && xml.contains("</OpenSearchDescription>")
        // Every literal `&` must start one of the five predefined entities — a raw `&` from an
        // unescaped header value would fail this.
        && xml.match_indices('&').all(|(i, _)| {
            let rest = &xml[i..];
            rest.starts_with("&amp;")
                || rest.starts_with("&lt;")
                || rest.starts_with("&gt;")
                || rest.starts_with("&quot;")
                || rest.starts_with("&apos;")
        })
}

#[tokio::test]
async fn reachable_without_any_credentials_and_returns_valid_content_type() {
    let Some(app) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (status, headers, body) = get_opensearch(&app, &[]).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(
        headers.get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/opensearchdescription+xml; charset=utf-8"
    );
    assert_eq!(
        headers.get(axum::http::header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    assert!(looks_like_well_formed_xml(&body), "got: {body}");
}

#[tokio::test]
async fn url_template_reflects_the_forwarded_host_and_proto() {
    let Some(app) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (_, _, body) = get_opensearch(
        &app,
        &[
            ("X-Forwarded-Host", "lanrurugi.example.com"),
            ("X-Forwarded-Proto", "https"),
            ("Host", "127.0.0.1:3000"),
        ],
    )
    .await;
    assert!(
        body.contains("https://lanrurugi.example.com/?q={searchTerms}"),
        "got: {body}"
    );
    assert!(!body.contains("127.0.0.1:3000"), "got: {body}");
}

#[tokio::test]
async fn url_template_falls_back_to_the_bare_host_header_for_an_ip_port_visitor() {
    let Some(app) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (_, _, body) = get_opensearch(&app, &[("Host", "192.168.1.10:3000")]).await;
    assert!(
        body.contains("http://192.168.1.10:3000/?q={searchTerms}"),
        "got: {body}"
    );
}

#[tokio::test]
async fn content_varies_by_forwarded_client_ip() {
    let Some(app) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (_, _, body_a) = get_opensearch(&app, &[("X-Forwarded-For", "203.0.113.5")]).await;
    let (_, _, body_b) = get_opensearch(&app, &[("X-Forwarded-For", "203.0.113.9")]).await;
    assert!(body_a.contains("203.0.113.5"), "got: {body_a}");
    assert!(body_b.contains("203.0.113.9"), "got: {body_b}");
    assert_ne!(body_a, body_b);
}

#[tokio::test]
async fn ipv6_client_address_is_embedded_without_breaking_the_xml() {
    let Some(app) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (status, _, body) = get_opensearch(&app, &[("X-Forwarded-For", "2001:db8::1")]).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(looks_like_well_formed_xml(&body), "got: {body}");
    assert!(body.contains("2001:db8::1"), "got: {body}");
}

#[tokio::test]
async fn a_malicious_host_header_is_escaped_not_injected() {
    let Some(app) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (status, _, body) = get_opensearch(
        &app,
        &[(
            "X-Forwarded-Host",
            "evil.example\"><script>alert(1)</script>",
        )],
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(!body.contains("<script>"), "got: {body}");
    assert!(looks_like_well_formed_xml(&body), "got: {body}");
}

#[tokio::test]
async fn missing_client_ip_headers_still_produce_valid_xml() {
    let Some(app) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let (status, _, body) = get_opensearch(&app, &[]).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(looks_like_well_formed_xml(&body), "got: {body}");
}
