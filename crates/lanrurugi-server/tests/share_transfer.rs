//! 017 share protocol end to end over real iroh connections on loopback (US4, FR-022, SC-009):
//! a sharer with a full AppState, and a bare endpoint acting as the importer. Requires
//! `LANRURUGI_TEST_REDIS_URL`; skips otherwise.

mod common;

use bao_tree::io::outboard::PreOrderOutboard;
use bao_tree::{BaoTree, ChunkNum, ChunkRanges};
use deadpool_redis::redis::AsyncCommands;
use iroh::endpoint::{presets, Connection};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use lanrurugi_api::share_net::{self, call, read_body};
use lanrurugi_core::entities::Archive;
use lanrurugi_core::ids::ShareImportId;
use lanrurugi_core::ids::{ArchiveId, ShareId};
use lanrurugi_share_proto::code::ShareCode;
use lanrurugi_share_proto::derive::access_hash;
use lanrurugi_share_proto::derive::DerivedKeys;
use lanrurugi_share_proto::messages::{
    ChunkSpan, ClaimKind, HelpReason, HelpState, HelperAddr, HelperStatus, ManifestItem,
    Permissions, RefusalReason, Request, Response,
};
use lanrurugi_share_proto::record::{self, KnockV1};
use lanrurugi_storage::keys::CONFIG_KEY;
use lanrurugi_storage::shares::{
    HelpState as LocalHelpState, ImportArchive, ImportArchiveState, ImportState, Share,
    ShareImport, ShareImportRepository, ShareItem, ShareRepository,
};
use lanrurugi_storage::test_support::RedisTestLock;

const BLOCK: bao_tree::BlockSize = bao_tree::BlockSize::from_chunk_log(4);

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

async fn importer() -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .bind()
        .await
        .unwrap()
}

