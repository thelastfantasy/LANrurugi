//! The sharer's side of the share protocol (contracts/share-protocol.md "Requests"). Every request
//! re-checks the share's status, permissions and IP rules, so revocation and edits apply to the
//! very next request (SC-009).

use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use bao_tree::io::outboard::PreOrderOutboard;
use bao_tree::{BaoTree, ChunkNum, ChunkRanges};
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh::EndpointId;
use lanrurugi_core::ids::{ArchiveId, ClaimId, ShareId};
use lanrurugi_share_proto::messages::{
    ChunkSpan, Envelope, HelpReason, HelpState, HelperAddr, HelperStatus, ManifestItem,
    RefusalReason, Request, Response, ShareManifest, PROTO_VERSION,
};
use lanrurugi_share_proto::ticket::{SignedTicket, Ticket, TicketError, TICKET_TTL_SECS};
use lanrurugi_storage::shares::{
    ClaimKind, ClaimOutcome, ClaimRefusal, HelperInfo, Share, ShareStatus, HELP_OFFLINE_AFTER_SECS,
};
use sha2::{Digest, Sha256};

use super::{
    blobs, direct_address, read_frame, wait_for_direct_address, write_frame, Session, Shared,
    DIRECT_PATH_WAIT,
};
use crate::shares::{browse_manifest, hex, now_secs, repo};
use crate::AppState;

/// Largest single `Range` answer, so one request never buffers more than this.
const MAX_RANGE_BYTES: u64 = 8 * 1024 * 1024;
const CHUNK_BYTES: u64 = 1024;
/// Caps on what one helper registration may carry.
const MAX_HELPER_ADDRS: usize = 8;
const MAX_HELPER_ARCHIVES: usize = 10_000;

#[derive(Clone)]
pub struct ShareProtocol {
    pub(crate) state: AppState,
    pub(crate) shared: Arc<Shared>,
}

impl std::fmt::Debug for ShareProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShareProtocol")
    }
}

