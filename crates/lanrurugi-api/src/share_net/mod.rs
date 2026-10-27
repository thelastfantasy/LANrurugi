//! Instance-to-instance sharing over iroh (specs/017-archive-sharing US4/US5, research §1).
//!
//! One iroh endpoint per instance, identified by the node secret. It speaks the share protocol
//! (`lanrurugi-share-proto`) under ALPN `h3`, so a connection's first packet reads like HTTP/3 and
//! never names the application (FR-045). Peers are dialled by endpoint id with the relays from the
//! knock record or a `via` hint; iroh's own address lookup services are not used, apart from mDNS on
//! the local network (FR-047).

pub mod blobs;
pub mod helper;
pub mod import;
pub mod limits;
pub mod nodes;
pub mod proxy;
pub mod publish;
pub mod server;
pub mod status;

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey};
use lanrurugi_core::ids::{ArchiveId, ShareId};
use lanrurugi_share_proto::framing;
use lanrurugi_share_proto::messages::{Envelope, RefusalReason, Request, Response};
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;

use crate::share_settings::ShareSettings;
use crate::AppState;

/// The ALPN every sharing connection uses.
pub const ALPN: &[u8] = b"h3";
/// Neutral mDNS service label; the library default names the transport.
const MDNS_SERVICE: &str = "lshr";
/// Public rendezvous relays used while public services are on and none are configured.
/// Two operators, so one going away leaves lookups working; records go to and are asked of both.
/// Neither runs the default relays, so no single operator sees both a lookup and the connection
/// that follows it.
pub const DEFAULT_PKARR_RELAYS: [&str; 2] = ["https://pkarr.pubky.app", "https://relay.pkarr.org"];
/// How long an `Open` session stays usable.
const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
/// How long a claim on an IP-restricted share waits for a direct path (research §4).
pub const DIRECT_PATH_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, thiserror::Error)]
pub enum ShareNetError {
    #[error("sharing is not running on this instance")]
    NotRunning,
    #[error("the sharer could not be reached: {0}")]
    Unreachable(String),
    #[error("the sharer refused: {0}")]
    Refused(RefusalReason),
    #[error("protocol error: {0}")]
    Protocol(String),
}

impl ShareNetError {
    /// The stable code shown to the user (contracts/http-api.md "Refusal reasons").
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused(r) => r.code(),
            Self::Unreachable(_) | Self::NotRunning => "sharer_unreachable",
            Self::Protocol(_) => "internal",
        }
    }
}

/// What an `Open` session grants: browsing one share, from one remote endpoint.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    pub share_id: ShareId,
    pub remote: EndpointId,
    pub created: Instant,
}

/// State shared by the endpoint's protocol handler and the rest of the server.
pub(crate) struct Shared {
    pub signing: ed25519_dalek::SigningKey,
    pub sessions: Mutex<HashMap<String, Session>>,
    /// blake3 → archive, for answering `Range` without a library scan.
    pub blobs: Mutex<HashMap<[u8; 32], ArchiveId>>,
}

impl Shared {
    pub fn session(&self, token: &str, remote: EndpointId) -> Option<Session> {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        sessions.retain(|_, s| s.created.elapsed() < SESSION_TTL);
        sessions.get(token).filter(|s| s.remote == remote).cloned()
    }
}

pub struct ShareNet {
    router: Router,
    /// The embedded relay of the relay role, when offered.
    relay: tokio::sync::Mutex<Option<iroh_relay::server::Server>>,
    /// Endpoints mDNS has seen on the local network, asked by `ResolveLocal`.
    pub(crate) lan_peers: Arc<Mutex<std::collections::HashSet<EndpointId>>>,
}

impl ShareNet {
    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint().id()
    }
}

static RUNNING: RwLock<Option<Arc<ShareNet>>> = RwLock::const_new(None);

/// The running endpoint, if sharing has started.
pub async fn handle() -> Option<Arc<ShareNet>> {
    RUNNING.read().await.clone()
}

