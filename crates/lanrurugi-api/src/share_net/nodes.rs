//! LANrurugi nodes: instances with a public address that offer rendezvous and relay to their
//! friends, so a circle of instances needs no third party (FR-048, FR-054, FR-055, research §7).
//!
//! The rendezvous role stores signed knock packets it cannot read; the relay role embeds
//! `iroh-relay`. Both also run without a library, as `lanrurugi-server share-node`.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use iroh::endpoint::{presets, Connection};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use lanrurugi_share_proto::messages::{RefusalReason, Request, Response, ShareEngine};
use lanrurugi_storage::keys;

use super::{endpoint_hex, read_frame, write_frame, ALPN};
use crate::share_settings::ShareSettings;

pub use lanrurugi_share::nodes::{
    configured_nodes, node_relay_url, own_relay_url, parse_node, NodeRef,
};

/// Publishes this instance's node record under its own code, so a peer holding the 16-character
/// code can find the endpoint id and the relays to reach it through.
pub async fn publish_node_record(
    state: &crate::AppState,
    net: &super::ShareNet,
    settings: &ShareSettings,
) {
    lanrurugi_share::publish::publish_node_record(state, net, settings).await
}

/// Restarts this instance's relay role on the same endpoint: every connection being forwarded
/// through it drops at once, while the endpoint itself (and therefore every direct session) keeps
/// running. That is the split "destroy the node" needs — the relay is what a rotation invalidates.
pub async fn restart_relay(
    state: &crate::AppState,
    settings: &ShareSettings,
    net: &super::ShareNet,
) {
    if let Some(previous) = net.relay.lock().await.take() {
        let _ = previous.shutdown().await;
    }
    if !settings.flag("share_node_relay") {
        return;
    }
    let dir = state.library.temp_dir.join("share");
    match start_relay(settings, &dir, net.id()).await {
        Ok((server, port)) => {
            *net.relay.lock().await = Some(server);
            *net.relay_port.lock().await = Some(port);
        }
        Err(e) => tracing::warn!(error = %e, "relay role could not restart after a code rotation"),
    }
}

/// The `endpoint_id` a retracted record carries. Nothing can ever own it, so a peer that resolves
/// a retired code learns "this node is gone" instead of dialling an address that no longer answers.
pub const RETRACTED_ENDPOINT_ID: [u8; 32] = [0u8; 32];

/// Overwrites the record behind a retired code, so every peer still holding that code stops
/// resolving it at once rather than at the end of the relay's TTL (two hours). Called before the
/// identity is replaced, while its code can still be read.
pub async fn retract_node_record(
    state: &crate::AppState,
    code: &lanrurugi_share_proto::node_code::NodeCode,
) {
    let settings = crate::share_settings::load(state).await;
    let relays = super::publish::pkarr_relays(&settings);
    if relays.is_empty() {
        return;
    }
    let keys = lanrurugi_share_proto::derive::DerivedKeys {
        lookup_key: code.lookup_keypair(),
        record_key: code.record_key(),
    };
    let knock = lanrurugi_share_proto::record::KnockV1 {
        endpoint_id: RETRACTED_ENDPOINT_ID,
        relay_urls: Vec::new(),
        direct_addrs: Vec::new(),
    };
    let Ok(packet) = lanrurugi_share_proto::record::seal(&keys, &knock) else {
        return;
    };
    let payload = packet.to_relay_payload();
    let client = super::proxy::http_client(std::time::Duration::from_secs(20));
    for relay in relays {
        let url = node_relay_url(&relay, code);
        // A retraction that does not land leaves the code pointing at this instance for the whole
        // record TTL, so retry a couple of times before giving up loudly.
        let mut ok = false;
        for attempt in 0..3u32 {
            ok = client
                .put(&url)
                .header("content-type", "application/pkarr.org/relays#payload")
                .body(payload.clone())
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            if ok {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(500 << attempt)).await;
        }
        if !ok {
            tracing::warn!(relay = %relay, "the node record could not be retracted; it stays published until it expires");
        }
        crate::share_net::status::record_rendezvous(&relay, ok);
    }
}

