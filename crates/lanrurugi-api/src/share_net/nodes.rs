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

use super::{endpoint_hex, parse_endpoint_hex, read_frame, write_frame, ALPN};
use crate::share_settings::ShareSettings;

/// A configured node: `lrrg-node:<endpoint_id>@<host:port>[?relay=<https url>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRef {
    pub endpoint_id: [u8; 32],
    pub addr: String,
    pub relay: Option<String>,
}

pub fn parse_node(entry: &str) -> Option<NodeRef> {
    let rest = entry.trim().strip_prefix("lrrg-node:")?;
    let (head, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (id, addr) = head.split_once('@')?;
    let endpoint_id = parse_endpoint_hex(id)?;
    if !addr.contains(':') {
        return None;
    }
    let relay = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == "relay")
        .map(|(_, v)| v.to_string())
        .filter(|v| url::Url::parse(v).is_ok_and(|u| u.scheme() == "https"));
    Some(NodeRef {
        endpoint_id,
        addr: addr.to_string(),
        relay,
    })
}

pub fn configured_nodes(settings: &ShareSettings) -> Vec<NodeRef> {
    settings
        .list("share_nodes")
        .iter()
        .filter_map(|e| parse_node(e))
        .collect()
}

/// This instance's node string, when it offers a role and has a public address to give.
pub fn node_string(endpoint_id: &iroh::EndpointId, settings: &ShareSettings) -> Option<String> {
    if !settings.flag("share_node_rendezvous") && !settings.flag("share_node_relay") {
        return None;
    }
    let host = settings
        .external_addrs()
        .first()
        .map(|a| a.to_string())
        .or_else(|| {
            let url = url::Url::parse(&settings.public_url()?).ok()?;
            let port = settings.number("share_udp_port").unwrap_or(11204);
            Some(format!("{}:{port}", url.host_str()?))
        })?;
    let mut out = format!("lrrg-node:{}@{host}", endpoint_hex(endpoint_id));
    if settings.flag("share_node_relay") {
        if let Some(relay) = own_relay_url(settings) {
            out.push_str(&format!("?relay={relay}"));
        }
    }
    Some(out)
}

/// The public address of this instance's relay role: the tunnel address when set, else the
/// public URL's host on 443.
pub fn own_relay_url(settings: &ShareSettings) -> Option<String> {
    let explicit = settings.text("share_self_relay_url");
    if !explicit.is_empty() {
        return Some(explicit.to_string());
    }
    let url = url::Url::parse(&settings.public_url()?).ok()?;
    Some(format!("https://{}/", url.host_str()?))
}

/// The endpoint address of a configured node, its port resolved by DNS.
pub async fn node_addr(node: &NodeRef) -> Option<EndpointAddr> {
    let direct = tokio::net::lookup_host(&node.addr)
        .await
        .ok()
        .and_then(|mut a| a.next());
    let relays: Vec<String> = node.relay.iter().cloned().collect();
    super::endpoint_addr(&node.endpoint_id, &relays, direct)
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

/// Starts the embedded relay for the relay role (FR-048, FR-055): ACME for the public URL's host,
/// a certificate pair from files, or plain HTTP on a local port behind a tunnel or reverse proxy
/// that terminates TLS.
pub async fn start_relay(
    settings: &ShareSettings,
    data_dir: &Path,
    own: iroh::EndpointId,
) -> Result<iroh_relay::server::Server, String> {
    use iroh_relay::server::{AcmeConfig, CertConfig, RelayConfig, ServerConfig, TlsConfig};
    let port = settings.number("share_node_relay_port").unwrap_or(11205) as u16;
    let mut relay = match settings.text("share_node_relay_tls") {
        "proxy" => RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], port))),
        mode => {
            // The captive-portal HTTP listener is irrelevant here; keep it local and ephemeral.
            let mut relay = RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)));
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let builder = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_no_client_auth();
            let cert = if mode == "files" {
                let certs = load_certs(Path::new(settings.text("share_node_relay_cert")))?;
                let key = load_key(Path::new(settings.text("share_node_relay_key")))?;
                CertConfig::Manual {
                    server_config: builder
                        .with_single_cert(certs, key)
                        .map_err(|e| e.to_string())?,
                }
            } else {
                let host = settings
                    .public_url()
                    .and_then(|u| url::Url::parse(&u).ok())
                    .and_then(|u| u.host_str().map(str::to_string))
                    .ok_or("the relay role's ACME mode needs a public address")?;
                CertConfig::LetsEncrypt {
                    acme_config: AcmeConfig::letsencrypt(true)
                        .domains(vec![host])
                        .cache_path(data_dir.join("acme")),
                    server_config_builder: builder,
                }
            };
            relay.tls = Some(TlsConfig::new(any_addr(port), cert));
            relay
        }
    };
    relay.limits = relay_limits(settings);
    let allowed = relay_allow_list(settings);
    if !allowed.is_empty() {
        relay.access = Arc::new(AllowListed {
            own,
            allowed: allowed.into_iter().collect(),
        });
    }
    let mut config = ServerConfig::default();
    // QUIC address discovery on the standard port, so peers behind NAT learn their public
    // address from this relay and can hole-punch. It needs the relay's own TLS, so not in `proxy`.
    if relay.tls.is_some() {
        config.quic = Some(iroh_relay::server::QuicConfig::new(any_addr(
            iroh_relay::defaults::DEFAULT_RELAY_QUIC_PORT,
        )));
    }
    config.relay = Some(relay);
    iroh_relay::server::Server::spawn(config)
        .await
        .map_err(|e| e.to_string())
}

