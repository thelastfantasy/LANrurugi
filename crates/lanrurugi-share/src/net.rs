//! The running endpoint: what a rebuild replaces and what the protocol handlers borrow.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId, RelayMap, RelayMode, RelayUrl, SecretKey};

use crate::proxy;
use crate::settings::ShareSettings;
use lanrurugi_core::ids::{ArchiveId, ShareId};

/// How long an `Open` session stays usable.
pub const SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);

/// What an `Open` session grants: browsing one share, from one remote endpoint.
#[derive(Debug, Clone)]
pub struct Session {
    pub share_id: ShareId,
    pub remote: EndpointId,
    pub created: Instant,
}

/// State shared by the endpoint's protocol handler and the rest of the server.
pub struct Shared {
    pub signing: ed25519_dalek::SigningKey,
    pub sessions: Mutex<HashMap<String, Session>>,
    /// blake3 → archive, for answering `Range` without a library scan.
    pub blobs: Mutex<HashMap<[u8; 32], ArchiveId>>,
    /// Served import bytes per share. A `Range` answers one 1 MiB piece, so the ratio is kept in
    /// memory and only written back to Redis every few megabytes.
    pub ratios: Mutex<HashMap<ShareId, RatioState>>,
}

/// How much of a share has gone out to importers, and what to do once its target is reached.
#[derive(Debug, Clone, Default)]
pub struct RatioState {
    /// Bytes served to importers so far, including those not yet written back to Redis.
    pub imported: u64,
    /// Size of everything the share offers; zero until the first claim measured it.
    pub total: u64,
    pub target: Option<f32>,
    pub action: lanrurugi_storage::shares::RatioAction,
    pub rate: Option<u64>,
    /// The value `imported` had when it was last written to Redis.
    pub flushed: u64,
}

impl RatioState {
    /// The byte count the target ratio names, if there is one to enforce.
    pub fn limit(&self) -> Option<u64> {
        let target = self.target?;
        (self.total > 0).then_some((target as f64 * self.total as f64) as u64)
    }
}

impl Shared {
    pub fn session(&self, token: &str, remote: EndpointId) -> Option<Session> {
        let mut sessions = self.sessions.lock().expect("sessions lock");
        sessions.retain(|_, s| s.created.elapsed() < SESSION_TTL);
        sessions.get(token).filter(|s| s.remote == remote).cloned()
    }
}

pub struct ShareNet {
    pub router: Router,
    /// Sessions, blob map and unflushed ratio counters. Kept here so a rebuild can write the
    /// counters back before the endpoint (and this map) goes away.
    pub shared: Arc<Shared>,
    /// The embedded relay of the relay role, when offered.
    pub relay: tokio::sync::Mutex<Option<iroh_relay::server::Server>>,
    /// Endpoints mDNS has seen on the local network, asked by `ResolveLocal`.
    pub lan_peers: Arc<Mutex<std::collections::HashSet<EndpointId>>>,
    /// The UDP port the endpoint actually bound. With an automatic port this is only known after
    /// binding, and it changes on every rotation, so it is reported rather than inferred.
    pub udp_port: u16,
    /// The addresses this endpoint bound, kept so the watcher can notice the interface changed
    /// under it (a VPN reconnect hands out a new address while sharing is still "running").
    pub bound_addrs: Vec<std::net::IpAddr>,
    /// The TCP port the embedded relay actually listens on, when the relay role is on.
    pub relay_port: tokio::sync::Mutex<Option<u16>>,
    /// The relay-relevant settings the running relay was started from. A rebuild that leaves them
    /// untouched keeps the relay instead of dropping every peer connected through it.
    pub relay_key: tokio::sync::Mutex<Option<String>>,
}