/// How often the record behind a node code is refreshed. Well inside `RECORD_TTL_SECS` (3600) so
/// a peer that looks the code up never lands on an expired copy.
const NODE_RECORD_REFRESH_SECS: u64 = 600;

/// Keeps the record behind this node's code current for as long as this endpoint is the running
/// one. Addresses move — a DHCP lease, a renewed NAT mapping, a different relay — while the code
/// never changes, so republishing is the only thing that has to happen on a schedule.
pub fn spawn_record_refresh(state: crate::AppState, net: std::sync::Arc<super::ShareNet>) {
    // Identity is stable across endpoint rebuilds (same node secret), so comparing ids would leave
    // one refresh loop running per rebuild forever. The pointer tells them apart, and a weak one
    // does it without keeping the retired endpoint's memory alive.
    let mine = std::sync::Arc::downgrade(&net);
    tokio::spawn(async move {
        let mut tick =
            tokio::time::interval(std::time::Duration::from_secs(NODE_RECORD_REFRESH_SECS));
        tick.tick().await; // the first tick is immediate; the caller already published
        loop {
            tick.tick().await;
            let Some(current) = super::handle().await else {
                break;
            };
            let still_mine = mine
                .upgrade()
                .is_some_and(|net| std::sync::Arc::ptr_eq(&net, &current));
            if !still_mine {
                break;
            }
            let settings = crate::share_settings::load(&state).await;
            publish_node_record(&state, &current, &settings).await;
        }
    });
}

/// Reads a node record back. `None` means "this code is not published anywhere I can see", which
/// the caller reports as a failed connection rather than as a bad code.
pub async fn resolve_node_code(state: &crate::AppState, code: &str) -> Option<EndpointAddr> {
    let settings = crate::share_settings::load(state).await;
    lanrurugi_share::nodes::resolve_node_code(&settings, code).await
}

/// The endpoint address of a configured node, its port resolved by DNS.
pub async fn node_addr(state: &crate::AppState, node: &NodeRef) -> Option<EndpointAddr> {
    let settings = crate::share_settings::load(state).await;
    lanrurugi_share::nodes::node_addr(&settings, node).await
}

// ---------------------------------------------------------------------------------------------
// Rendezvous storage

/// Packets held by a node-only instance: signed packet bytes and when they were last written.
type MemoryPackets = Arc<Mutex<HashMap<String, (Vec<u8>, Instant)>>>;

/// Where a rendezvous role keeps packets: Redis in a full instance, memory in node-only mode.
#[derive(Clone)]
pub enum RendezvousStore {
    Redis(Pool),
    Memory(MemoryPackets),
}

const PACKET_TTL: Duration = Duration::from_secs(keys::SHARE_RDV_TTL_SECS as u64);
/// A pkarr packet is at most 1000 bytes of DNS plus 104 bytes of header.
const MAX_PACKET: usize = 1104;

impl RendezvousStore {
    pub fn memory() -> Self {
        Self::Memory(Arc::new(Mutex::new(HashMap::new())))
    }

    /// Verifies the packet's signature and keeps the newest one per key.
    pub async fn put(&self, packet: &[u8]) -> Result<(), RefusalReason> {
        if packet.len() < 32 + 64 + 8 || packet.len() > MAX_PACKET {
            return Err(RefusalReason::NotFound);
        }
        let key = pkarr::PublicKey::try_from(&packet[..32]).map_err(|_| RefusalReason::NotFound)?;
        let signed =
            pkarr::SignedPacket::from_relay_payload(&key, &Bytes::copy_from_slice(&packet[32..]))
                .map_err(|_| RefusalReason::NotFound)?;
        let name = crate::shares::hex(&key.to_bytes());
        if let Some(existing) = self.get_raw(&name).await {
            if let Ok(old) = pkarr::SignedPacket::from_relay_payload(
                &key,
                &Bytes::copy_from_slice(&existing[32..]),
            ) {
                if old.more_recent_than(&signed) {
                    return Ok(());
                }
            }
        }
        match self {
            Self::Redis(pool) => {
                let mut conn = pool.get().await.map_err(|_| RefusalReason::Internal)?;
                let _: () = conn
                    .set_ex(
                        keys::share_rdv_key(&name),
                        packet,
                        keys::SHARE_RDV_TTL_SECS as u64,
                    )
                    .await
                    .map_err(|_| RefusalReason::Internal)?;
            }
            Self::Memory(map) => {
                let mut map = map.lock().expect("rendezvous lock");
                map.retain(|_, (_, at)| at.elapsed() < PACKET_TTL);
                map.insert(name, (packet.to_vec(), Instant::now()));
            }
        }
        Ok(())
    }

