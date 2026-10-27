//! 017 recipient routes end to end through the real router (US1, FR-012–FR-015, FR-013a/b).
//! Requires `LANRURUGI_TEST_REDIS_URL`; skips otherwise.
//!
//! Requests arrive from `MockConnectInfo`'s loopback peer, which the share routes treat as a local
//! reverse proxy, so each test picks the recipient's address through `X-Forwarded-For`.

mod common;

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_api::Repositories;
use lanrurugi_core::entities::{Archive, Grouping};
use lanrurugi_core::ids::{ArchiveId, ShareId, TankId};
use lanrurugi_share_proto::code::ShareCode;
use lanrurugi_share_proto::derive::access_hash;
use lanrurugi_share_proto::messages::Permissions;
use lanrurugi_storage::keys::CONFIG_KEY;
use lanrurugi_storage::redis::RedisDbs;
use lanrurugi_storage::shares::{Share, ShareItem, ShareRepository};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn test_app() -> Option<(axum::Router, RedisDbs)> {
    let (state, redis) = common::test_state().await?;
    Some((common::app(state), redis))
}

fn archive(id: &str, file: &std::path::Path) -> Archive {
    Archive {
        id: ArchiveId(id.to_string()),
        name: id.to_string(),
        title: format!("title {id}"),
        file: file.to_string_lossy().into_owned(),
        tags: "artist:test".to_string(),
        summary: String::new(),
        arcsize: 5,
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A share stored directly, with a known code.
async fn make_share(
    redis: &RedisDbs,
    items: Vec<ShareItem>,
    edit: impl FnOnce(&mut Share),
) -> (Share, String) {
    let code = ShareCode::generate(false);
    let mut share = Share {
        id: ShareId::generate(),
        title: Some("test share".into()),
        code_hash: hex(&access_hash(&code)),
        short_code: false,
        code_enc: "00".into(),
        lookup_pubkey: "00".into(),
        record_key: "00".into(),
        items,
        permissions: Permissions::ALL,
        download_limit: None,
        downloads_used: 0,
        ip_allow: vec![],
        ip_first_n: None,
        expires_at: None,
        embed: false,
        allow_helpers: false,
        helpers_disabled_at: None,
        revoked_at: None,
        created_at: 1,
        created_by: "test".into(),
    };
    edit(&mut share);
    ShareRepository::new(redis.config.clone())
        .create(&share)
        .await
        .unwrap();
    (share, code.display())
}

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    from: &str,
    body: Option<Value>,
) -> axum::http::Response<axum::body::Body> {
    let mut builder = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("X-Forwarded-For", from);
    let body = match body {
        Some(v) => {
            builder = builder.header("Content-Type", "application/json");
            axum::body::Body::from(v.to_string())
        }
        None => axum::body::Body::empty(),
    };
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}

async fn json_body(response: axum::http::Response<axum::body::Body>) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Opens a share and returns the recipient token.
async fn open(app: &axum::Router, code: &str, from: &str) -> Option<String> {
    let response = send(
        app,
        "POST",
        "/api/share/r/open",
        from,
        Some(json!({ "code": code })),
    )
    .await;
    if !response.status().is_success() {
        return None;
    }
    json_body(response).await["token"]
        .as_str()
        .map(str::to_string)
}

struct Library {
    _dir: tempfile::TempDir,
    a: ArchiveId,
    b: ArchiveId,
    outside: ArchiveId,
    tank: TankId,
}

async fn library(redis: &RedisDbs) -> Library {
    let dir = tempfile::tempdir().unwrap();
    let suffix = ShareId::generate().to_string();
    let repos = Repositories::new(redis);
    let mut ids = Vec::new();
    for name in ["a", "b", "outside"] {
        let id = format!("{name}{suffix}");
        let path = dir.path().join(format!("{id}.cbz"));
        std::fs::write(&path, format!("bytes of {id}")).unwrap();
        repos.archives.save(&archive(&id, &path)).await.unwrap();
        ids.push(ArchiveId(id));
    }
    let tank = TankId(format!("TANK_{suffix}"));
    repos
        .groupings
        .save(&Grouping {
            tankid: tank.clone(),
            name: "tank".into(),
            summary: String::new(),
            tags: String::new(),
            progress: 0,
            archives: vec![ids[1].clone()],
            thumbnail_manual: false,
            thumbnail_source_archive: None,
            thumbnail_source_page: None,
            chapter_names: vec![],
            created_at: None,
            updated_at: None,
        })
        .await
        .unwrap();
    Library {
        _dir: dir,
        a: ids[0].clone(),
        b: ids[1].clone(),
        outside: ids[2].clone(),
        tank,
    }
}

#[tokio::test]
async fn recipients_browse_read_and_download_only_within_the_share() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    // Guest mode off: the code alone must grant access (FR-013a).
    let _: () = redis
        .config
        .get()
        .await
        .unwrap()
        .hset(CONFIG_KEY, "guestmode", "0")
        .await
        .unwrap();
    let lib = library(&redis).await;
    let (_, code) = make_share(
        &redis,
        vec![
            ShareItem::Archive { id: lib.a.clone() },
            ShareItem::Tankoubon {
                id: lib.tank.clone(),
            },
        ],
        |s| s.download_limit = Some(2),
    )
    .await;

    let from = "198.51.100.10";
    let token = open(&app, &code, from).await.expect("a valid code opens");
    let manifest = send(
        &app,
        "GET",
        &format!("/api/share/r/manifest?rt={token}"),
        from,
        None,
    )
    .await;
    assert_eq!(manifest.status(), 200);
    assert_eq!(
        manifest.headers().get("referrer-policy").unwrap(),
        "no-referrer"
    );
    let manifest = json_body(manifest).await;
    assert_eq!(manifest["manifest"]["items"].as_array().unwrap().len(), 2);

    // Inside the share: the tankoubon member downloads. Outside: the same 404 as no token.
    let inside = send(
        &app,
        "GET",
        &format!("/api/share/r/archives/{}/download?rt={token}", lib.b),
        from,
        None,
    )
    .await;
    assert_eq!(inside.status(), 200);
    let outside = send(
        &app,
        "GET",
        &format!("/api/share/r/archives/{}/download?rt={token}", lib.outside),
        from,
        None,
    )
    .await;
    assert_eq!(outside.status(), 404);
    let no_token = send(
        &app,
        "GET",
        &format!("/api/share/r/archives/{}/download", lib.a),
        from,
        None,
    )
    .await;
    assert_eq!(no_token.status(), 404);

    // Downloads count per token: a second download on the same token is free, and a second token
    // is the second claim, which uses up the limit.
    let again = send(
        &app,
        "GET",
        &format!("/api/share/r/archives/{}/download?rt={token}", lib.a),
        from,
        None,
    )
    .await;
    assert_eq!(again.status(), 200);
    let second = open(&app, &code, "198.51.100.11").await.unwrap();
    let ok = send(
        &app,
        "GET",
        &format!("/api/share/r/archives/{}/download?rt={second}", lib.a),
        "198.51.100.11",
        None,
    )
    .await;
    assert_eq!(ok.status(), 200);
    // A used-up share closes to browsers: opening it says why, and earlier tokens browse nothing.
    let reopened = send(
        &app,
        "POST",
        "/api/share/r/open",
        "198.51.100.12",
        Some(json!({ "code": code })),
    )
    .await;
    assert_eq!(reopened.status(), 403);
    assert_eq!(json_body(reopened).await["error"], "exhausted");
    let browse = send(
        &app,
        "GET",
        &format!("/api/share/r/manifest?rt={second}"),
        "198.51.100.11",
        None,
    )
    .await;
    assert_eq!(browse.status(), 404);

    // A wrong code is the same 404 as anything else.
    assert!(open(&app, "AAAAA-AAAAA", from).await.is_none());
}