impl ShareNet {
    /// Writes back served bytes still counted only in memory. A rebuild replaces the endpoint and
    /// this map with it, so without this every rotation silently loses a few megabytes of ratio.
    pub async fn flush_ratios(&self, host: &dyn crate::host::ShareHost) {
        let pending: Vec<(lanrurugi_core::ids::ShareId, u64)> = {
            let mut ratios = self.shared.ratios.lock().expect("ratios lock");
            ratios
                .iter_mut()
                .filter_map(|(id, ratio)| {
                    let pending = ratio.imported.saturating_sub(ratio.flushed);
                    if pending == 0 {
                        return None;
                    }
                    ratio.flushed = ratio.imported;
                    Some((id.clone(), pending))
                })
                .collect()
        };
        for (id, delta) in pending {
            host.add_import_bytes(&id, delta).await;
        }
    }

    pub fn endpoint(&self) -> &Endpoint {
        self.router.endpoint()
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint().id()
    }

    /// The UDP port this endpoint bound, for the settings page.
    pub fn udp_port(&self) -> u16 {
        self.udp_port
    }

    /// The addresses the endpoint was actually bound to (empty when it follows the host). 022 US1:
    /// a pinned endpoint must never publish an address it is not listening on.
    pub fn bound_addrs(&self) -> &[std::net::IpAddr] {
        &self.bound_addrs
    }

    /// The relay role's TCP port, when it is running.
    pub async fn relay_port(&self) -> Option<u16> {
        *self.relay_port.lock().await
    }
}

/// Everything `bind` needs from the caller that is not its own business: the relay map is built
/// from settings (api-side), while the sockets, proxy bridge and mDNS are the engine's.
pub struct BindOptions {
    pub secret: [u8; 32],
    pub port: u16,
    pub relay_map: RelayMap,
    pub pinned: String,
    pub portmap: bool,
    /// Addresses a manual port forward maps to this host (`share_external_addrs`).
    pub external_addrs: Vec<std::net::SocketAddr>,
}

/// The endpoint before any protocol handler is attached to it.
pub struct Bound {
    endpoint: Option<Endpoint>,
    pub shared: Arc<Shared>,
    pub bound_addrs: Vec<std::net::IpAddr>,
    pub lan_peers: Arc<Mutex<std::collections::HashSet<EndpointId>>>,
}

/// Builds and binds the endpoint. The caller attaches the protocol handler and turns this into a
/// `ShareNet`, because the handler is the part that still lives with the application.
pub async fn bind(options: BindOptions) -> Result<Bound, String> {
    let secret_key = SecretKey::from_bytes(&options.secret);
    // Never `Disabled`: with no relay of its own the endpoint must still dial through the relay a
    // sharer names in its knock record or `via` hint (an own tunnel, a friend's node).
    let mut builder = Endpoint::builder(presets::Minimal)
        .secret_key(secret_key)
        .alpns(vec![ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(options.relay_map));
    // A named interface (a VPN tunnel, say) binds only its own addresses: that is the whole
    // point — traffic must not fall back to the physical link. IPv6 is bound as well where the
    // host has it, without failing on IPv4-only hosts.
    let pinned = options.pinned.as_str();
    let device = pinned.trim();
    let device = (!device.is_empty()).then_some(device);
    netwatch::set_bind_device(device);
    iroh_relay::client::set_bind_device(device);
    let mut bound_addrs: Vec<std::net::IpAddr> = Vec::new();
    if !pinned.trim().is_empty() {
        // The builder starts with a wildcard IPv4 and IPv6 transport. Binding the tunnel's own
        // address only replaces the transport of that address family, so on an IPv4-only tunnel
        // iroh would still listen on `[::]` everywhere, send IPv6 over the physical link and then
        // publish that real address as a direct address. Drop them first.
        builder = builder.clear_ip_transports();
    }
    match crate::interfaces::bind_addrs(pinned, options.port) {
        Some(addrs) => {
            for addr in addrs {
                if addr.ip().is_unspecified() {
                    bound_addrs.clear();
                } else {
                    bound_addrs.push(addr.ip());
                }
                builder = builder.bind_addr(addr).map_err(|e| e.to_string())?;
            }
        }
        None => {
            // Pinned to an interface that is not there: binding everything would serve from the
            // physical link, which is exactly what pinning exists to prevent.
            if !pinned.trim().is_empty() {
                return Err(format!(
                    "the chosen network interface {pinned} is not present; sharing stays off until it is"
                ));
            }
            builder = builder
                .bind_addr(std::net::SocketAddr::from(([0, 0, 0, 0], options.port)))
                .map_err(|e| e.to_string())?
                .bind_addr_with_opts(
                    std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, options.port)),
                    iroh::endpoint::BindOpts::default().set_is_required(false),
                )
                .map_err(|e| e.to_string())?;
        }
    }
    // The user's own proxy: iroh only takes an HTTP proxy, so SOCKS goes through a local bridge.
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
    if !options.portmap {
        builder = builder.portmapper_config(iroh::endpoint::PortmapperConfig::Disabled);
    }
    let endpoint = builder.bind().await.map_err(|e| e.to_string())?;
    for addr in options.external_addrs {
        endpoint.add_external_addr(addr).await;
    }
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
        ratios: Mutex::new(HashMap::new()),
        signing: ed25519_dalek::SigningKey::from_bytes(&options.secret),
        sessions: Mutex::new(HashMap::new()),
        blobs: Mutex::new(HashMap::new()),
    });
    Ok(Bound {
        endpoint: Some(endpoint),
        shared,
        bound_addrs,
        lan_peers,
    })
}