    async fn get_raw(&self, name: &str) -> Option<Vec<u8>> {
        match self {
            Self::Redis(pool) => {
                let mut conn = pool.get().await.ok()?;
                conn.get(keys::share_rdv_key(name)).await.ok().flatten()
            }
            Self::Memory(map) => map
                .lock()
                .expect("rendezvous lock")
                .get(name)
                .filter(|(_, at)| at.elapsed() < PACKET_TTL)
                .map(|(p, _)| p.clone()),
        }
    }

    pub async fn get(&self, lookup_pubkey: &[u8; 32]) -> Option<Vec<u8>> {
        self.get_raw(&crate::shares::hex(lookup_pubkey)).await
    }

    /// Persists a memory store across restarts of a node-only process.
    pub fn save(&self, path: &Path) {
        if let Self::Memory(map) = self {
            let map = map.lock().expect("rendezvous lock");
            let packets: Vec<String> = map
                .values()
                .filter(|(_, at)| at.elapsed() < PACKET_TTL)
                .map(|(p, _)| crate::shares::hex(p))
                .collect();
            let _ = std::fs::write(path, packets.join("\n"));
        }
    }

    pub async fn load(&self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            if let Some(packet) = crate::shares::unhex(line.trim()) {
                let _ = self.put(&packet).await;
            }
        }
    }
}

/// Per-source request counting for the rendezvous role, generous enough for a sharer publishing
/// many shares at once.
pub struct RendezvousLimiter {
    counts: Mutex<HashMap<String, (u32, Instant)>>,
}

const RDV_PER_MINUTE: u32 = 600;

impl RendezvousLimiter {
    pub fn new() -> Self {
        Self {
            counts: Mutex::new(HashMap::new()),
        }
    }

    pub fn allow(&self, source: &str) -> bool {
        let mut counts = self.counts.lock().expect("rendezvous limiter lock");
        if counts.len() > 100_000 {
            counts.retain(|_, (_, start)| start.elapsed() < Duration::from_secs(60));
        }
        let entry = counts
            .entry(source.to_string())
            .or_insert((0, Instant::now()));
        if entry.1.elapsed() >= Duration::from_secs(60) {
            *entry = (0, Instant::now());
        }
        entry.0 += 1;
        entry.0 <= RDV_PER_MINUTE
    }
}

impl Default for RendezvousLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Answers `RendezvousPut`/`RendezvousGet`; `None` for any other request.
pub async fn answer_rendezvous(
    store: &RendezvousStore,
    limiter: &RendezvousLimiter,
    source: &str,
    request: &Request,
) -> Option<Result<Response, RefusalReason>> {
    let answer = match request {
        Request::RendezvousPut { signed_packet } => {
            if !limiter.allow(source) {
                return Some(Err(RefusalReason::RateLimited));
            }
            store.put(signed_packet).await.map(|_| Response::Ok)
        }
        Request::RendezvousGet { lookup_pubkey } => {
            if !limiter.allow(source) {
                return Some(Err(RefusalReason::RateLimited));
            }
            Ok(match store.get(lookup_pubkey).await {
                Some(signed_packet) => Response::Packet { signed_packet },
                None => Response::NotHere,
            })
        }
        _ => return None,
    };
    Some(answer)
}

// ---------------------------------------------------------------------------------------------
// Relay role

use lanrurugi_share::relay::load_certs;
pub use lanrurugi_share::relay::{parse_allow_entry, relay_allow_list, start_relay};

// ---------------------------------------------------------------------------------------------
// Node-only mode (FR-054)

#[derive(Clone)]
struct NodeProtocol {
    store: RendezvousStore,
    limiter: Arc<RendezvousLimiter>,
}

