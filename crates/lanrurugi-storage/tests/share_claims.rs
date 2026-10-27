//! The claim script against real Redis (017 FR-015, FR-004). Skipped when
//! `LANRURUGI_TEST_REDIS_URL` is unset or unreachable.

use std::net::IpAddr;

use lanrurugi_core::ids::{ArchiveId, ShareId};
use lanrurugi_share_proto::messages::Permissions;
use lanrurugi_storage::shares::{
    ClaimKind, ClaimOutcome, ClaimRefusal, Share, ShareItem, ShareRepository,
};
use lanrurugi_storage::test_support::{test_pool, RedisTestLock};

fn share(now: i64) -> Share {
    Share {
        id: ShareId::generate(),
        title: None,
        code_hash: format!("test-{}", ShareId::generate()),
        short_code: false,
        code_enc: "00".into(),
        lookup_pubkey: "00".into(),
        record_key: "00".into(),
        items: vec![ShareItem::Archive {
            id: ArchiveId::from("claim-test-archive"),
        }],
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
        created_at: now,
        created_by: "test".into(),
    }
}

fn ip(n: u8) -> IpAddr {
    IpAddr::from([198, 51, 100, n])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn claims_count_once_and_respect_limits() {
    let Some(pool) = test_pool().await else {
        eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set or unreachable");
        return;
    };
    let lock = RedisTestLock::acquire(&pool, "share_claims").await;
    let repo = ShareRepository::new(pool.clone());
    let now = 1_000;

    // Twenty simultaneous claims for one remaining download: exactly one succeeds.
    let mut limited = share(now);
    limited.download_limit = Some(1);
    repo.create(&limited).await.unwrap();
    let handles: Vec<_> = (0..20)
        .map(|i| {
            let repo = repo.clone();
            let s = limited.clone();
            tokio::spawn(async move {
                repo.claim(&s, ClaimKind::BrowserDownload, ip(i), None, None, now)
                    .await
                    .unwrap()
            })
        })
        .collect();
    let mut created = 0;
    for h in handles {
        match h.await.unwrap() {
            ClaimOutcome::Created(_) => created += 1,
            ClaimOutcome::Refused(ClaimRefusal::Exhausted) => {}
            other => panic!("unexpected outcome {other:?}"),
        }
    }
    assert_eq!(created, 1);
    assert_eq!(
        repo.get(&limited.id).await.unwrap().unwrap().downloads_used,
        1
    );

    // Claiming again under the same claim id never counts twice.
    let open = share(now);
    repo.create(&open).await.unwrap();
    let ClaimOutcome::Created(claim) = repo
        .claim(&open, ClaimKind::Import, ip(1), None, Some("ep"), now)
        .await
        .unwrap()
    else {
        panic!("first claim should be created");
    };
    let again = repo
        .claim(
            &open,
            ClaimKind::Import,
            ip(2),
            Some(claim.clone()),
            Some("ep"),
            now + 5,
        )
        .await
        .unwrap();
    assert_eq!(again, ClaimOutcome::Existing(claim));
    assert_eq!(repo.get(&open.id).await.unwrap().unwrap().downloads_used, 1);
    assert_eq!(repo.claim_history(&open.id, 10).await.unwrap().len(), 1);

    // First-N locks the share to the first address.
    let mut first = share(now);
    first.ip_first_n = Some(1);
    repo.create(&first).await.unwrap();
    assert!(matches!(
        repo.claim(&first, ClaimKind::BrowserDownload, ip(10), None, None, now)
            .await
            .unwrap(),
        ClaimOutcome::Created(_)
    ));
    assert_eq!(
        repo.claim(&first, ClaimKind::BrowserDownload, ip(11), None, None, now)
            .await
            .unwrap(),
        ClaimOutcome::Refused(ClaimRefusal::NotFound)
    );
    assert!(repo.admit_address(&first, ip(10)).await.unwrap());
    assert!(!repo.admit_address(&first, ip(11)).await.unwrap());

    // A revoked share refuses; a missing permission is reported as such.
    let revoked = share(now);
    repo.create(&revoked).await.unwrap();
    repo.revoke(&revoked.id, now).await.unwrap();
    assert_eq!(
        repo.claim(&revoked, ClaimKind::Import, ip(1), None, None, now)
            .await
            .unwrap(),
        ClaimOutcome::Refused(ClaimRefusal::Revoked)
    );
    let mut read_only = share(now);
    read_only.permissions = Permissions {
        import: false,
        read: true,
        download: false,
    };
    repo.create(&read_only).await.unwrap();
    assert_eq!(
        repo.claim(&read_only, ClaimKind::Import, ip(1), None, None, now)
            .await
            .unwrap(),
        ClaimOutcome::Refused(ClaimRefusal::PermissionDenied)
    );

    // The allow-list refuses outsiders with the same answer as an unknown share.
    let mut allow = share(now);
    allow.ip_allow = vec!["203.0.113.0/24".into()];
    repo.create(&allow).await.unwrap();
    assert_eq!(
        repo.claim(&allow, ClaimKind::Import, ip(1), None, None, now)
            .await
            .unwrap(),
        ClaimOutcome::Refused(ClaimRefusal::NotFound)
    );
    assert_eq!(
        repo.by_code_hash(&allow.code_hash)
            .await
            .unwrap()
            .unwrap()
            .id,
        allow.id
    );

    lock.release().await;
}