async fn dial(client: &Endpoint, sharer: &share_net::ShareNet) -> Connection {
    let port = sharer
        .endpoint()
        .bound_sockets()
        .into_iter()
        .find(|s| s.is_ipv4())
        .expect("an IPv4 socket")
        .port();
    let addr = EndpointAddr::new(sharer.id())
        .with_ip_addr(std::net::SocketAddr::from(([127, 0, 0, 1], port)));
    client.connect(addr, share_net::ALPN).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_archive_transfers_verified_and_revocation_stops_renewal() {
    let Some((mut state, redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let lock = RedisTestLock::acquire(&redis.config, "share_public_services").await;
    let _: () = redis
        .config
        .get()
        .await
        .unwrap()
        .hset(CONFIG_KEY, "share_public_services", "0")
        .await
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    state.library.temp_dir = dir.path().to_path_buf();

    // A 3.5 MiB archive: several 1 MiB pieces plus a partial one.
    let data: Vec<u8> = (0..3_600_000u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    let file = dir.path().join("archive.zip");
    std::fs::write(&file, &data).unwrap();
    let archive_id = format!("transfer{}", ShareId::generate());
    state
        .repos
        .archives
        .save(&Archive {
            id: ArchiveId(archive_id.clone()),
            name: "archive".into(),
            title: "archive".into(),
            file: file.to_string_lossy().into_owned(),
            tags: String::new(),
            summary: String::new(),
            arcsize: data.len() as u64,
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
    let code = ShareCode::generate(false);
    let share = Share {
        id: ShareId::generate(),
        title: Some("transfer".into()),
        code_hash: hex(&access_hash(&code)),
        short_code: false,
        code_enc: "00".into(),
        lookup_pubkey: "00".into(),
        record_key: "00".into(),
        items: vec![ShareItem::Archive {
            id: ArchiveId(archive_id.clone()),
        }],
        permissions: Permissions::ALL,
        download_limit: Some(1),
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
    let shares = ShareRepository::new(redis.config.clone());
    shares.create(&share).await.unwrap();

    let settings = lanrurugi_api::share_settings::load(&state).await;
    let sharer = share_net::bind(&state, &settings, rand::random(), 0)
        .await
        .unwrap();
    let client = importer().await;
    let conn = dial(&client, &sharer).await;

    // Open, then a wrong code is refused as not found.
    let Ok((
        Response::Opened {
            session, manifest, ..
        },
        _,
    )) = call(
        &conn,
        Request::Open {
            access_hash: access_hash(&code),
        },
    )
    .await
    else {
        panic!("open should succeed");
    };
    assert!(matches!(manifest.items[0], ManifestItem::Archive(_)));
    let wrong = call(
        &conn,
        Request::Open {
            access_hash: [0; 32],
        },
    )
    .await;
    assert!(matches!(
        wrong,
        Err(share_net::ShareNetError::Refused(RefusalReason::NotFound))
    ));

    // Claim counts the one allowed download; the archive is not yet fingerprinted, so prepare it.
    let Ok((Response::Claimed { claim_id, .. }, _)) = call(
        &conn,
        Request::Claim {
            session: session.clone(),
            kind: ClaimKind::Import,
        },
    )
    .await
    else {
        panic!("claim should succeed");
    };
    let Ok((Response::Prepared { blake3, size }, _)) = call(
        &conn,
        Request::Prepare {
            session: session.clone(),
            claim_id: claim_id.clone(),
            archive_id: archive_id.clone(),
        },
    )
    .await
    else {
        panic!("prepare should succeed");
    };
    assert_eq!(blake3, *blake3::hash(&data).as_bytes());
    assert_eq!(size, data.len() as u64);

    // The limit is now used up, but an existing claim keeps renewing (FR-015).
    let Ok((Response::Renewed { ticket, .. }, _)) = call(
        &conn,
        Request::Renew {
            claim_id: claim_id.clone(),
        },
    )
    .await
    else {
        panic!("renew should succeed");
    };

    // Fetch every piece and verify it against the root.
    let tree = BaoTree::new(size, BLOCK);
    let mut received = vec![0u8; size as usize];
    let mut outboard = PreOrderOutboard {
        root: blake3::Hash::from_bytes(blake3),
        tree,
        data: vec![0u8; tree.outboard_size() as usize],
    };
    let total_chunks = tree.chunks().0;
    let mut start = 0;
    let mut tampered_once = false;
    while start < total_chunks {
        let end = (start + 64).min(total_chunks);
        let (Response::Bytes { len, .. }, mut recv) = call(
            &conn,
            Request::Range {
                ticket: ticket.clone(),
                blake3,
                spans: vec![ChunkSpan { start, end }],
            },
        )
        .await
        .unwrap() else {
            panic!("range should answer with bytes");
        };
        let mut body = read_body(&mut recv, len, 8 * 1024 * 1024).await.unwrap();
        let ranges = ChunkRanges::from(ChunkNum(start)..ChunkNum(end));
        if !tampered_once {
            // A flipped byte must be rejected before anything is written.
            let mut bad = body.clone();
            let at = bad.len() - 10;
            bad[at] ^= 0xff;
            let mut scratch = vec![0u8; size as usize];
            let mut scratch_ob = outboard.clone();
            assert!(bao_tree::io::sync::decode_ranges(
                std::io::Cursor::new(bad),
                &ranges,
                &mut scratch,
                &mut scratch_ob
            )
            .is_err());
            tampered_once = true;
        }
        bao_tree::io::sync::decode_ranges(
            std::io::Cursor::new(std::mem::take(&mut body)),
            &ranges,
            &mut received,
            &mut outboard,
        )
        .unwrap();
        start = end;
    }
    assert_eq!(received, data);

    // Another endpoint cannot use this ticket.
    let other = importer().await;
    let other_conn = dial(&other, &sharer).await;
    let stolen = call(
        &other_conn,
        Request::Range {
            ticket: ticket.clone(),
            blake3,
            spans: vec![ChunkSpan { start: 0, end: 1 }],
        },
    )
    .await;
    assert!(stolen.is_err());

    // After revocation, renewal is refused, which stops every source.
    shares.revoke(&share.id, 2).await.unwrap();
    let renewed = call(&conn, Request::Renew { claim_id }).await;
    assert!(matches!(
        renewed,
        Err(share_net::ShareNetError::Refused(RefusalReason::NotFound))
    ));

    let _: () = redis
        .config
        .get()
        .await
        .unwrap()
        .hdel(CONFIG_KEY, "share_public_services")
        .await
        .unwrap();
    lock.release().await;
}

fn archive_record(id: &str, file: &std::path::Path, size: u64) -> Archive {
    Archive {
        id: ArchiveId(id.to_string()),
        name: id.into(),
        title: id.into(),
        file: file.to_string_lossy().into_owned(),
        tags: String::new(),
        summary: String::new(),
        arcsize: size,
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

fn loopback_addr(net: &share_net::ShareNet) -> String {
    let port = net
        .endpoint()
        .bound_sockets()
        .into_iter()
        .find(|s| s.is_ipv4())
        .expect("an IPv4 socket")
        .port();
    format!("127.0.0.1:{port}")
}

async fn open_and_claim(conn: &Connection, code: &ShareCode) -> (String, String, Vec<HelperAddr>) {
    let Ok((Response::Opened { session, .. }, _)) = call(
        conn,
        Request::Open {
            access_hash: access_hash(code),
        },
    )
    .await
    else {
        panic!("open should succeed");
    };
    let Ok((
        Response::Claimed {
            claim_id, helpers, ..
        },
        _,
    )) = call(
        conn,
        Request::Claim {
            session: session.clone(),
            kind: ClaimKind::Import,
        },
    )
    .await
    else {
        panic!("claim should succeed");
    };
    (session, claim_id, helpers)
}

async fn helper_status(conn: &Connection, share: &Share) -> HelperStatus {
    match call(
        conn,
        Request::HelperHeartbeat {
            share_id: share.id.to_string(),
        },
    )
    .await
    {
        Ok((Response::HelperStatus(s), _)) => s,
        other => panic!("heartbeat should answer a status, got {other:?}"),
    }
}

/// US7 (T094, T098): helper registration rules, helper-served ranges, and the status a helper
/// receives as the share changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn helpers_serve_verified_ranges_and_follow_the_share() {
    let Some((mut state, redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let lock = RedisTestLock::acquire(&redis.config, "share_public_services").await;
    let mut config = redis.config.get().await.unwrap();
    let _: () = config
        .hset(CONFIG_KEY, "share_public_services", "0")
        .await
        .unwrap();
    let _: () = config
        .hset(CONFIG_KEY, "share_helper_max_peers", "1")
        .await
        .unwrap();
    drop(config);
    let dir = tempfile::tempdir().unwrap();
    state.library.temp_dir = dir.path().to_path_buf();

    // Two archives on the sharer; the helper holds its own copy of the first.
    let data: Vec<u8> = (0..2_500_000u32)
        .map(|i| (i.wrapping_mul(2_246_822_519) >> 20) as u8)
        .collect();
    let tag = ShareId::generate();
    let (a, b, local) = (
        format!("helpa{tag}"),
        format!("helpb{tag}"),
        format!("helpl{tag}"),
    );
    for (id, name) in [(&a, "a.zip"), (&b, "b.zip"), (&local, "local.zip")] {
        let file = dir.path().join(name);
        std::fs::write(&file, &data[..if id == &b { 10_000 } else { data.len() }]).unwrap();
        let size = std::fs::metadata(&file).unwrap().len();
        state
            .repos
            .archives
            .save(&archive_record(id, &file, size))
            .await
            .unwrap();
    }
    let code = ShareCode::generate(false);
    let mut share = Share {
        id: ShareId::generate(),
        title: Some("helped".into()),
        code_hash: hex(&access_hash(&code)),
        short_code: false,
        code_enc: "00".into(),
        lookup_pubkey: "00".into(),
        record_key: "00".into(),
        items: vec![
            ShareItem::Archive {
                id: ArchiveId(a.clone()),
            },
            ShareItem::Archive {
                id: ArchiveId(b.clone()),
            },
        ],
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
    let shares = ShareRepository::new(redis.config.clone());
    shares.create(&share).await.unwrap();

    let settings = lanrurugi_api::share_settings::load(&state).await;
    let sharer = share_net::bind(&state, &settings, rand::random(), 0)
        .await
        .unwrap();
    let helper = share_net::bind(&state, &settings, rand::random(), 0)
        .await
        .unwrap();
    let helper_to_sharer = dial(helper.endpoint(), &sharer).await;
    let register = |archives: Vec<String>| Request::HelperRegister {
        share_id: share.id.to_string(),
        archives,
        relay_urls: vec![],
        addrs: vec![loopback_addr(&helper)],
    };

    // Only an endpoint that claimed may register, and only while the share allows helpers.
    let early = call(&helper_to_sharer, register(vec![a.clone(), b.clone()])).await;
    assert!(matches!(
        early,
        Err(share_net::ShareNetError::Refused(RefusalReason::NotFound))
    ));
    open_and_claim(&helper_to_sharer, &code).await;
    let refused = call(&helper_to_sharer, register(vec![a.clone(), b.clone()])).await;
    assert!(matches!(
        refused,
        Err(share_net::ShareNetError::Refused(
            RefusalReason::PermissionDenied
        ))
    ));
    let items = share.items.clone();
    share.allow_helpers = true;
    shares.save(&share, &items).await.unwrap();
    let Ok((Response::HelperStatus(status), _)) =
        call(&helper_to_sharer, register(vec![a.clone(), b.clone()])).await
    else {
        panic!("registration should be accepted");
    };
    assert_eq!(status.state, HelpState::Active);

    // The helper's own record of the import, finished and helping.
    let root = *blake3::hash(&data).as_bytes();
    let import = ShareImport {
        id: ShareImportId::generate(),
        code: Some("unused".into()),
        sharer_endpoint_id: hex(sharer.id().as_bytes()),
        share_id: share.id.to_string(),
        claim_id: String::new(),
        title: "helped".into(),
        manifest: serde_json::Value::Null,
        category: None,
        queue_item_id: String::new(),
        archives: vec![ImportArchive {
            sharer_archive_id: a.clone(),
            title: String::new(),
            blake3: Some(hex(&root)),
            size: data.len() as u64,
            state: ImportArchiveState::Done,
            bytes_done: data.len() as u64,
            error: None,
            note: None,
            local_archive_id: Some(local.clone()),
        }],
        concurrency: 1,
        help_deliver: true,
        help_hidden: false,
        help_state: LocalHelpState::Active,
        help_reason: None,
        help_state_at: None,
        help_last_contact_at: None,
        help_removed_archives: vec![],
        help_bytes_served: 0,
        inherit_categories: false,
        state: ImportState::Done,
        error: None,
        warnings: Vec::new(),
        filename_policy: Default::default(),
        paused_until: None,
        created_at: 1,
    };
    let imports = ShareImportRepository::new(redis.config.clone());
    imports.save(&import).await.unwrap();

    // An importer learns about the helper from its claim and from every renewal.
    let client = importer().await;
    let conn = dial(&client, &sharer).await;
    let (session, claim_id, helpers) = open_and_claim(&conn, &code).await;
    assert_eq!(helpers.len(), 1);
    assert_eq!(helpers[0].endpoint_id, *helper.id().as_bytes());
    assert_eq!(helpers[0].addrs, vec![loopback_addr(&helper)]);
    let Ok((Response::Prepared { blake3, size }, _)) = call(
        &conn,
        Request::Prepare {
            session: session.clone(),
            claim_id: claim_id.clone(),
            archive_id: a.clone(),
        },
    )
    .await
    else {
        panic!("prepare should succeed");
    };
    assert_eq!(blake3, root);
    let Ok((Response::Renewed { ticket, helpers }, _)) = call(
        &conn,
        Request::Renew {
            claim_id: claim_id.clone(),
        },
    )
    .await
    else {
        panic!("renew should succeed");
    };
    assert_eq!(helpers.len(), 1);

    // The whole archive from the helper alone, every piece verified against the sharer's root.
    let to_helper = dial(&client, &helper).await;
    let tree = BaoTree::new(size, BLOCK);
    let mut received = vec![0u8; size as usize];
    let mut outboard = PreOrderOutboard {
        root: blake3::Hash::from_bytes(blake3),
        tree,
        data: vec![0u8; tree.outboard_size() as usize],
    };
    let total_chunks = tree.chunks().0;
    let mut start = 0;
    while start < total_chunks {
        let end = (start + 64).min(total_chunks);
        let (Response::Bytes { len, .. }, mut recv) = call(
            &to_helper,
            Request::Range {
                ticket: ticket.clone(),
                blake3,
                spans: vec![ChunkSpan { start, end }],
            },
        )
        .await
        .unwrap() else {
            panic!("the helper should answer with bytes");
        };
        let body = read_body(&mut recv, len, 8 * 1024 * 1024).await.unwrap();
        let ranges = ChunkRanges::from(ChunkNum(start)..ChunkNum(end));
        bao_tree::io::sync::decode_ranges(
            std::io::Cursor::new(body),
            &ranges,
            &mut received,
            &mut outboard,
        )
        .unwrap();
        start = end;
    }
    assert_eq!(received, data);

    // Helpers never serve outboards, and refuse a ticket issued to someone else.
    let outboard_ask = call(
        &to_helper,
        Request::Outboard {
            ticket: ticket.clone(),
            blake3,
        },
    )
    .await;
    assert!(outboard_ask.is_err());
    let thief = importer().await;
    let thief_conn = dial(&thief, &helper).await;
    let stolen = call(
        &thief_conn,
        Request::Range {
            ticket: ticket.clone(),
            blake3,
            spans: vec![ChunkSpan { start: 0, end: 1 }],
        },
    )
    .await;
    assert!(stolen.is_err());

    // With one helping slot, a second importer waits while the first is served.
    let second = importer().await;
    let second_to_sharer = dial(&second, &sharer).await;
    let (_, second_claim, _) = open_and_claim(&second_to_sharer, &code).await;
    let Ok((
        Response::Renewed {
            ticket: second_ticket,
            ..
        },
        _,
    )) = call(
        &second_to_sharer,
        Request::Renew {
            claim_id: second_claim,
        },
    )
    .await
    else {
        panic!("renew should succeed");
    };
    let second_to_helper = dial(&second, &helper).await;
    let waiting = call(
        &second_to_helper,
        Request::Range {
            ticket: second_ticket,
            blake3,
            spans: vec![ChunkSpan { start: 0, end: 1 }],
        },
    )
    .await;
    assert!(matches!(
        waiting,
        Err(share_net::ShareNetError::Refused(
            RefusalReason::Paused { .. }
        ))
    ));

    // Status as the share changes: an archive removed, downloads used up and raised, revoked.
    let items = share.items.clone();
    share.items = vec![ShareItem::Archive {
        id: ArchiveId(a.clone()),
    }];
    shares.save(&share, &items).await.unwrap();
    let status = helper_status(&helper_to_sharer, &share).await;
    assert_eq!(
        (status.state, status.removed_archives.clone()),
        (HelpState::Active, vec![b.clone()])
    );

    let used = shares.get(&share.id).await.unwrap().unwrap().downloads_used;
    share.download_limit = Some(used);
    shares.save(&share, &share.items.clone()).await.unwrap();
    let status = helper_status(&helper_to_sharer, &share).await;
    assert_eq!(
        (status.state, status.reason),
        (HelpState::Paused, Some(HelpReason::Exhausted))
    );
    share.download_limit = Some(used + 5);
    shares.save(&share, &share.items.clone()).await.unwrap();
    assert_eq!(
        helper_status(&helper_to_sharer, &share).await.state,
        HelpState::Active
    );

    shares.revoke(&share.id, 4242).await.unwrap();
    let status = helper_status(&helper_to_sharer, &share).await;
    assert_eq!(
        (status.state, status.reason, status.at),
        (HelpState::Ended, Some(HelpReason::Revoked), 4242)
    );
    assert!(call(&conn, Request::Renew { claim_id }).await.is_err());

    // The helper stores the answer and stops for good: no code is kept to reconnect with.
    share_net::helper::apply_status(&state, &import.id, &status).await;
    let stored = imports.get(&import.id).await.unwrap().unwrap();
    assert_eq!(stored.help_state, LocalHelpState::Ended);
    assert_eq!(stored.help_reason.as_deref(), Some("revoked"));
    assert!(stored.code.is_none() && !stored.serves_help());
    let after = call(
        &to_helper,
        Request::Range {
            ticket,
            blake3,
            spans: vec![ChunkSpan { start: 0, end: 1 }],
        },
    )
    .await;
    assert!(after.is_err());

    let mut config = redis.config.get().await.unwrap();
    let _: () = config
        .hdel(
            CONFIG_KEY,
            &["share_public_services", "share_helper_max_peers"],
        )
        .await
        .unwrap();
    lock.release().await;
}

/// T077: a LANrurugi node with the rendezvous role stores signed knock packets it cannot read and
/// hands them back; a tampered packet is refused, and the record carries nothing but the endpoint
/// id and relay URLs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rendezvous_node_stores_and_returns_knock_records() {
    let Some((state, redis)) = common::test_state().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
        return;
    };
    let lock = RedisTestLock::acquire(&redis.config, "share_public_services").await;
    let mut config = redis.config.get().await.unwrap();
    let _: () = config
        .hset(CONFIG_KEY, "share_public_services", "0")
        .await
        .unwrap();
    let _: () = config
        .hset(CONFIG_KEY, "share_node_rendezvous", "1")
        .await
        .unwrap();
    drop(config);
    let settings = lanrurugi_api::share_settings::load(&state).await;
    let node = share_net::bind(&state, &settings, rand::random(), 0)
        .await
        .unwrap();

    let code = ShareCode::generate(false);
    let keys = DerivedKeys::derive(&code);
    let knock = KnockV1 {
        endpoint_id: [7; 32],
        relay_urls: vec!["https://relay.example.org/".into()],
    };
    let packet = record::seal(&keys, &knock).unwrap().as_bytes().to_vec();

    let sharer = importer().await;
    let to_node = dial(&sharer, &node).await;
    let put = call(
        &to_node,
        Request::RendezvousPut {
            signed_packet: packet.clone(),
        },
    )
    .await;
    assert!(matches!(put, Ok((Response::Ok, _))));
    let mut tampered = packet.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(call(
        &to_node,
        Request::RendezvousPut {
            signed_packet: tampered
        }
    )
    .await
    .is_err());

    let importer_ep = importer().await;
    let from_node = dial(&importer_ep, &node).await;
    let Ok((Response::Packet { signed_packet }, _)) = call(
        &from_node,
        Request::RendezvousGet {
            lookup_pubkey: keys.lookup_public(),
        },
    )
    .await
    else {
        panic!("the node should hand the packet back");
    };
    assert_eq!(signed_packet, packet);
    let opened = pkarr::SignedPacket::from_relay_payload(
        &keys.lookup_key.public_key(),
        &bytes::Bytes::copy_from_slice(&signed_packet[32..]),
    )
    .unwrap();
    assert_eq!(record::open(&keys, &opened).unwrap(), knock);
    let unknown = call(
        &from_node,
        Request::RendezvousGet {
            lookup_pubkey: [9; 32],
        },
    )
    .await;
    assert!(matches!(unknown, Ok((Response::NotHere, _))));

    let mut config = redis.config.get().await.unwrap();
    let _: () = config
        .hdel(
            CONFIG_KEY,
            &["share_public_services", "share_node_rendezvous"],
        )
        .await
        .unwrap();
    lock.release().await;
}