impl std::fmt::Debug for NodeProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NodeProtocol")
    }
}

impl ProtocolHandler for NodeProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let source = endpoint_hex(&connection.remote_id());
        loop {
            let Ok((mut send, mut recv)) = connection.accept_bi().await else {
                return Ok(());
            };
            let this = self.clone();
            let source = source.clone();
            tokio::spawn(async move {
                let Ok((engine, request)) = read_frame::<Request>(&mut recv).await else {
                    return;
                };
                let answer = if !engine.is_compatible_with(&ShareEngine::current()) {
                    Response::Refused(RefusalReason::UnsupportedVersion {
                        engine: ShareEngine::current(),
                    })
                } else {
                    match answer_rendezvous(&this.store, &this.limiter, &source, &request).await {
                        Some(Ok(r)) => r,
                        Some(Err(reason)) => Response::Refused(reason),
                        None => Response::Refused(RefusalReason::NotFound),
                    }
                };
                let _ = write_frame(&mut send, &answer).await;
                let _ = send.finish();
            });
        }
    }
}

/// Options of `lanrurugi-server share-node`.
pub struct NodeOptions {
    pub data_dir: PathBuf,
    pub udp_port: u16,
    pub public_host: Option<String>,
    pub relay: bool,
    pub relay_port: u16,
    pub relay_tls: String,
    pub relay_cert: Option<PathBuf>,
    pub relay_key: Option<PathBuf>,
    /// Extra trusted roots, for a relay certificate from a private CA.
    pub ca_file: Option<PathBuf>,
    /// Per-client relay rate in bytes per second; `None` keeps the default.
    pub relay_rate: Option<u64>,
    /// Endpoints the relay admits (ids or node strings); empty admits everyone.
    pub relay_allow: Vec<String>,
}