/// The relays this instance listens on and advertises (FR-027, FR-048, FR-055).
pub fn relay_urls(settings: &ShareSettings) -> Vec<RelayUrl> {
    let node_relays: Vec<String> = nodes::configured_nodes(settings)
        .into_iter()
        .filter_map(|n| n.relay)
        .collect();
    let mut candidates: Vec<String> = settings
        .list("share_relay_urls")
        .into_iter()
        .chain(node_relays)
        .chain(std::iter::once(
            settings.text("share_self_relay_url").to_string(),
        ))
        // The relay role's own relay, so this endpoint can be reached through it.
        .chain(
            settings
                .flag("share_node_relay")
                .then(|| nodes::own_relay_url(settings))
                .flatten(),
        )
        .collect();
    if settings.flag("share_public_services") {
        candidates.extend(
            iroh::defaults::prod::default_relay_map()
                .urls::<Vec<RelayUrl>>()
                .iter()
                .map(|u| u.to_string()),
        );
    }
    // One spelling per relay (iroh's own defaults end in a dot), so the same relay never appears
    // twice in the map or in a code.
    let mut urls: Vec<RelayUrl> = Vec::new();
    for url in candidates
        .iter()
        .filter_map(|s| crate::share_settings::canonical_https_url(s))
        .filter_map(|s| s.parse::<RelayUrl>().ok())
    {
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

/// Starts (or restarts, after a settings change) this instance's endpoint.
pub async fn start(state: &AppState) -> Result<Arc<ShareNet>, String> {
    let settings = crate::share_settings::load(state).await;
    let secret = crate::shares::node_secret(state).await?;
    let port = settings.number("share_udp_port").unwrap_or(11204) as u16;
    // The old endpoint holds the UDP port (and the relay role its TCP port): release them before
    // binding again, or every rebuild after a settings change fails to bind.
    let mut running = RUNNING.write().await;
    if let Some(previous) = running.take() {
        let _ = previous.router.shutdown().await;
        if let Some(relay) = previous.relay.lock().await.take() {
            let _ = relay.shutdown().await;
        }
    }
    let net = Arc::new(bind(state, &settings, secret, port).await?);
    *running = Some(net.clone());
    drop(running);
    if settings.flag("share_node_relay") {
        let dir = state.library.temp_dir.join("share");
        match nodes::start_relay(&settings, &dir, net.id()).await {
            Ok(server) => *net.relay.lock().await = Some(server),
            Err(e) => tracing::warn!(error = %e, "relay role could not start"),
        }
    }
    tracing::info!(endpoint = %net.id(), "sharing endpoint started");
    Ok(net)
}

/// Binds an endpoint serving the share protocol for `state`. Tests bind several in one process.
pub async fn bind(
    state: &AppState,
    settings: &ShareSettings,
    secret: [u8; 32],
    port: u16,
) -> Result<ShareNet, String> {
    let secret_key = SecretKey::from_bytes(&secret);
    // Never `Disabled`: with no relay of its own the endpoint must still dial through the relay a
    // sharer names in its knock record or `via` hint (an own tunnel, a friend's node).
    let relay_mode = RelayMode::Custom(relay_urls(settings).into_iter().collect::<RelayMap>());
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret_key)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(relay_mode);
    // IPv6 too where the host has it (FR-050 item 7), without failing on IPv4-only hosts.
    builder = builder
        .bind_addr(SocketAddr::from(([0, 0, 0, 0], port)))
        .map_err(|e| e.to_string())?
        .bind_addr_with_opts(
            SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)),
            iroh::endpoint::BindOpts::default().set_is_required(false),
        )
        .map_err(|e| e.to_string())?;
    // The user's own proxy (FR-051): iroh only takes an HTTP proxy, so SOCKS goes through a
    // local bridge. Only relay connections use it; direct UDP and the LAN never do.
    match proxy::choose() {
        proxy::Choice::Http(url) => builder = builder.proxy_url(url),
        proxy::Choice::Socks(url) => match proxy::start_socks_bridge(url).await {
            Ok(addr) => {
                if let Ok(url) = url::Url::parse(&format!("http://{addr}")) {
                    builder = builder.proxy_url(url);
                }
            }
            Err(e) => tracing::warn!(error = %e, "SOCKS bridge could not start"),
        },
        proxy::Choice::None => {}
    }
    builder = builder.ca_tls_config(relay_tls());
    for addr in settings.external_addrs() {
        builder = builder.external_addr(addr);
    }
    if !settings.flag("share_portmap") {
        builder = builder.portmapper_config(iroh::endpoint::PortmapperConfig::Disabled);
    }
    let endpoint = builder.bind().await.map_err(|e| e.to_string())?;
    // mDNS under a neutral label (the library default names the transport), kept so `ResolveLocal`
    // can ask the peers it found (FR-024).
    let lan_peers = Arc::new(Mutex::new(std::collections::HashSet::new()));
    match iroh_mdns_address_lookup::MdnsAddressLookup::builder()
        .service_name(MDNS_SERVICE)
        .build(endpoint.id())
    {
        Ok(mdns) => {
            if let Ok(lookup) = endpoint.address_lookup() {
                lookup.add(mdns.clone());
            }
            let peers = lan_peers.clone();
            tokio::spawn(async move {
                use futures_util::StreamExt;
                let mut events = mdns.subscribe().await;
                while let Some(event) = events.next().await {
                    let mut peers = peers.lock().expect("lan peers lock");
                    match event {
                        iroh_mdns_address_lookup::DiscoveryEvent::Discovered {
                            endpoint_info,
                            ..
                        } => {
                            peers.insert(endpoint_info.endpoint_id);
                        }
                        iroh_mdns_address_lookup::DiscoveryEvent::Expired { endpoint_id } => {
                            peers.remove(&endpoint_id);
                        }
                        _ => {}
                    }
                }
            });
        }
        Err(e) => tracing::warn!(error = %e, "mDNS discovery unavailable"),
    }
    let shared = Arc::new(Shared {
        signing: ed25519_dalek::SigningKey::from_bytes(&secret),
        sessions: Mutex::new(HashMap::new()),
        blobs: Mutex::new(HashMap::new()),
    });
    let router = Router::builder(endpoint)
        .accept(
            ALPN,
            server::ShareProtocol {
                state: state.clone(),
                shared: shared.clone(),
            },
        )
        .spawn();
    Ok(ShareNet {
        router,
        relay: tokio::sync::Mutex::new(None),
        lan_peers,
    })
}

