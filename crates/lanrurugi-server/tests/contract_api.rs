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
    let subscriptions = std::sync::Arc::new(
        lanrurugi_storage::subscriptions::SubscriptionRepository::new(redis.config.clone()),
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
    let activity_dedup = std::sync::Arc::new(
        lanrurugi_storage::activity_dedup::ActivityDedupGate::new(redis.config.clone()),
    );
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    let state = AppState {
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
        subscriptions: subscriptions.clone(),
        subscriptions_in_flight: Default::default(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
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
/// The binary's single test session, created on first use.
///
/// Every test here logging in separately used to exceed the configured "max login devices" limit
/// concurrently, and the handler evicts the oldest session when it does — so one test's request
/// could come back 401 because a *different* test had just logged in (observed live as
/// intermittent `left: 401, right: 200` failures in this file; adding one more test that logs in
/// made them frequent). The cookie is an opaque session id, so sharing one across tests is safe.
static SHARED_COOKIE: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

async fn login_cookie(app: &axum::Router) -> String {
    SHARED_COOKIE
        .get_or_init(|| login_cookie_fresh(app))
        .await
        .clone()
}

async fn login_cookie_fresh(app: &axum::Router) -> String {
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

/// Additive raw-archive search (`GET /search/archives`) regression coverage: an archive that
/// has been folded into a Tankoubon is removed from `LRR_TANKGROUPED`, so the existing grouped
/// `/search/ids` endpoint correctly returns no standalone result for it. That is the wrong answer
/// when a caller is checking whether the archive itself is in the library, so the new endpoint
/// must force `groupby_tanks=false` (and ignore `tankonly`) and return the underlying archive
/// record instead of making the member invisible.
#[tokio::test]
async fn ungrouped_search_finds_archives_folded_into_tankoubons() {
    use deadpool_redis::redis::AsyncCommands;
    use lanrurugi_search::keys::TANKGROUPED_KEY;

    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let cookie = login_cookie(&app).await;

    let id = "a1b2c3d4".repeat(5);
    let title = "Folded Raw Search Fixture";
    let tag = "zzrawsearchfixture:folded";
    let repo = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    repo.save(&Archive {
        id: lanrurugi_core::ids::ArchiveId(id.clone()),
        name: title.to_string(),
        title: title.to_string(),
        file: format!("/nonexistent/{id}.zip"),
        tags: tag.to_string(),
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

    lanrurugi_search::indexer::index_new_archive(
        &redis.search,
        &lanrurugi_search::Equivalence::default(),
        &id,
        title,
    )
    .await
    .unwrap();
    lanrurugi_search::indexer::update_tag_indexes(
        &redis.search,
        &lanrurugi_search::Equivalence::default(),
        &id,
        "",
        tag,
    )
    .await
    .unwrap();

    // A real Tankoubon whose reverse index points at the archive, so the raw endpoint can report
    // which Tankoubon it is filed under.
    let tankid = lanrurugi_core::ids::TankId("TANK_9170000001".to_string());
    let grouping_repo =
        lanrurugi_storage::repository::GroupingRepository::new(redis.archive.clone());
    grouping_repo
        .save(&lanrurugi_core::entities::Grouping {
            tankid: tankid.clone(),
            name: "Raw Search Fixture Tank".to_string(),
            summary: String::new(),
            tags: String::new(),
            progress: 0,
            archives: vec![lanrurugi_core::ids::ArchiveId(id.clone())],
            thumbnail_manual: false,
            thumbnail_source_archive: None,
            thumbnail_source_page: None,
            chapter_names: Default::default(),
            created_at: None,
            updated_at: None,
        })
        .await
        .unwrap();

    // What a real Tankoubon membership change does (`indexer::sync_tank_membership`'s `joined`
    // half): a member archive no longer appears in the grouped search candidate set.
    let mut sconn = redis.search.get().await.unwrap();
    let _: () = sconn.srem(TANKGROUPED_KEY, &id).await.unwrap();

    let (status, grouped) = get_json(
        &app,
        &format!("/api/search/ids?filter={tag}"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        grouped["data"].as_array().unwrap().is_empty(),
        "the existing grouped endpoint should still hide the tank member: {grouped}"
    );

    // Even explicitly asking for grouped/tank-only results must not reintroduce the hidden member:
    // `/search/archives` owns the raw-archive semantics, not its caller's query parameters.
    let (status, raw) = get_json(
        &app,
        &format!("/api/search/archives?filter={tag}&groupby_tanks=true&tankonly=true"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let data = raw["data"].as_array().unwrap();
    assert_eq!(
        data.len(),
        1,
        "new endpoint must return the raw archive: {raw}"
    );
    assert_eq!(data[0]["arcid"], id.as_str());
    assert_eq!(
        data[0]["tankid"],
        tankid.as_str(),
        "raw endpoint must report the Tankoubon the archive belongs to: {raw}"
    );
    assert_eq!(data[0]["archive_index"], 0);
    assert_eq!(data[0]["tank_sequence"], 1);
    assert_eq!(data[0]["tankoubon"]["id"], tankid.as_str());
    assert_eq!(
        data[0]["tankoubon"]["name"], "Raw Search Fixture Tank",
        "raw endpoint must include the owning Tankoubon's own metadata: {raw}"
    );

    lanrurugi_search::indexer::remove_archive_index(
        &redis.search,
        &lanrurugi_search::Equivalence::default(),
        &id,
        title,
        tag,
    )
    .await
    .unwrap();
    grouping_repo.delete(&tankid).await.unwrap();
    repo.delete(&lanrurugi_core::ids::ArchiveId(id))
        .await
        .unwrap();
}

/// Regression guard for a real bug (found live, 2026-10-10): page-level bookmarks live on the
/// **config** logical DB (`BookmarksRepository`'s own docs), but the search engine built its own
/// repository from the *archive* pool — so `has:bookmark` and `bookmark:"name"` matched nothing in
/// production, while the engine's own unit test happily passed because it wrote and read through
/// that same wrong pool (a test that agrees with the bug). Only a test reaching the real router —
/// and therefore the real `AppState` wiring — can see the difference, which is what this is.
#[tokio::test]
async fn bookmark_search_operators_see_bookmarks_where_the_router_stores_them() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let cookie = login_cookie(&app).await;

    let id = "b00c0de0".repeat(5);
    let title = "Bookmark Search Fixture";
    let tag = "zzbookmarkfixture:config";
    let repo = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    repo.save(&Archive {
        id: lanrurugi_core::ids::ArchiveId(id.clone()),
        name: title.to_string(),
        title: title.to_string(),
        file: format!("/nonexistent/{id}.zip"),
        tags: tag.to_string(),
        summary: String::new(),
        arcsize: 1,
        pagecount: 4,
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
    lanrurugi_search::indexer::index_new_archive(
        &redis.search,
        &lanrurugi_search::Equivalence::default(),
        &id,
        title,
    )
    .await
    .unwrap();
    lanrurugi_search::indexer::update_tag_indexes(
        &redis.search,
        &lanrurugi_search::Equivalence::default(),
        &id,
        "",
        tag,
    )
    .await
    .unwrap();

    // Written through the same repository instance `AppState` holds (config DB), i.e. exactly where
    // the app itself would put it.
    let bookmarks = lanrurugi_storage::bookmarks::BookmarksRepository::new(redis.config.clone());
    bookmarks.add(&id, 3, 1_700_000_000).await.unwrap();
    bookmarks
        .set_name(&id, 3, Some("Chapter One"))
        .await
        .unwrap();

    // The tag keeps the assertion independent of whatever else happens to be bookmarked in the
    // shared scratch Redis.
    let (status, has) = get_json(
        &app,
        &format!("/api/search/ids?filter={tag}%20has:bookmark"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        has["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some(id.as_str())),
        "`has:bookmark` must see a bookmark stored on the config DB: {has}"
    );

    // Substring + case-insensitive for the name, and folded on both sides — the documented
    // `bookmark:"name"` semantics, not a whole-name equality.
    let (status, named) = get_json(
        &app,
        &format!("/api/search/ids?filter={tag}%20bookmark:%22chapter%22"),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert!(
        named["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str() == Some(id.as_str())),
        "`bookmark:\"chapter\"` must match a name containing it: {named}"
    );

    bookmarks.remove(&id, 3, 1_700_000_100).await.unwrap();
    lanrurugi_search::indexer::remove_archive_index(
        &redis.search,
        &lanrurugi_search::Equivalence::default(),
        &id,
        title,
        tag,
    )
    .await
    .unwrap();
    repo.delete(&lanrurugi_core::ids::ArchiveId(id))
        .await
        .unwrap();
}

/// Percent-encodes a filter for a query string. Hand-rolled instead of pulling in a URL crate for
/// the test target: the matrix below uses spaces, quotes, parentheses, `|` and `%`, none of which
/// any existing test here needed (`:` is left literal for readability — it is legal in a query
/// value).
fn encode_filter(filter: &str) -> String {
    let mut out = String::new();
    for byte in filter.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The search syntax matrix: every operator the query language grew (`|`, grouping, `-`, `title:`,
/// `filename:`, `is:*`, `has:*`, `in:`, `size:`, `read:`, `phrase:`, `category:`, `bookmark:`,
/// `date_added:` comparisons, wildcards) asserted **through the real router**, plus the
/// combinations where a bug is most likely to hide — precedence against grouping, a phrase inside a
/// negated conjunction, state + numeric + text in one query.
///
/// Deliberately API-level rather than engine-level: the bookmark operators were silently dead in
/// production for exactly one reason — the engine built its own repository from the wrong logical
/// DB — while its unit test passed, because that test wrote and read through the same wrong pool.
/// Anything wired by `AppState` can only be checked here.
///
/// Assertions are membership/exclusion on this test's own fixtures rather than whole-set equality:
/// `is:new`/`size:`/`read:`/`is:untagged` are library-wide predicates, and this Redis is shared
/// with the other tests in this binary. Every filter that can be scoped is scoped by `run`, a
/// per-invocation tag namespace.
#[tokio::test]
async fn search_syntax_matrix_covers_every_operator_and_its_combinations() {
    // Serialized with the other tests that create and delete Tankoubons in this shared scratch
    // Redis (`subfolders_to_categories_...`/`subfolders_to_tankoubons_...` take this same lock):
    // `LRR_TANKGROUPED` is global mutable state and this matrix asserts on tank membership.
    let _serial = GLOBAL_LISTING_LOCK.lock().await;
    use deadpool_redis::redis::AsyncCommands;
    use lanrurugi_core::entities::{Archive, Category, Grouping};
    use lanrurugi_core::ids::{ArchiveId, CategoryId, TankId};
    use lanrurugi_search::keys::{NEW_KEY, TANKGROUPED_KEY};

    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let cookie = login_cookie(&app).await;
    let eq = lanrurugi_search::Equivalence::default();

    /// Current Unix seconds — the fixtures' own `date_added` tags, so `is:new`'s timed-window
    /// modes see them as recent.
    fn now_secs() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    let run = format!(
        "zzsyn{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    );

    const A: &str = "a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1";
    const B: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2";
    const C: &str = "c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3c3";
    const D: &str = "d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4d4";
    const E: &str = "e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5e5";
    /// The one fixture folded into a Tankoubon: `in:tank` is *defined* as "absent from
    /// `LRR_TANKGROUPED`", and with `groupby_tanks=true` that same set is the search's own candidate
    /// scope — so this state has to live on a dedicated archive, or it would hide that archive from
    /// every other case in the matrix.
    const F: &str = "f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6f6";
    // `TANK_` + exactly 10 characters: legacy's own glossary glob, which both the grouping
    // repository and the search side enumerate tanks with.
    let tank = "TANK_99zzsyn001".to_string();

    let repo = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    let bookmarks = lanrurugi_storage::bookmarks::BookmarksRepository::new(redis.config.clone());
    let categories = lanrurugi_storage::repository::CategoryRepository::new(redis.archive.clone());
    let groupings = lanrurugi_storage::repository::GroupingRepository::new(redis.archive.clone());
    let suggestions =
        lanrurugi_storage::archive_split_suggestions::ArchiveSplitSuggestionsRepository::new(
            redis.config.clone(),
        );

    // id, title, tags, progress, pagecount, arcsize, has_patch
    let fixtures: Vec<(&str, &str, String, u32, u32, u64, bool)> = vec![
        (
            // `date_added` is the *current* second and progress stays under `pagecount` on purpose:
            // `is:new` must hold under every `newbadgemode` (membership-only, until-finished, or a
            // timed window), without this test writing that config field itself — an earlier
            // version did, and broke the neighbouring settings test that asserts the same hash.
            A,
            "Alpha Beta Gamma",
            format!("{run},artist:shared,female:x,date_added:{}", now_secs()),
            9,
            10,
            200 * 1024 * 1024,
            true,
        ),
        (
            B,
            "Beta Alpha",
            format!("{run},artist:shared,female:y"),
            5,
            10,
            1024 * 1024,
            false,
        ),
        (
            C,
            "Gamma Alpha Beta",
            format!("{run},artist:other,date_added:{}", now_secs()),
            0,
            10,
            500 * 1024,
            false,
        ),
        // No tags at all: this is the fixture `is:untagged` must find, so it deliberately cannot be
        // scoped by `run`.
        (D, "Untagged Solo", String::new(), 0, 10, 1024 * 1024, false),
        (
            E,
            "Zeta Unique",
            format!("{run},zzrel:alpha,date_added:1780272000"),
            0,
            10,
            2 * 1024 * 1024,
            false,
        ),
        (
            F,
            "Delta Epsilon",
            format!("{run},artist:folded"),
            0,
            10,
            3 * 1024 * 1024,
            false,
        ),
    ];

    for (id, title, tags, progress, pagecount, arcsize, has_patch) in &fixtures {
        repo.save(&Archive {
            id: ArchiveId(id.to_string()),
            name: title.to_string(),
            title: title.to_string(),
            // Basename deliberately spelled unlike the title, so `filename:` and `title:` cannot
            // accidentally agree.
            file: format!("/nonexistent/{}-alpha-file-name.zip", id),
            tags: tags.clone(),
            summary: String::new(),
            arcsize: *arcsize,
            pagecount: *pagecount,
            isnew: false,
            lastreadpage: *progress,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: *has_patch,
        })
        .await
        .unwrap();
        lanrurugi_search::indexer::index_new_archive(&redis.search, &eq, id, title)
            .await
            .unwrap();
        lanrurugi_search::indexer::update_tag_indexes(&redis.search, &eq, id, "", tags)
            .await
            .unwrap();
        // `progress` is not part of the `Archive` entity's own save path above (the reader writes
        // it straight onto the hash), so set it the same way the API does.
        let mut aconn = redis.archive.get().await.unwrap();
        let _: () = aconn
            .hset(id, "progress", progress.to_string())
            .await
            .unwrap();
    }

    // `is:new`: a and c are in `LRR_NEW` (every freshly indexed archive is), b/e are not.
    let mut sconn = redis.search.get().await.unwrap();
    for id in [B, E] {
        let _: () = sconn.srem(NEW_KEY, id).await.unwrap();
    }
    // Two traps here, both legacy's own id shapes: the tank id must match
    // `TANK_??????????` (the glob both `GroupingRepository::list_all` and the search side's own
    // tank enumeration use — a `TANK_zzsynmatrix1` fixture is simply invisible to them), and it is
    // built through the repository plus `indexer::add_tank_to_index` rather than through
    // `PUT /api/tankoubons/{id}/{archive}`, because that endpoint re-syncs Tankoubon membership
    // globally and would undo the hand-written `srem` another test in this binary relies on.
    groupings
        .save(&Grouping {
            tankid: TankId(tank.clone()),
            name: "Syntax Matrix Tank".to_string(),
            summary: String::new(),
            tags: String::new(),
            progress: 0,
            archives: vec![ArchiveId(F.to_string())],
            thumbnail_manual: false,
            thumbnail_source_archive: None,
            thumbnail_source_page: None,
            chapter_names: Default::default(),
            created_at: None,
            updated_at: None,
        })
        .await
        .unwrap();
    lanrurugi_search::indexer::add_tank_to_index(&redis.search, &tank)
        .await
        .unwrap();
    // Without its own title-index entry a tank stays invisible to the *ordered* result page even
    // though `LRR_TANKGROUPED` lists it as a candidate — `tankoubons.rs`'s own create path says so
    // in as many words, and this fixture reproduced it by omitting exactly this call.
    lanrurugi_search::indexer::update_title_index(
        &redis.search,
        &eq,
        &tank,
        "",
        "Syntax Matrix Tank",
    )
    .await
    .unwrap();
    // What joining a Tankoubon does to a member: it leaves the standalone set, which is exactly
    // what `in:tank` reads.
    let _: () = sconn.srem(TANKGROUPED_KEY, F).await.unwrap();

    // Static category (a only) and a dynamic one whose predicate matches a and b — `has:category`
    // must see the first and not the second, `category:` must see both.
    // `CategoryRepository::list_all` scans legacy's own `SET_??????????` glob, so a fixture id of
    // any other shape is invisible to every name-based category lookup.
    let static_cat = CategoryId("SET_99zzsyn001".to_string());
    let dynamic_cat = CategoryId("SET_99zzsyn002".to_string());
    categories
        .save(&Category {
            catid: static_cat.clone(),
            name: "Syntax Static Category".to_string(),
            search: None,
            archives: vec![ArchiveId(A.to_string())],
            pinned: false,
            visible_to_guest: false,
        })
        .await
        .unwrap();
    categories
        .save(&Category {
            catid: dynamic_cat.clone(),
            name: "Syntax Dynamic Category".to_string(),
            search: Some(format!("{run} artist:shared")),
            archives: vec![],
            pinned: false,
            visible_to_guest: false,
        })
        .await
        .unwrap();

    bookmarks.add(A, 3, 1_700_000_000).await.unwrap();
    bookmarks.set_name(A, 3, Some("Chapter One")).await.unwrap();
    suggestions
        .save(
            &lanrurugi_storage::archive_split_suggestions::ArchiveSplitSuggestion {
                archive_id: A.to_string(),
                suggestion_version: 1,
                created_at: 1_700_000_000,
                split_groups: vec![],
                warnings: vec![],
            },
        )
        .await
        .unwrap();

    let ids_for_with = |filter: String, grouped: bool| {
        let app = app.clone();
        let cookie = cookie.clone();
        async move {
            let uri = format!(
                "/api/search/ids?filter={}&groupby_tanks={grouped}",
                encode_filter(&filter)
            );
            // Retried on a 5xx only: this binary's tests share one scratch Redis, and a burst of
            // searches here can starve a request of a pooled connection (observed as an
            // intermittent 500 while a neighbouring test's own request was in flight). A filter
            // that genuinely errors still fails, after the retries.
            let mut attempt = 0;
            let (status, body) = loop {
                let (status, body) = get_json(&app, &uri, Some(&cookie)).await;
                attempt += 1;
                if status.is_success() || !status.is_server_error() || attempt >= 3 {
                    break (status, body);
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            };
            assert_eq!(
                status,
                axum::http::StatusCode::OK,
                "filter {filter:?}: {body}"
            );
            body["data"]
                .as_array()
                .unwrap_or_else(|| panic!("filter {filter:?} returned no data array: {body}"))
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect::<Vec<String>>()
        }
    };
    let ids_for = |filter: String| ids_for_with(filter, true);

    // (filter, must be present, must be absent)
    let cases: Vec<(String, Vec<&str>, Vec<&str>)> = vec![
        // --- plain terms, implicit AND, scoping ------------------------------------------------
        (run.clone(), vec![A, B, C, E], vec![D]),
        (format!("{run} artist:shared"), vec![A, B], vec![C, E]),
        (format!("{run} zzrel:alpha"), vec![E], vec![A, B, C]),
        // wildcards are preserved through the grammar's own escaping
        (format!("{run} zzrel:al*"), vec![E], vec![A, B]),
        // --- boolean structure ----------------------------------------------------------------
        (
            format!("{run} artist:shared | artist:other"),
            vec![A, B, C],
            vec![E],
        ),
        // Grouping must mean exactly what distributing the AND by hand means.
        (
            format!("{run} (artist:shared | artist:other)"),
            vec![A, B, C],
            vec![E],
        ),
        // AND binds tighter than OR: `{run} artist:shared artist:other` is unsatisfiable, so the
        // parenthesised form below is *not* what the unparenthesised one means.
        (
            format!("{run} artist:shared artist:other"),
            vec![],
            vec![A, B, C, E],
        ),
        (
            format!("{run} artist:shared artist:other | zzrel:alpha"),
            vec![E],
            vec![A, B, C],
        ),
        (format!("{run} -artist:shared"), vec![C, E], vec![A, B]),
        (
            // a/b carry `artist:shared` and c carries `artist:other`, so e — carrying neither — is
            // what a doubly-negated conjunction should leave behind. f carries neither either, but
            // it is folded into the Tankoubon, so it is not in the grouped candidate set at all;
            // its own `in:tank` case below is what covers that state.
            format!("{run} -artist:shared -artist:other"),
            vec![E],
            vec![A, B, C, F],
        ),
        // --- text fields ----------------------------------------------------------------------
        (format!("{run} title:alpha"), vec![A, C], vec![E]),
        // `filename:` is a documented *alias* of `title:` (`grammar.rs::split_field_prefix`: "the
        // stored `title` is what legacy calls both"), so these two must agree — including on the
        // basename-shaped token, which is deliberately unlike every fixture's title and is
        // therefore expected to match nothing at all.
        (format!("{run} filename:alpha"), vec![A, C], vec![E]),
        (
            format!("{run} filename:alpha-file"),
            vec![],
            vec![A, B, C, E],
        ),
        // --- phrase: order and adjacency, not just co-occurrence ------------------------------
        (
            format!("{run} phrase:\"alpha beta\""),
            vec![A, C],
            vec![B, E],
        ),
        (format!("{run} phrase:\"beta alpha\""), vec![B], vec![A, C]),
        (
            format!("{run} phrase:\"alpha gamma\""),
            vec![],
            vec![A, B, C],
        ),
        // --- archive state --------------------------------------------------------------------
        (format!("{run} is:completed"), vec![A], vec![B, C, E]),
        (format!("{run} is:incomplete"), vec![B, C, E], vec![A]),
        (format!("{run} is:read"), vec![A, B], vec![C, E]),
        (format!("{run} is:unread"), vec![C, E], vec![A, B]),
        (format!("{run} is:new"), vec![A, C], vec![B, E]),
        // not scoped by `run`: d has no tags at all by construction
        ("is:untagged".to_string(), vec![D], vec![]),
        // --- numeric / date -------------------------------------------------------------------
        (format!("{run} size:>=100M"), vec![A], vec![B, C, E]),
        (format!("{run} size:<1M"), vec![C], vec![A, B, E]),
        (format!("{run} size:>=1M size:<2M"), vec![B], vec![A, C, E]),
        (format!("{run} read:>=80%"), vec![A], vec![B, C, E]),
        (format!("{run} read:>=40%"), vec![A, B], vec![C, E]),
        // a and c carry a `date_added` of *now* (see their fixture comments) while b carries none
        // at all and e carries the fixed 2026-06-01 one, so a comparison against 2026-01-01 has to
        // include the first two and e, and exclude b.
        (
            format!("{run} date_added:>=2026-01-01"),
            vec![A, C, E],
            vec![B],
        ),
        // The other direction, with a date before every fixture's own tag.
        (
            format!("{run} date_added:<2020-01-01"),
            vec![],
            vec![A, C, E],
        ),
        // --- associated state -----------------------------------------------------------------
        (format!("{run} has:patch"), vec![A], vec![B, C, E]),
        (format!("{run} has:bookmark"), vec![A], vec![B, C, E]),
        (
            format!("{run} bookmark:\"chapter\""),
            vec![A],
            vec![B, C, E],
        ),
        (
            format!("{run} bookmark:\"CHAPTER ONE\""),
            vec![A],
            vec![B, C],
        ),
        (format!("{run} bookmark:\"nope\""), vec![], vec![A, B, C]),
        // static membership only: b is matched by the *dynamic* category, not a static one
        (format!("{run} has:category"), vec![A], vec![B, C, E]),
        (
            format!("{run} has:split-suggestion"),
            vec![A],
            vec![B, C, E],
        ),
        // --- categories ----------------------------------------------------------------------
        (
            format!("{run} category:{static_cat}"),
            vec![A],
            vec![B, C, E],
        ),
        (
            format!("{run} category:\"Syntax Static Category\""),
            vec![A],
            vec![B, C, E],
        ),
        (
            format!("{run} category:\"Syntax Dynamic Category\""),
            vec![A, B],
            vec![C, E],
        ),
        // --- combinations: where a precedence or scope bug shows up --------------------------
        (
            format!("{run} (phrase:\"alpha beta\" | is:completed) -has:patch"),
            vec![C],
            vec![A, B, E],
        ),
        (
            format!("{run} size:>=1M (is:read | has:bookmark) -title:gamma"),
            vec![B],
            vec![A, C, E],
        ),
        (
            format!("{run} (title:zeta | zzrel:alpha) -is:read"),
            vec![E],
            vec![A, B, C],
        ),
        (
            format!("{run} category:\"Syntax Dynamic Category\" has:bookmark"),
            vec![A],
            vec![B, C, E],
        ),
        (
            format!("{run} phrase:\"beta alpha\" | size:<1M"),
            vec![B, C],
            vec![A, E],
        ),
        (
            format!("{run} (is:new | has:patch) size:>=100M"),
            vec![A],
            vec![B, C, E],
        ),
    ];

    let mut failures: Vec<String> = Vec::new();
    for (filter, must_include, must_exclude) in &cases {
        // Paced on purpose: ~50 back-to-back searches each take a pooled Redis connection, and this
        // binary runs its tests in parallel against one shared scratch Redis — un-paced, a
        // neighbouring test's single request intermittently came back 500 (confirmed by running
        // this test ignored: the failure disappeared).
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        let got = ids_for(filter.clone()).await;
        for id in must_include {
            if !got.iter().any(|g| g == id) {
                failures.push(format!("{filter:?}: expected {id} in {got:?}"));
            }
        }
        for id in must_exclude {
            if got.iter().any(|g| g == id) {
                failures.push(format!("{filter:?}: expected {id} absent from {got:?}"));
            }
        }
    }
    // `in:tank` needs the *raw* scope: with `groupby_tanks=true` the candidate set is
    // `LRR_TANKGROUPED` itself, so a folded archive is absent by construction and the operator
    // could only ever answer "nothing". Asserted from both sides — f is folded in, b is not.
    let raw_folded = ids_for_with(format!("{run} in:tank"), false).await;
    if !raw_folded.iter().any(|id| id == F) {
        failures.push(format!("`{run} in:tank`: expected {F} in {raw_folded:?}"));
    }
    for id in [A, B, C, E] {
        if raw_folded.iter().any(|g| g == id) {
            failures.push(format!(
                "`{run} in:tank`: expected {id} absent from {raw_folded:?}"
            ));
        }
    }
    let raw_not_folded = ids_for_with(format!("{run} -in:tank"), false).await;
    for id in [A, B, C, E] {
        if !raw_not_folded.iter().any(|g| g == id) {
            failures.push(format!(
                "`{run} -in:tank`: expected {id} in {raw_not_folded:?}"
            ));
        }
    }
    if raw_not_folded.iter().any(|g| g == F) {
        failures.push(format!(
            "`{run} -in:tank`: expected {F} absent from {raw_not_folded:?}"
        ));
    }

    // `is:tank` is a Tankoubon aggregate with no tags of its own, so it can only be asserted by
    // membership — and because this test binary runs its cases in parallel against one shared
    // scratch Redis (other tests here create and delete their own Tankoubons through the API),
    // the membership is re-asserted immediately before the check rather than trusted from
    // fixture setup minutes earlier.
    let tanks = ids_for("is:tank".to_string()).await;
    if !tanks.iter().any(|id| id == &tank) {
        failures.push(format!("`is:tank`: expected {tank} in {tanks:?}"));
    }
    if let Some(bad) = tanks.iter().find(|id| !id.starts_with("TANK_")) {
        failures.push(format!("`is:tank`: returned a non-aggregate {bad}"));
    }
    assert!(
        failures.is_empty(),
        "{} syntax matrix case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );

    // `sortby=relevance` ranks a title hit above a tag hit — a is titled "...Alpha..." while e only
    // carries `zzrel:alpha`, so a must come first with relevance ordering and the default order
    // (date_added desc) is free to disagree.
    let (status, relevant) = get_json(
        &app,
        &format!(
            "/api/search?filter={}&sortby=relevance",
            encode_filter(&format!("{run} alpha"))
        ),
        Some(&cookie),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let ordered: Vec<String> = relevant["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["arcid"].as_str().map(str::to_string))
        .collect();
    let a_pos = ordered.iter().position(|id| id == A);
    let e_pos = ordered.iter().position(|id| id == E);
    assert!(
        matches!((a_pos, e_pos), (Some(a_pos), Some(e_pos)) if a_pos < e_pos),
        "relevance must rank the title hit ({A}) above the tag hit ({E}): {ordered:?}"
    );

    // --- cleanup ------------------------------------------------------------------------------
    for (id, title, tags, ..) in &fixtures {
        lanrurugi_search::indexer::remove_archive_index(&redis.search, &eq, id, title, tags)
            .await
            .unwrap();
        repo.delete(&ArchiveId(id.to_string())).await.unwrap();
    }
    lanrurugi_search::indexer::remove_tank_from_index(&redis.search, &tank)
        .await
        .unwrap();
    groupings.delete(&TankId(tank.clone())).await.unwrap();
    categories.delete(&static_cat).await.unwrap();
    categories.delete(&dynamic_cat).await.unwrap();
    bookmarks.remove(A, 3, 1_700_000_100).await.unwrap();
    suggestions.delete(A).await.unwrap();
}

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
async fn version_endpoint_is_public_and_disabled_shape_is_stable() {
    let Some((app, _redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };

    // Contract tests run with `disable_update_check: true`; the endpoint must still be reachable
    // anonymously (it is mounted in the public router group, like `/api/info`) and must not make
    // an outbound GitHub request in that state.
    let (status, json) = get_json(&app, "/api/version", None).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    assert_eq!(json["enabled"], false);
    assert!(json["details"]["version"].is_string());
    assert!(json["details"]["sha"].is_null() || json["details"]["sha"].is_string());
    assert!(json["data"]["isLatest"].is_boolean());
    assert!(json["cached"].is_boolean());
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
    let subscriptions = std::sync::Arc::new(
        lanrurugi_storage::subscriptions::SubscriptionRepository::new(redis.config.clone()),
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
    let activity_dedup = std::sync::Arc::new(
        lanrurugi_storage::activity_dedup::ActivityDedupGate::new(redis.config.clone()),
    );
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    let state = AppState {
        equivalence: std::sync::Arc::new(lanrurugi_search::Equivalence::default()),
        discovery_singleflight: std::sync::Arc::new(
            lanrurugi_core::singleflight::Singleflight::new(8),
        ),
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
        subscriptions: subscriptions.clone(),
        subscriptions_in_flight: Default::default(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
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
    let subscriptions = std::sync::Arc::new(
        lanrurugi_storage::subscriptions::SubscriptionRepository::new(redis.config.clone()),
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
    let activity_dedup = std::sync::Arc::new(
        lanrurugi_storage::activity_dedup::ActivityDedupGate::new(redis.config.clone()),
    );
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );
    let state = AppState {
        equivalence: std::sync::Arc::new(lanrurugi_search::Equivalence::default()),
        discovery_singleflight: std::sync::Arc::new(
            lanrurugi_core::singleflight::Singleflight::new(8),
        ),
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
        subscriptions: subscriptions.clone(),
        subscriptions_in_flight: Default::default(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
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
/// Serializes the two `subfolders_to_*` tests below — the only tests in this file that read a
/// *global* listing (`/api/categories`, `/api/tankoubons`) right after creating an entry in it.
/// Cargo runs this file's tests on parallel threads against one shared Redis, so whichever one
/// asserts while the other has already created its own subfolder-derived entry can see it.
///
/// The window is real, not theoretical: `create_category` mirrors legacy's `SET_<now>` id
/// allocation with a check-then-save loop (`lanrurugi-api/src/categories.rs`), so two creations in
/// the same second where *both checks run before either save* pick the same id and one clobbers the
/// other. Holding this for the whole test body removes the concurrency (each one then sees the
/// other's key on its own check and bumps to `SET_<now+1>`, exactly as legacy's loop intends).
/// The same check-then-save race between two genuinely concurrent HTTP requests stays a
/// product-side, legacy-inherited property — deliberately not changed here.
static GLOBAL_LISTING_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
async fn subfolders_to_categories_creates_a_category_visible_in_list_all() {
    let _serial = GLOBAL_LISTING_LOCK.lock().await;
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
    let subscriptions = std::sync::Arc::new(
        lanrurugi_storage::subscriptions::SubscriptionRepository::new(redis.config.clone()),
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
    let activity_dedup = std::sync::Arc::new(
        lanrurugi_storage::activity_dedup::ActivityDedupGate::new(redis.config.clone()),
    );
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
        subscriptions: subscriptions.clone(),
        subscriptions_in_flight: Default::default(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
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

/// Native `subfolders-to-tankoubons` integration test: each subfolder with archives becomes one
/// Tankoubon, and the created Tankoubons are visible through the normal listing endpoint.
#[tokio::test]
async fn subfolders_to_tankoubons_creates_tankoubons_visible_in_list_all() {
    let _serial = GLOBAL_LISTING_LOCK.lock().await;
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
    let subscriptions = std::sync::Arc::new(
        lanrurugi_storage::subscriptions::SubscriptionRepository::new(redis.config.clone()),
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
    let activity_dedup = std::sync::Arc::new(
        lanrurugi_storage::activity_dedup::ActivityDedupGate::new(redis.config.clone()),
    );
    let import_snapshots = std::sync::Arc::new(
        lanrurugi_backup::import_snapshot::ImportSnapshotRepository::new(redis.config.clone()),
    );

    let library_dir = tempfile::tempdir().unwrap();
    let series_dir = library_dir.path().join("My Series");
    let another_dir = library_dir.path().join("Another Series");
    let nested_dir = series_dir.join("nested");
    std::fs::create_dir_all(&series_dir).unwrap();
    std::fs::create_dir_all(&another_dir).unwrap();
    std::fs::create_dir_all(&nested_dir).unwrap();
    let path_a = series_dir.join("Volume 1.zip");
    let path_b = series_dir.join("Volume 2.zip");
    let path_c = another_dir.join("Volume 1.cbz");
    let path_d = nested_dir.join("Volume 0.zip");
    for path in [&path_a, &path_b, &path_c, &path_d] {
        std::fs::write(path, b"fake archive bytes").unwrap();
    }

    // These IDs must not collide with the singleton "a"/"b"/"c"/"d"/"f" archive IDs used by
    // other tests in this same binary: `cargo test` runs tests in parallel against one shared
    // Redis, so a same-ID deletion/rewrite from another test can make this test's `id_by_path`
    // miss a path and silently skip a first-level subfolder.
    let id_a = "e".repeat(40);
    let id_b = "g".repeat(40);
    let id_c = "h".repeat(40);
    let id_d = "i".repeat(40);
    let archive_repo = lanrurugi_storage::repository::ArchiveRepository::new(redis.archive.clone());
    for (id, path, title) in [
        (id_a.clone(), path_a, "Volume 1"),
        (id_b.clone(), path_b, "Volume 2"),
        (id_c.clone(), path_c, "Volume 1"),
        (id_d.clone(), path_d, "Volume 0"),
    ] {
        archive_repo
            .save(&Archive {
                id: lanrurugi_core::ids::ArchiveId(id.clone()),
                name: title.to_string(),
                title: title.to_string(),
                file: path.to_string_lossy().to_string(),
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
    }

    let state = AppState {
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
        subscriptions: subscriptions.clone(),
        subscriptions_in_flight: Default::default(),
        recommend_cache: recommend_cache.clone(),
        ignored_group_suggestions: ignored_group_suggestions.clone(),
        compare_cache: compare_cache.clone(),
        bookmarks: bookmarks.clone(),
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
    let app = lanrurugi_server::app::build_app(state, None, None).layer(
        axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            0,
        ))),
    );

    let cookie = login_cookie(&app).await;
    let mut config_conn = redis.config.get().await.unwrap();
    use deadpool_redis::redis::AsyncCommands;
    let _: () = config_conn
        .hset("LRR_CONFIG", "subfolders_to_tankoubons", "0")
        .await
        .unwrap();

    // When the setting is off, the endpoint is a non-destructive no-op.
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/database/scripts/subfolders-to-tankoubons")
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let disabled: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(disabled["enabled"], false);
    assert_eq!(disabled["created_tankoubons"].as_array().unwrap().len(), 0);

    let _: () = config_conn
        .hset("LRR_CONFIG", "subfolders_to_tankoubons", "1")
        .await
        .unwrap();
    let response = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/api/database/scripts/subfolders-to-tankoubons")
                .header("cookie", &cookie)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let created: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(created["enabled"], true);
    let created_tanks = created["created_tankoubons"].as_array().unwrap();
    assert_eq!(
        created_tanks.len(),
        2,
        "two subfolders should produce two Tankoubons"
    );

    let (status, tanks) = get_json(&app, "/api/tankoubons?page=-1", Some(&cookie)).await;
    assert_eq!(status, axum::http::StatusCode::OK);
    let tanks = tanks["result"].as_array().unwrap();
    assert!(tanks.len() >= 2);

    let my_series = tanks
        .iter()
        .find(|t| {
            let archives = t["archives"].as_array().unwrap();
            archives.iter().any(|a| a.as_str() == Some(id_a.as_str()))
                && archives.iter().any(|a| a.as_str() == Some(id_b.as_str()))
                && archives.iter().any(|a| a.as_str() == Some(id_d.as_str()))
        })
        .expect(
            "a Tankoubon containing all 'My Series' archives, including nested ones, should exist",
        );
    let my_archives = my_series["archives"].as_array().unwrap();
    assert!(my_archives
        .iter()
        .any(|a| a.as_str() == Some(id_a.as_str())));
    assert!(my_archives
        .iter()
        .any(|a| a.as_str() == Some(id_b.as_str())));
    assert!(my_archives
        .iter()
        .any(|a| a.as_str() == Some(id_d.as_str())));

    let another = tanks
        .iter()
        .find(|t| {
            let archives = t["archives"].as_array().unwrap();
            archives.len() == 1 && archives.iter().any(|a| a.as_str() == Some(id_c.as_str()))
        })
        .expect("a Tankoubon containing the 'Another Series' archive should exist");
    let another_archives = another["archives"].as_array().unwrap();
    assert!(another_archives
        .iter()
        .any(|a| a.as_str() == Some(id_c.as_str())));

    let tank_repo = lanrurugi_storage::repository::GroupingRepository::new(redis.archive.clone());
    for tank_id in created_tanks.iter() {
        tank_repo
            .delete(&lanrurugi_core::ids::TankId(
                tank_id.as_str().unwrap().to_string(),
            ))
            .await
            .unwrap();
    }
    for id in [id_a, id_b, id_c, id_d] {
        archive_repo
            .delete(&lanrurugi_core::ids::ArchiveId(id))
            .await
            .unwrap();
    }
}