/// Runs a rendezvous (and optionally relay) node with no library and no Redis until Ctrl-C.
pub async fn run_standalone(options: NodeOptions) -> Result<(), String> {
    std::fs::create_dir_all(&options.data_dir).map_err(|e| e.to_string())?;
    let key_path = options.data_dir.join("node.key");
    let secret = match std::fs::read_to_string(&key_path)
        .ok()
        .and_then(|s| crate::shares::unhex(s.trim()))
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
    {
        Some(secret) => secret,
        None => {
            let fresh: [u8; 32] = rand::random();
            std::fs::write(&key_path, crate::shares::hex(&fresh)).map_err(|e| e.to_string())?;
            fresh
        }
    };
    let store = RendezvousStore::memory();
    let packets_path = options.data_dir.join("rendezvous.packets");
    store.load(&packets_path).await;

    let mut settings = HashMap::new();
    settings.insert(
        "share_node_relay_port".to_string(),
        options.relay_port.to_string(),
    );
    settings.insert(
        "share_node_relay_tls".to_string(),
        options.relay_tls.clone(),
    );
    if let Some(host) = &options.public_host {
        settings.insert("share_public_url".to_string(), format!("https://{host}"));
    }
    if let Some(cert) = &options.relay_cert {
        settings.insert(
            "share_node_relay_cert".to_string(),
            cert.to_string_lossy().into_owned(),
        );
    }
    if let Some(key) = &options.relay_key {
        settings.insert(
            "share_node_relay_key".to_string(),
            key.to_string_lossy().into_owned(),
        );
    }
    if let Some(rate) = options.relay_rate {
        settings.insert("share_node_relay_rate".to_string(), rate.to_string());
    }
    if !options.relay_allow.is_empty() {
        settings.insert(
            "share_node_relay_allow".to_string(),
            options.relay_allow.join("\n"),
        );
    }
    let node_settings = ShareSettings::from_map(&settings);
    let relay_server = if options.relay {
        let own = iroh::SecretKey::from_bytes(&secret).public();
        Some(start_relay(&node_settings, &options.data_dir, own).await?)
    } else {
        None
    };

    let host = options
        .public_host
        .clone()
        .unwrap_or_else(|| "<public-host>".to_string());
    let relay_url = options.relay.then(|| {
        let port = match (options.relay_tls.as_str(), options.relay_port) {
            ("proxy", _) | (_, 443) => String::new(),
            (_, p) => format!(":{p}"),
        };
        format!("https://{host}{port}/")
    });
    // With the relay role the node stays connected to its own relay, so peers that cannot send
    // UDP (blocked, proxy only) still reach its rendezvous through it.
    let relay_mode = match relay_url
        .as_deref()
        .and_then(|u| u.parse::<iroh::RelayUrl>().ok())
    {
        Some(url) if options.public_host.is_some() => {
            RelayMode::Custom(iroh::RelayMap::from_iter([url]))
        }
        _ => RelayMode::Disabled,
    };
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&secret))
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(relay_mode);
    if let Some(path) = &options.ca_file {
        let roots = load_certs(path)?;
        builder = builder.ca_tls_config(super::relay_tls().with_extra_roots(roots));
    } else {
        builder = builder.ca_tls_config(super::relay_tls());
    }
    let endpoint = builder
        .bind_addr(SocketAddr::from(([0, 0, 0, 0], options.udp_port)))
        .map_err(|e| e.to_string())?
        .bind_addr_with_opts(
            SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, options.udp_port)),
            iroh::endpoint::BindOpts::default().set_is_required(false),
        )
        .map_err(|e| e.to_string())?
        .bind()
        .await
        .map_err(|e| e.to_string())?;
    let id = endpoint.id();
    let router = Router::builder(endpoint)
        .accept(
            ALPN,
            NodeProtocol {
                store: store.clone(),
                limiter: Arc::new(RendezvousLimiter::new()),
            },
        )
        .spawn();
    let mut node = format!(
        "lrrg-node:{}@{host}:{}",
        endpoint_hex(&id),
        options.udp_port
    );
    if let Some(url) = &relay_url {
        node.push_str(&format!("?relay={url}"));
    }
    println!("{node}");
    tracing::info!(node = %node, "share node running");

    let _ = tokio::signal::ctrl_c().await;
    store.save(&packets_path);
    let _ = router.shutdown().await;
    if let Some((server, _port)) = relay_server {
        let _ = server.shutdown().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_share_proto::code::ShareCode;
    use lanrurugi_share_proto::derive::DerivedKeys;
    use lanrurugi_share_proto::record::{seal, KnockV1};

    #[test]
    fn node_strings_parse_with_and_without_a_relay() {
        let id = "ab".repeat(32);
        let node = parse_node(&format!(
            "lrrg-node:{id}@node.example.org:443?relay=https://node.example.org/"
        ))
        .unwrap();
        assert_eq!(node.addr, "node.example.org:443");
        assert_eq!(node.relay.as_deref(), Some("https://node.example.org/"));
        assert!(parse_node(&format!("lrrg-node:{id}@node.example.org:443"))
            .unwrap()
            .relay
            .is_none());
        assert!(parse_node("lrrg-node:short@x:1").is_none());
        assert!(parse_node(&format!("lrrg-node:{id}@nohostport")).is_none());
    }

    #[tokio::test]
    async fn the_store_keeps_only_verified_packets_and_never_sees_inside() {
        let store = RendezvousStore::memory();
        let keys = DerivedKeys::derive(&ShareCode::parse("K7QM9XRT2H").unwrap());
        let packet = seal(
            &keys,
            &KnockV1 {
                endpoint_id: [1; 32],
                relay_urls: vec![],
                direct_addrs: vec![],
            },
        )
        .unwrap();
        let bytes = packet.as_bytes().to_vec();
        store.put(&bytes).await.unwrap();
        assert_eq!(store.get(&keys.lookup_public()).await.unwrap(), bytes);

        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 1;
        assert!(store.put(&tampered).await.is_err());
        assert!(store.put(&[0u8; 10]).await.is_err());
    }

    #[test]
    fn the_rendezvous_limiter_caps_a_minute() {
        let limiter = RendezvousLimiter::new();
        for _ in 0..RDV_PER_MINUTE {
            assert!(limiter.allow("a"));
        }
        assert!(!limiter.allow("a"));
        assert!(limiter.allow("b"));
    }
}