/// The proxy sharing follows, for the settings status line (FR-051), without credentials.
pub fn followed_proxy() -> Option<String> {
    let mut url = match proxy::choose() {
        proxy::Choice::Http(url) | proxy::Choice::Socks(url) => url,
        proxy::Choice::None => return None,
    };
    let _ = url.set_password(None);
    let _ = url.set_username("");
    Some(url.to_string())
}

/// Relay TLS trusts what the operating system trusts, so a TLS-inspecting proxy whose CA the
/// system already trusts works too; a host without a certificate store uses the bundled roots.
pub fn relay_tls() -> iroh::tls::CaTlsConfig {
    match lanrurugi_core::http_client::root_source() {
        lanrurugi_core::http_client::RootSource::System => iroh::tls::CaTlsConfig::system(),
        lanrurugi_core::http_client::RootSource::Bundled => iroh::tls::CaTlsConfig::embedded(),
    }
}

/// The remote's address on a direct path, if one is open. A relayed connection exposes only the
/// relay, which must never stand in for the peer's address (research §4).
pub fn direct_address(conn: &Connection) -> Option<IpAddr> {
    let paths = conn.paths();
    let mut fallback = None;
    for path in paths.iter() {
        if let iroh::TransportAddr::Ip(addr) = path.remote_addr() {
            if path.is_selected() {
                return Some(addr.ip());
            }
            fallback = Some(addr.ip());
        }
    }
    fallback
}