/// Per-client rate limit from `share_node_relay_rate` (bytes per second, empty for none). Every
/// relayed byte enters from one client, so this caps what any one peer can push through.
fn relay_limits(settings: &ShareSettings) -> iroh_relay::server::Limits {
    let mut limits = iroh_relay::server::Limits::default();
    let rate = settings
        .number("share_node_relay_rate")
        .map(|n| n.min(u64::from(u32::MAX)) as u32)
        .and_then(std::num::NonZeroU32::new);
    if let Some(rate) = rate {
        let mut client = iroh_relay::server::ClientRateLimit::new(rate);
        // A second's worth of burst keeps short handshakes snappy.
        client.max_burst_bytes = Some(rate);
        limits.client_rx = Some(client);
    }
    limits
}

/// Endpoint ids from `share_node_relay_allow`: bare hex ids or node strings.
pub fn relay_allow_list(settings: &ShareSettings) -> Vec<iroh::EndpointId> {
    settings
        .list("share_node_relay_allow")
        .iter()
        .filter_map(|entry| parse_allow_entry(entry))
        .collect()
}

pub fn parse_allow_entry(entry: &str) -> Option<iroh::EndpointId> {
    let entry = entry.trim();
    let bytes = match parse_node(entry) {
        Some(node) => node.endpoint_id,
        None => parse_endpoint_hex(entry)?,
    };
    iroh::EndpointId::from_bytes(&bytes).ok()
}

/// Admits this instance and the listed endpoints only. Everyone else, including importers of
/// this instance's own shares, has to use another relay.
#[derive(Debug)]
struct AllowListed {
    own: iroh::EndpointId,
    allowed: std::collections::HashSet<iroh::EndpointId>,
}

impl iroh_relay::server::AccessControl for AllowListed {
    async fn on_connect(
        &self,
        request: &iroh_relay::server::ClientRequest,
    ) -> iroh_relay::server::Access {
        let id = request.endpoint_id();
        if id == self.own || self.allowed.contains(&id) {
            iroh_relay::server::Access::Allow
        } else {
            iroh_relay::server::Access::Deny { reason: None }
        }
    }
}

/// The wildcard address for a public listener: `[::]` where the host has IPv6 (on Linux it
/// accepts IPv4 too), else `0.0.0.0`.
fn any_addr(port: u16) -> SocketAddr {
    if std::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).is_ok() {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port))
    } else {
        SocketAddr::from(([0, 0, 0, 0], port))
    }
}

fn load_certs(path: &Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject;
    rustls::pki_types::CertificateDer::pem_file_iter(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn load_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    use rustls::pki_types::pem::PemObject;
    rustls::pki_types::PrivateKeyDer::from_pem_file(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

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
    if let Some(server) = relay_server {
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