#[tokio::test]
async fn permissions_ip_rules_and_revocation_are_enforced() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let lib = library(&redis).await;

    // Without read permission, the page list is refused with 403 (the token itself is valid).
    let (_, code) = make_share(
        &redis,
        vec![ShareItem::Archive { id: lib.a.clone() }],
        |s| {
            s.permissions = Permissions {
                import: true,
                read: false,
                download: false,
            };
        },
    )
    .await;
    let from = "198.51.100.20";
    let token = open(&app, &code, from).await.unwrap();
    let pages = send(
        &app,
        "GET",
        &format!("/api/share/r/archives/{}/pages?rt={token}", lib.a),
        from,
        None,
    )
    .await;
    assert_eq!(pages.status(), 403);
    let download = send(
        &app,
        "GET",
        &format!("/api/share/r/archives/{}/download?rt={token}", lib.a),
        from,
        None,
    )
    .await;
    assert_eq!(download.status(), 403);

    // An allow-list refuses outsiders with 404 at open, and an existing token from a new address.
    let (allowed, code) = make_share(
        &redis,
        vec![ShareItem::Archive { id: lib.a.clone() }],
        |s| {
            s.ip_allow = vec!["203.0.113.0/24".into()];
        },
    )
    .await;
    assert!(open(&app, &code, "198.51.100.21").await.is_none());
    let token = open(&app, &code, "203.0.113.5")
        .await
        .expect("allowed address opens");
    let moved = send(
        &app,
        "GET",
        &format!("/api/share/r/manifest?rt={token}"),
        "198.51.100.22",
        None,
    )
    .await;
    assert_eq!(moved.status(), 404);

    // Revocation applies to the very next request.
    ShareRepository::new(redis.config.clone())
        .revoke(&allowed.id, 2)
        .await
        .unwrap();
    let after = send(
        &app,
        "GET",
        &format!("/api/share/r/manifest?rt={token}"),
        "203.0.113.5",
        None,
    )
    .await;
    assert_eq!(after.status(), 404);
}

#[tokio::test]
async fn share_management_is_refused_to_anonymous_callers() {
    let Some((app, _redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let list = send(&app, "GET", "/api/shares", "198.51.100.30", None).await;
    assert!(
        list.status() == 401 || list.status() == 403,
        "{}",
        list.status()
    );
}

#[tokio::test]
async fn the_embed_card_shows_only_what_the_share_allows() {
    let Some((app, redis)) = test_app().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let lib = library(&redis).await;
    let from = "198.51.100.40";
    let card = |code: &str| format!("/api/share/r/card?code={code}");

    let (_, off) = make_share(
        &redis,
        vec![ShareItem::Archive { id: lib.a.clone() }],
        |_| {},
    )
    .await;
    assert_eq!(
        send(&app, "GET", &card(&off), from, None).await.status(),
        404
    );

    let (_, on) = make_share(
        &redis,
        vec![ShareItem::Archive { id: lib.a.clone() }],
        |s| s.embed = true,
    )
    .await;
    let body = json_body(send(&app, "GET", &card(&on), from, None).await).await;
    assert_eq!(body["title"], "test share");
    assert_eq!(body["covers"][0], lib.a.to_string());
    assert!(body["token"].is_string());

    let (_, restricted) = make_share(
        &redis,
        vec![ShareItem::Archive { id: lib.a.clone() }],
        |s| {
            s.embed = true;
            s.ip_allow = vec!["203.0.113.0/24".into()];
        },
    )
    .await;
    let body = json_body(send(&app, "GET", &card(&restricted), from, None).await).await;
    assert_eq!(body["restricted"], true);
    assert!(body.get("title").is_none() && body.get("covers").is_none());
}