/// Waits up to `limit` for a direct path to appear.
pub async fn wait_for_direct_address(conn: &Connection, limit: Duration) -> Option<IpAddr> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(ip) = direct_address(conn) {
            return Some(ip);
        }
        if Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// Reads one length-prefixed frame.
pub async fn read_frame<T: serde::de::DeserializeOwned>(
    recv: &mut RecvStream,
) -> Result<T, ShareNetError> {
    let mut prefix = [0u8; 4];
    recv.read_exact(&mut prefix)
        .await
        .map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    let len = framing::body_len(prefix).map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    let mut body = vec![0u8; len];
    recv.read_exact(&mut body)
        .await
        .map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    framing::decode(&body).map_err(|e| ShareNetError::Protocol(e.to_string()))
}

pub async fn write_frame<T: serde::Serialize>(
    send: &mut SendStream,
    message: &T,
) -> Result<(), ShareNetError> {
    let frame = framing::encode(message).map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    send.write_all(&frame)
        .await
        .map_err(|e| ShareNetError::Protocol(e.to_string()))
}

/// Sends one request on a fresh stream and returns the response header, plus the stream for a
/// body that follows it. A `Refused` answer becomes an error.
pub async fn call(
    conn: &Connection,
    request: Request,
) -> Result<(Response, RecvStream), ShareNetError> {
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| ShareNetError::Unreachable(e.to_string()))?;
    write_frame(&mut send, &Envelope::new(request)).await?;
    send.finish()
        .map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    match read_frame::<Response>(&mut recv).await? {
        Response::Refused(reason) => Err(ShareNetError::Refused(reason)),
        response => Ok((response, recv)),
    }
}

/// Reads a response body announced as `len` bytes.
pub async fn read_body(
    recv: &mut RecvStream,
    len: u64,
    max: u64,
) -> Result<Vec<u8>, ShareNetError> {
    // `len` is the peer's own claim: refuse one that no answer could legitimately carry, before
    // reserving anything for it.
    if len > max {
        return Err(ShareNetError::Protocol(format!(
            "body of {len} bytes exceeds the {max}-byte limit"
        )));
    }
    let mut body = Vec::with_capacity(len.min(64 * 1024 * 1024) as usize);
    recv.take(len)
        .read_to_end(&mut body)
        .await
        .map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    if body.len() as u64 != len {
        return Err(ShareNetError::Protocol("truncated body".into()));
    }
    Ok(body)
}

/// Dials a sharer.
pub async fn connect(net: &ShareNet, addr: EndpointAddr) -> Result<Connection, ShareNetError> {
    tokio::time::timeout(Duration::from_secs(30), net.endpoint().connect(addr, ALPN))
        .await
        .map_err(|_| ShareNetError::Unreachable("timed out".into()))?
        .map_err(|e| ShareNetError::Unreachable(e.to_string()))
}

/// The endpoint address for a `via` hint or a knock record.
pub fn endpoint_addr(
    endpoint_id: &[u8; 32],
    relays: &[String],
    direct: Option<SocketAddr>,
) -> Option<EndpointAddr> {
    let id = EndpointId::from_bytes(endpoint_id).ok()?;
    let mut addr = EndpointAddr::new(id);
    for relay in relays {
        if let Ok(url) = relay.parse::<RelayUrl>() {
            addr = addr.with_relay_url(url);
        }
    }
    if let Some(direct) = direct {
        addr = addr.with_ip_addr(direct);
    }
    Some(addr)
}

/// Lowercase hex of an endpoint id, as `via` hints and node strings carry it.
pub fn endpoint_hex(id: &EndpointId) -> String {
    id.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_endpoint_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}