impl ProtocolHandler for ShareProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // Which route a peer used, for diagnosing NAT setups (T071); never the peer's content.
        let peer = super::endpoint_hex(&connection.remote_id());
        let route = if direct_address(&connection).is_some() {
            "direct"
        } else {
            "relay"
        };
        tracing::info!(peer = &peer[..12], route, "sharing connection");
        if route == "relay" {
            // Hole punching usually completes after the first exchange; log when it does.
            let conn = connection.clone();
            let peer = peer.clone();
            tokio::spawn(async move {
                if wait_for_direct_address(&conn, DIRECT_PATH_WAIT)
                    .await
                    .is_some()
                {
                    tracing::info!(
                        peer = &peer[..12],
                        route = "direct",
                        "sharing connection upgraded"
                    );
                }
            });
        }
        loop {
            let (send, recv) = match connection.accept_bi().await {
                Ok(streams) => streams,
                // The peer closed the connection; nothing more to serve.
                Err(_) => return Ok(()),
            };
            let this = self.clone();
            let conn = connection.clone();
            tokio::spawn(async move { this.serve_stream(conn, send, recv).await });
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn refusal(reason: ClaimRefusal) -> RefusalReason {
    match reason {
        ClaimRefusal::Revoked => RefusalReason::Revoked,
        ClaimRefusal::Expired => RefusalReason::Expired,
        ClaimRefusal::Exhausted => RefusalReason::Exhausted,
        ClaimRefusal::PermissionDenied => RefusalReason::PermissionDenied,
        ClaimRefusal::NotFound => RefusalReason::NotFound,
    }
}

/// Own-share uploads may be paused by the schedule (FR-042) or busy at their importer cap; the
/// importer keeps its progress and comes back at `until` (FR-044).
fn held(reason: super::limits::Held) -> RefusalReason {
    match reason {
        super::limits::Held::Paused { until } => RefusalReason::Paused {
            until: until.map(|t| t as u64),
        },
        super::limits::Held::Busy => RefusalReason::Paused {
            until: Some(unix_now() + 30),
        },
    }
}

impl ShareProtocol {
    async fn serve_stream(self, conn: Connection, mut send: SendStream, mut recv: RecvStream) {
        let envelope: Envelope = match read_frame(&mut recv).await {
            Ok(e) => e,
            Err(_) => return,
        };
        if envelope.proto != PROTO_VERSION {
            let _ = write_frame(
                &mut send,
                &Response::Refused(RefusalReason::UnsupportedVersion {
                    supported: vec![PROTO_VERSION],
                }),
            )
            .await;
            let _ = send.finish();
            return;
        }
        let remote = conn.remote_id();
        let outcome = self.handle(&conn, remote, envelope.request).await;
        let (header, body) = match outcome {
            Ok(answer) => answer,
            Err(reason) => (Response::Refused(reason), None),
        };
        if write_frame(&mut send, &header).await.is_ok() {
            if let Some(body) = body {
                let _ = send.write_all(&body).await;
            }
        }
        let _ = send.finish();
    }

    async fn handle(
        &self,
        conn: &Connection,
        remote: EndpointId,
        request: Request,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let request_copy = match &request {
            Request::RendezvousPut { .. } | Request::RendezvousGet { .. } => request.clone(),
            _ => Request::Renew {
                claim_id: String::new(),
            },
        };
        match request {
            Request::ResolveLocal { lookup_hash } => self.resolve_local(lookup_hash).await,
            Request::Open { access_hash } => self.open(conn, remote, access_hash).await,
            Request::Thumbnail {
                session,
                archive_id,
            } => self.thumbnail(remote, &session, archive_id).await,
            Request::Claim { session, .. } => self.claim(conn, remote, &session).await,
            Request::Prepare {
                session,
                claim_id,
                archive_id,
            } => self.prepare(remote, &session, &claim_id, archive_id).await,
            Request::Renew { claim_id } => self.renew(conn, remote, &claim_id).await,
            Request::Range {
                ticket,
                blake3,
                spans,
            } => self.range(remote, &ticket, blake3, &spans).await,
            Request::Outboard { ticket, blake3 } => self.outboard(remote, &ticket, blake3).await,
            Request::RendezvousPut { .. } | Request::RendezvousGet { .. } => {
                self.rendezvous(remote, &request_copy).await
            }
            Request::HelperRegister {
                share_id,
                archives,
                relay_urls,
                addrs,
            } => {
                self.helper_register(remote, share_id, archives, relay_urls, addrs)
                    .await
            }
            Request::HelperHeartbeat { share_id } => self.helper_heartbeat(remote, share_id).await,
        }
    }

    /// Status and IP rules for a request from `conn`. IP-restricted shares need the peer's own
    /// address, which a relayed path does not show (research §4).
    async fn admit(
        &self,
        conn: &Connection,
        share: &Share,
    ) -> Result<Option<IpAddr>, RefusalReason> {
        match share.status(now_secs()) {
            ShareStatus::Revoked | ShareStatus::Expired => return Err(RefusalReason::NotFound),
            _ => {}
        }
        if !share.has_ip_restriction() {
            return Ok(direct_address(conn));
        }
        let Some(ip) = wait_for_direct_address(conn, DIRECT_PATH_WAIT).await else {
            return Err(RefusalReason::AddressUnverifiable);
        };
        let admitted = share.ip_allowed(ip)
            && repo(&self.state)
                .admit_address(share, ip)
                .await
                .unwrap_or(false);
        if admitted {
            Ok(Some(ip))
        } else {
            Err(RefusalReason::NotFound)
        }
    }

    async fn share_for_session(
        &self,
        remote: EndpointId,
        session: &str,
    ) -> Result<(Session, Share), RefusalReason> {
        let session = self
            .shared
            .session(session, remote)
            .ok_or(RefusalReason::NotFound)?;
        let share = repo(&self.state)
            .get(&session.share_id)
            .await
            .ok()
            .flatten()
            .ok_or(RefusalReason::NotFound)?;
        Ok((session, share))
    }

    /// The rendezvous role (FR-048): only while this instance offers it.
    async fn rendezvous(
        &self,
        remote: EndpointId,
        request: &Request,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        static LIMITER: std::sync::OnceLock<super::nodes::RendezvousLimiter> =
            std::sync::OnceLock::new();
        let settings = crate::share_settings::load(&self.state).await;
        if !settings.flag("share_node_rendezvous") {
            return Err(RefusalReason::NotFound);
        }
        let store = super::nodes::RendezvousStore::Redis(self.state.redis.config.clone());
        let limiter = LIMITER.get_or_init(super::nodes::RendezvousLimiter::new);
        match super::nodes::answer_rendezvous(
            &store,
            limiter,
            &super::endpoint_hex(&remote),
            request,
        )
        .await
        {
            Some(Ok(response)) => Ok((response, None)),
            Some(Err(reason)) => Err(reason),
            None => Err(RefusalReason::NotFound),
        }
    }

    async fn resolve_local(
        &self,
        lookup_hash: [u8; 32],
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let shares = repo(&self.state).list().await.unwrap_or_default();
        let now = now_secs();
        let found = shares.iter().find(|s| {
            matches!(s.status(now), ShareStatus::Active | ShareStatus::Exhausted)
                && crate::shares::unhex(&s.lookup_pubkey)
                    .is_some_and(|key| Sha256::digest(&key).as_slice() == lookup_hash)
        });
        Ok(match found {
            Some(share) => (
                Response::Found {
                    share_id: share.id.to_string(),
                },
                None,
            ),
            None => (Response::NotHere, None),
        })
    }

    async fn open(
        &self,
        conn: &Connection,
        remote: EndpointId,
        access_hash: [u8; 32],
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let limiter = crate::share_attempts::shared();
        let source = super::endpoint_hex(&remote);
        if !limiter.allowed(&source, Instant::now()) {
            return Err(RefusalReason::RateLimited);
        }
        let share = repo(&self.state)
            .by_code_hash(&hex(&access_hash))
            .await
            .ok()
            .flatten();
        let admitted = match &share {
            Some(share) => self.admit(conn, share).await.map(|_| ()),
            None => Err(RefusalReason::NotFound),
        };
        if let Err(reason) = admitted {
            if reason == RefusalReason::NotFound {
                let penalty = limiter.record_failure(&source, Instant::now());
                limiter.apply(penalty).await;
            }
            return Err(reason);
        }
        let share = share.expect("admitted implies found");
        let token: [u8; 32] = rand::random();
        let token = hex(&token);
        self.shared.sessions.lock().expect("sessions lock").insert(
            token.clone(),
            Session {
                share_id: share.id.clone(),
                remote,
                created: Instant::now(),
            },
        );
        let settings = crate::share_settings::load(&self.state).await;
        Ok((
            Response::Opened {
                session: token,
                manifest: browse_manifest(&self.state, &share).await,
                public_url: settings.public_url(),
            },
            None,
        ))
    }

    async fn thumbnail(
        &self,
        remote: EndpointId,
        session: &str,
        archive_id: String,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let (_, share) = self.share_for_session(remote, session).await?;
        let manifest = browse_manifest(&self.state, &share).await;
        if !manifest.archives().any(|a| a.archive_id == archive_id) {
            return Err(RefusalReason::NotFound);
        }
        let params: crate::archives::GetThumbnailParams =
            serde_json::from_value(serde_json::json!({})).map_err(|_| RefusalReason::Internal)?;
        let response = crate::archives::get_archive_thumbnail(
            State(self.state.clone()),
            None,
            Path(ArchiveId(archive_id)),
            Query(params),
            axum::http::HeaderMap::new(),
        )
        .await;
        if !response.status().is_success() {
            return Err(RefusalReason::NotFound);
        }
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body = axum::body::to_bytes(response.into_body(), 16 * 1024 * 1024)
            .await
            .map_err(|_| RefusalReason::Internal)?;
        Ok((
            Response::Bytes {
                content_type,
                len: body.len() as u64,
            },
            Some(body.to_vec()),
        ))
    }

    /// The claim form of the manifest: fingerprints where they are ready, and each archive's
    /// category names for "keep the sharer's categories". Unready archives are queued for the
    /// background job and listed without a fingerprint (research §13).
    async fn claim_manifest(&self, share: &Share) -> (ShareManifest, Vec<[u8; 32]>) {
        let mut manifest = browse_manifest(&self.state, share).await;
        let mut roots = Vec::new();
        let mut pending = Vec::new();
        for item in &mut manifest.items {
            let entries: Vec<&mut lanrurugi_share_proto::messages::ArchiveEntry> = match item {
                ManifestItem::Archive(a) => vec![a],
                ManifestItem::Tankoubon { members, .. } => members.iter_mut().collect(),
                ManifestItem::Unavailable { .. } => vec![],
            };
            for entry in entries {
                let id = ArchiveId(entry.archive_id.clone());
                entry.categories = self
                    .state
                    .repos
                    .categories
                    .for_archive(&id)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|c| !c.is_dynamic())
                    .map(|c| c.name)
                    .collect();
                match blobs::ready(&self.state, &id).await {
                    Some(info) => {
                        entry.blake3 = Some(info.blake3);
                        entry.size = info.size;
                        roots.push(info.blake3);
                        self.shared
                            .blobs
                            .lock()
                            .expect("blobs lock")
                            .insert(info.blake3, id);
                    }
                    None => pending.push(id),
                }
            }
        }
        if !pending.is_empty() {
            let state = self.state.clone();
            tokio::spawn(async move {
                blobs::prepare(&state, &pending).await;
            });
        }
        roots.sort();
        roots.dedup();
        (manifest, roots)
    }

    fn sign_ticket(
        &self,
        share: &Share,
        claim_id: &ClaimId,
        importer: EndpointId,
        roots: Vec<[u8; 32]>,
    ) -> SignedTicket {
        Ticket {
            share_id: share.id.to_string(),
            claim_id: claim_id.to_string(),
            importer: *importer.as_bytes(),
            roots,
            expires_at: unix_now() + TICKET_TTL_SECS,
        }
        .sign(&self.shared.signing)
    }

    async fn claim(
        &self,
        conn: &Connection,
        remote: EndpointId,
        session: &str,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let (_, share) = self.share_for_session(remote, session).await?;
        if !share.permissions.import {
            return Err(RefusalReason::PermissionDenied);
        }
        self.uploads_open(remote).await?;
        let address = self.admit(conn, &share).await?;
        // Counting needs an address; a relayed peer of an unrestricted share is recorded by the
        // relay-only placeholder rather than by the relay's address.
        let address = address.unwrap_or(IpAddr::from([0, 0, 0, 0]));
        let outcome = repo(&self.state)
            .claim(
                &share,
                ClaimKind::Import,
                address,
                None,
                Some(&super::endpoint_hex(&remote)),
                now_secs(),
            )
            .await
            .map_err(|_| RefusalReason::Internal)?;
        let claim_id = match outcome {
            ClaimOutcome::Created(id) | ClaimOutcome::Existing(id) => id,
            ClaimOutcome::Refused(reason) => return Err(refusal(reason)),
        };
        let (manifest, roots) = self.claim_manifest(&share).await;
        let ticket = self.sign_ticket(&share, &claim_id, remote, roots);
        Ok((
            Response::Claimed {
                claim_id: claim_id.to_string(),
                manifest,
                ticket,
                helpers: self.active_helpers(&share, remote).await,
            },
            None,
        ))
    }

    async fn prepare(
        &self,
        remote: EndpointId,
        session: &str,
        claim_id: &str,
        archive_id: String,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let (_, share) = self.share_for_session(remote, session).await?;
        if !repo(&self.state)
            .claim_exists(&ClaimId::from(claim_id))
            .await
            .unwrap_or(false)
        {
            return Err(RefusalReason::NotFound);
        }
        let manifest = browse_manifest(&self.state, &share).await;
        if !manifest.archives().any(|a| a.archive_id == archive_id) {
            return Err(RefusalReason::NotFound);
        }
        self.uploads_open(remote).await?;
        let id = ArchiveId(archive_id);
        let info = blobs::ensure_ready(&self.state, &id)
            .await
            .ok_or(RefusalReason::NotFound)?;
        self.shared
            .blobs
            .lock()
            .expect("blobs lock")
            .insert(info.blake3, id);
        Ok((
            Response::Prepared {
                blake3: info.blake3,
                size: info.size,
            },
            None,
        ))
    }

    /// A fresh ticket for an existing claim. Exhaustion never stops an existing claim (FR-015);
    /// revocation, expiry, a removed import permission or a now-refused address do.
    async fn renew(
        &self,
        conn: &Connection,
        remote: EndpointId,
        claim_id: &str,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let claim_id = ClaimId::from(claim_id);
        let share_id = self
            .claim_share(&claim_id, remote)
            .await
            .ok_or(RefusalReason::NotFound)?;
        let share = repo(&self.state)
            .get(&share_id)
            .await
            .ok()
            .flatten()
            .ok_or(RefusalReason::NotFound)?;
        if !share.permissions.import {
            return Err(RefusalReason::PermissionDenied);
        }
        self.admit(conn, &share).await?;
        let (_, roots) = self.claim_manifest(&share).await;
        Ok((
            Response::Renewed {
                ticket: self.sign_ticket(&share, &claim_id, remote, roots),
                helpers: self.active_helpers(&share, remote).await,
            },
            None,
        ))
    }

    /// The share of a live claim made by `remote`.
    async fn claim_share(&self, claim_id: &ClaimId, remote: EndpointId) -> Option<ShareId> {
        use deadpool_redis::redis::AsyncCommands;
        let mut conn = self.state.redis.config.get().await.ok()?;
        let fields: std::collections::HashMap<String, String> = conn
            .hgetall(lanrurugi_storage::keys::share_claim_key(claim_id))
            .await
            .ok()?;
        if fields.get("endpoint_id")? != &super::endpoint_hex(&remote) {
            return None;
        }
        Some(ShareId(fields.get("share_id")?.clone()))
    }

    fn verify_ticket(
        &self,
        remote: EndpointId,
        ticket: &SignedTicket,
        blake3: &[u8; 32],
    ) -> Result<ArchiveId, RefusalReason> {
        let own = self.shared.signing.verifying_key().to_bytes();
        ticket
            .verify_for(&own, remote.as_bytes(), blake3, unix_now())
            .map_err(|_| RefusalReason::NotFound)?;
        self.shared
            .blobs
            .lock()
            .expect("blobs lock")
            .get(blake3)
            .cloned()
            .ok_or(RefusalReason::NotFound)
    }

    async fn range(
        &self,
        remote: EndpointId,
        ticket: &SignedTicket,
        blake3: [u8; 32],
        spans: &[ChunkSpan],
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let own = self.shared.signing.verifying_key().to_bytes();
        if matches!(
            ticket.verify(&own, unix_now()),
            Err(TicketError::BadSignature)
        ) {
            // Not one of ours: maybe a share this instance helps deliver.
            return self.help_range(remote, ticket, blake3, spans).await;
        }
        let archive_id = self.verify_ticket(remote, ticket, &blake3)?;
        let caps = super::limits::admit(
            &self.state,
            super::limits::Group::Own,
            &super::endpoint_hex(&remote),
        )
        .await
        .map_err(held)?;
        let info = blobs::ready(&self.state, &archive_id)
            .await
            .filter(|i| i.blake3 == blake3)
            .ok_or(RefusalReason::NotFound)?;
        let body = encode_range(info, spans).await?;
        super::limits::throttle(super::limits::Group::Own, caps, body.len() as u64).await;
        Ok(bytes(body))
    }

    /// The helper role (FR-033, FR-035): a range of a helped share's archive, for an importer
    /// holding a ticket the sharer signed for it. Helpers never serve outboards (FR-047).
    async fn help_range(
        &self,
        remote: EndpointId,
        ticket: &SignedTicket,
        blake3: [u8; 32],
        spans: &[ChunkSpan],
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let record = super::helper::serving_import(&self.state, &ticket.ticket.share_id)
            .await
            .ok_or(RefusalReason::NotFound)?;
        let sharer =
            super::parse_endpoint_hex(&record.sharer_endpoint_id).ok_or(RefusalReason::NotFound)?;
        ticket
            .verify_for(&sharer, remote.as_bytes(), &blake3, unix_now())
            .map_err(|_| RefusalReason::NotFound)?;
        let local =
            super::helper::local_archive(&record, &hex(&blake3)).ok_or(RefusalReason::NotFound)?;
        let caps = super::limits::admit(
            &self.state,
            super::limits::Group::Helping,
            &super::endpoint_hex(&remote),
        )
        .await
        .map_err(held)?;
        let info = blobs::ensure_ready(&self.state, &local)
            .await
            .filter(|i| i.blake3 == blake3)
            .ok_or(RefusalReason::NotFound)?;
        let body = encode_range(info, spans).await?;
        super::limits::throttle(super::limits::Group::Helping, caps, body.len() as u64).await;
        super::helper::count_served(&record.id, body.len() as u64);
        Ok(bytes(body))
    }

    /// The helpers importers may use now: heard from in the last 15 min, never the asker itself,
    /// and none while the share does not allow helpers.
    async fn active_helpers(&self, share: &Share, remote: EndpointId) -> Vec<HelperAddr> {
        if !share.allow_helpers
            || !matches!(
                share.status(now_secs()),
                ShareStatus::Active | ShareStatus::Exhausted
            )
        {
            return Vec::new();
        }
        let asker = super::endpoint_hex(&remote);
        repo(&self.state)
            .helpers(&share.id, now_secs() - HELP_OFFLINE_AFTER_SECS)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|h| h.endpoint_id != asker)
            .filter_map(|h| {
                Some(HelperAddr {
                    endpoint_id: super::parse_endpoint_hex(&h.endpoint_id)?,
                    relay_urls: h.relay_urls,
                    addrs: h.addrs,
                })
            })
            .collect()
    }

    /// What a helper of `share` should do now (T095): ended for a revoked or expired share or once
    /// helpers are disallowed, paused while exhausted, else active.
    async fn helper_status(&self, share: &Share, archives: &[String]) -> HelperStatus {
        let now = now_secs();
        let at = |t: Option<i64>| t.unwrap_or(now).max(0) as u64;
        let (state, reason, since) = match share.status(now) {
            ShareStatus::Revoked => (
                HelpState::Ended,
                Some(HelpReason::Revoked),
                at(share.revoked_at),
            ),
            ShareStatus::Expired => (
                HelpState::Ended,
                Some(HelpReason::Expired),
                at(share.expires_at),
            ),
            _ if !share.allow_helpers => (
                HelpState::Ended,
                Some(HelpReason::HelpingDisabled),
                at(share.helpers_disabled_at),
            ),
            ShareStatus::Exhausted => {
                let last = repo(&self.state)
                    .claim_history(&share.id, 1)
                    .await
                    .ok()
                    .and_then(|h| h.first().map(|c| c.started_at));
                (HelpState::Paused, Some(HelpReason::Exhausted), at(last))
            }
            ShareStatus::Active => (HelpState::Active, None, at(Some(share.created_at))),
        };
        let current: std::collections::HashSet<String> = browse_manifest(&self.state, share)
            .await
            .archives()
            .map(|a| a.archive_id.clone())
            .collect();
        HelperStatus {
            state,
            reason,
            at: since,
            removed_archives: archives
                .iter()
                .filter(|a| !current.contains(*a))
                .cloned()
                .collect(),
        }
    }

    /// `HelperRegister` (T091): only for shares that allow helpers, from endpoints that made an
    /// import claim on them.
    async fn helper_register(
        &self,
        remote: EndpointId,
        share_id: String,
        archives: Vec<String>,
        relay_urls: Vec<String>,
        addrs: Vec<String>,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let share = repo(&self.state)
            .get(&ShareId(share_id))
            .await
            .ok()
            .flatten()
            .ok_or(RefusalReason::NotFound)?;
        let endpoint = super::endpoint_hex(&remote);
        if !repo(&self.state)
            .is_importer(&share.id, &endpoint)
            .await
            .unwrap_or(false)
        {
            return Err(RefusalReason::NotFound);
        }
        if !share.allow_helpers && share.helpers_disabled_at.is_none() {
            return Err(RefusalReason::PermissionDenied);
        }
        let archives: Vec<String> = archives.into_iter().take(MAX_HELPER_ARCHIVES).collect();
        let status = self.helper_status(&share, &archives).await;
        if status.state != HelpState::Ended {
            let info = HelperInfo {
                endpoint_id: endpoint,
                relay_urls: relay_urls
                    .into_iter()
                    .filter(|u| u.parse::<iroh::RelayUrl>().is_ok())
                    .take(MAX_HELPER_ADDRS)
                    .collect(),
                addrs: addrs
                    .into_iter()
                    .filter(|a| a.parse::<std::net::SocketAddr>().is_ok())
                    .take(MAX_HELPER_ADDRS)
                    .collect(),
                archives,
                last_seen: now_secs(),
            };
            repo(&self.state)
                .register_helper(&share.id, &info)
                .await
                .map_err(|_| RefusalReason::Internal)?;
        }
        Ok((Response::HelperStatus(status), None))
    }

    async fn helper_heartbeat(
        &self,
        remote: EndpointId,
        share_id: String,
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let share = repo(&self.state)
            .get(&ShareId(share_id))
            .await
            .ok()
            .flatten()
            .ok_or(RefusalReason::NotFound)?;
        let info = repo(&self.state)
            .helper_heartbeat(&share.id, &super::endpoint_hex(&remote), now_secs())
            .await
            .ok()
            .flatten()
            .ok_or(RefusalReason::NotFound)?;
        Ok((
            Response::HelperStatus(self.helper_status(&share, &info.archives).await),
            None,
        ))
    }

    /// Refuses with `paused` while own-share uploads are paused or full.
    async fn uploads_open(&self, remote: EndpointId) -> Result<(), RefusalReason> {
        super::limits::admit(
            &self.state,
            super::limits::Group::Own,
            &super::endpoint_hex(&remote),
        )
        .await
        .map(|_| ())
        .map_err(held)
    }

    async fn outboard(
        &self,
        remote: EndpointId,
        ticket: &SignedTicket,
        blake3: [u8; 32],
    ) -> Result<(Response, Option<Vec<u8>>), RefusalReason> {
        let archive_id = self.verify_ticket(remote, ticket, &blake3)?;
        let info = blobs::ready(&self.state, &archive_id)
            .await
            .ok_or(RefusalReason::NotFound)?;
        let body = tokio::fs::read(&info.outboard_path)
            .await
            .map_err(|_| RefusalReason::NotFound)?;
        Ok((
            Response::Bytes {
                content_type: None,
                len: body.len() as u64,
            },
            Some(body),
        ))
    }
}