impl Bound {
    /// Hands the endpoint over to the caller, which attaches the protocol handler. Taking it out
    /// (rather than borrowing) is what lets `from_bound` still own the rest of the struct.
    pub fn take_endpoint(&mut self) -> Endpoint {
        self.endpoint.take().expect("the endpoint is taken once")
    }
}

impl ShareNet {
    /// Wraps an already-bound endpoint that has a protocol handler attached.
    pub fn from_bound(bound: Bound, router: Router, udp_port: u16) -> Self {
        Self {
            router,
            shared: bound.shared,
            relay: tokio::sync::Mutex::new(None),
            lan_peers: bound.lan_peers,
            udp_port,
            bound_addrs: bound.bound_addrs,
            relay_port: tokio::sync::Mutex::new(None),
            relay_key: tokio::sync::Mutex::new(None),
        }
    }
}

/// The ALPN every sharing connection uses.
pub const ALPN: &[u8] = b"h3";
/// Neutral mDNS service label; the library default names the transport.
pub const MDNS_SERVICE: &str = "lshr";

/// CA roots used for relay TLS: the system store or the bundled one, as configured.
pub fn relay_tls() -> iroh::tls::CaTlsConfig {
    match lanrurugi_core::http_client::root_source() {
        lanrurugi_core::http_client::RootSource::System => iroh::tls::CaTlsConfig::system(),
        lanrurugi_core::http_client::RootSource::Bundled => iroh::tls::CaTlsConfig::embedded(),
    }
}

pub fn relay_urls(settings: &ShareSettings) -> Vec<RelayUrl> {
    let node_relays: Vec<String> = crate::nodes::configured_nodes(settings)
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
                .then(|| crate::nodes::own_relay_url(settings))
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
        .filter_map(|s| crate::settings::canonical_https_url(s))
        .filter_map(|s| s.parse::<RelayUrl>().ok())
    {
        if !urls.contains(&url) {
            urls.push(url);
        }
    }
    urls
}

/// Starts (or restarts, after a settings change) this instance's endpoint.
/// The settings the embedded relay is built from. Unchanged across a rebuild, the relay is kept
/// running: restarting it drops every peer currently connected through it for no benefit.
pub fn relay_settings_key(settings: &ShareSettings) -> String {
    [
        "share_node_relay",
        "share_node_relay_port",
        "share_node_relay_tls",
        "share_node_relay_cert",
        "share_node_relay_key",
        "share_node_relay_allow",
        "share_node_relay_rate",
        "share_public_url",
        "share_self_relay_url",
    ]
    .iter()
    .map(|field| format!("{field}={}", settings.text(field)))
    .collect::<Vec<_>>()
    .join("\n")
}