fn bytes(body: Vec<u8>) -> (Response, Option<Vec<u8>>) {
    (
        Response::Bytes {
            content_type: None,
            len: body.len() as u64,
        },
        Some(body),
    )
}

/// The verified bao encoding of `spans` of a prepared blob.
async fn encode_range(
    info: blobs::BlobInfo,
    spans: &[ChunkSpan],
) -> Result<Vec<u8>, RefusalReason> {
    let mut ranges = ChunkRanges::empty();
    let mut requested = 0u64;
    for span in spans {
        if span.end <= span.start {
            continue;
        }
        requested += (span.end - span.start) * CHUNK_BYTES;
        ranges |= ChunkRanges::from(ChunkNum(span.start)..ChunkNum(span.end));
    }
    if requested > MAX_RANGE_BYTES {
        return Err(RefusalReason::Internal);
    }
    tokio::task::spawn_blocking(move || -> std::io::Result<Vec<u8>> {
        let data = std::fs::File::open(&info.data_path)?;
        let outboard = PreOrderOutboard {
            root: blake3::Hash::from_bytes(info.blake3),
            tree: BaoTree::new(info.size, blobs::BLOCK_SIZE),
            data: std::fs::File::open(&info.outboard_path)?,
        };
        let mut out = Vec::new();
        bao_tree::io::sync::encode_ranges_validated(&data, &outboard, &ranges, &mut out)?;
        Ok(out)
    })
    .await
    .map_err(|_| RefusalReason::Internal)?
    .map_err(|_| RefusalReason::NotFound)
}
