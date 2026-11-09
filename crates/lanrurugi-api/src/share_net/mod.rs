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
pub use lanrurugi_share::interfaces;
pub mod limits;
pub use lanrurugi_share::link;
pub mod nodes;
pub use lanrurugi_share::proxy;
pub mod publish;
pub mod server;
pub use lanrurugi_share::status;

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::Router;
use iroh::{EndpointAddr, EndpointId, RelayMap};

use lanrurugi_share_proto::framing;
use lanrurugi_share_proto::messages::{RefusalReason, Request, Response, ShareEngine};
use tokio::io::AsyncReadExt;
use tokio::sync::RwLock;

use crate::share_settings::ShareSettings;
use crate::AppState;

pub use lanrurugi_share::net::ALPN;
/// Public rendezvous relays used while public services are on and none are configured.
/// Two operators, so one going away leaves lookups working; records go to and are asked of both.
/// Neither runs the default relays, so no single operator sees both a lookup and the connection
/// that follows it.
pub const DEFAULT_PKARR_RELAYS: [&str; 2] = ["https://pkarr.pubky.app", "https://relay.pkarr.org"];
/// How long a claim on an IP-restricted share waits for a direct path (research §4).
pub const DIRECT_PATH_WAIT: Duration = Duration::from_secs(15);
/// How long one request waits for its answer. A peer that accepts the connection and then says
/// nothing would otherwise hold the caller open indefinitely.
const ANSWER_TIMEOUT_SECS: u64 = 60;

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

pub use lanrurugi_share::net::{RatioState, Session, ShareNet, Shared, SESSION_TTL};

/// Transfers currently moving bytes. Port rotation waits for this to reach zero: rebinding the
/// endpoint tears down every connection on it, and a rotation in the middle of a transfer would be
/// indistinguishable from a network failure to both sides.
static ACTIVE_TRANSFERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Marks a transfer as in flight until dropped, so rotation can wait for it.
pub(crate) struct TransferGuard;

impl TransferGuard {
    pub(crate) fn begin() -> Self {
        ACTIVE_TRANSFERS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        ACTIVE_TRANSFERS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

pub(crate) fn active_transfers() -> usize {
    ACTIVE_TRANSFERS.load(std::sync::atomic::Ordering::Relaxed)
}

/// A free port the OS hands out; used when a port setting is empty ("automatic").
fn free_port() -> Result<u16, String> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| e.to_string())?;
    socket
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| e.to_string())
}

/// `share_external_addrs`: addresses to advertise as direct addresses. A bare address takes the
/// endpoint's own port.
fn external_addrs(settings: &ShareSettings, port: u16) -> Vec<SocketAddr> {
    settings
        .list("share_external_addrs")
        .iter()
        .filter_map(|entry| match entry.parse::<SocketAddr>() {
            Ok(addr) => Some(addr),
            Err(_) => entry
                .parse::<std::net::IpAddr>()
                .ok()
                .map(|ip| SocketAddr::new(ip, port)),
        })
        .collect()
}

/// The port to bind this time: the pinned one, or a fresh random one per start when unset.
fn port_for(settings: &ShareSettings, key: &str) -> Result<u16, String> {
    match settings.text(key).trim() {
        "" => free_port(),
        raw => raw
            .parse::<u16>()
            .map_err(|_| format!("\"{key}\" is not a port: {raw}")),
    }
}

static RUNNING: RwLock<Option<Arc<ShareNet>>> = RwLock::const_new(None);

/// The running endpoint, if sharing has started.
pub async fn handle() -> Option<Arc<ShareNet>> {
    RUNNING.read().await.clone()
}

/// The relays this instance listens on and advertises (FR-027, FR-048, FR-055).
pub use lanrurugi_share::net::{relay_settings_key, relay_urls};

pub async fn start(state: &AppState) -> Result<Arc<ShareNet>, String> {
    let settings = crate::share_settings::load(state).await;
    let secret = crate::shares::node_secret(state).await?;
    // The old endpoint holds the UDP port (and the relay role its TCP port): release them before
    // binding again, or every rebuild after a settings change fails to bind.
    let relay_key = relay_settings_key(&settings);
    let mut kept_relay: Option<(iroh_relay::server::Server, u16, EndpointId)> = None;
    let mut running = RUNNING.write().await;
    if let Some(previous) = running.take() {
        previous.flush_ratios(state).await;
        // Closing the endpoint is what actually releases the UDP socket: dropping our `Arc` is not
        // enough while a spawned task (a republisher, an importer) still holds one, and a fixed
        // port that never comes free would make every settings change pick a new port — and so a
        // new address in every published record and share code.
        previous.endpoint().close().await;
        let previous_id = previous.endpoint().id();
        let _ = previous.router.shutdown().await;
        let previous_port = *previous.relay_port.lock().await;
        let unchanged = previous.relay_key.lock().await.as_deref() == Some(relay_key.as_str());
        let mut relay_slot = previous.relay.lock().await;
        if settings.flag("share_node_relay") && unchanged {
            if let (Some(server), Some(port)) = (relay_slot.take(), previous_port) {
                kept_relay = Some((server, port, previous_id));
            }
        }
        if let Some(relay) = relay_slot.take() {
            let _ = relay.shutdown().await;
        }
    }
    // A fixed port keeps a forwarded port working across restarts; if something else already
    // holds it (a second instance, say), fall back to a free one instead of leaving sharing off.
    // An empty setting means "random every time", which still retries the draw: a free port found
    // a moment ago can be taken by the time QUIC binds it.
    let mut fallback = settings.text("share_udp_port").trim().is_empty();
    let mut attempt = 0u32;
    let net = loop {
        let port = if fallback {
            free_port()?
        } else {
            port_for(&settings, "share_udp_port")?
        };
        match bind(state, &settings, secret, port).await {
            Ok(net) => break Arc::new(net),
            Err(e) if attempt < 24 => {
                attempt += 1;
                if !fallback {
                    // The endpoint this one replaces still holds the socket until the tasks that
                    // borrowed it (republisher, in-flight imports) let go, which takes a few
                    // seconds. Waiting is what keeps the configured port — and with it every
                    // published address — stable across a settings change.
                    if attempt < 20 {
                        tracing::debug!(port, error = %e, attempt, "the sharing port is not free yet");
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    } else {
                        fallback = true;
                        tracing::warn!(
                            port,
                            error = %e,
                            "the configured sharing port is unavailable; picking a free one"
                        );
                    }
                } else {
                    tracing::debug!(error = %e, attempt, "retrying a random sharing port");
                }
            }
            Err(e) => return Err(e),
        }
    };
    *running = Some(net.clone());
    drop(running);
    if settings.flag("share_node_relay") {
        // A kept relay is bound to the endpoint identity it was started for; that identity only
        // changes when the node secret does, and then the relay has to be rebuilt.
        match kept_relay.take() {
            Some((server, port, previous_id)) if previous_id == net.id() => {
                *net.relay.lock().await = Some(server);
                *net.relay_port.lock().await = Some(port);
                *net.relay_key.lock().await = Some(relay_key.clone());
                tracing::info!(port, "kept the running relay across the endpoint rebuild");
            }
            other => {
                if let Some((server, _, _)) = other {
                    let _ = server.shutdown().await;
                }
                let dir = state.library.temp_dir.join("share");
                match nodes::start_relay(&settings, &dir, net.id()).await {
                    Ok((server, port)) => {
                        *net.relay.lock().await = Some(server);
                        *net.relay_port.lock().await = Some(port);
                        *net.relay_key.lock().await = Some(relay_key.clone());
                    }
                    Err(e) => tracing::warn!(error = %e, "relay role could not start"),
                }
            }
        }
    }
    // The node record a peer's 16-character code resolves to: republished on every (re)start, so
    // it always carries the addresses the endpoint actually has right now.
    if settings.flag("share_node_relay") || settings.flag("share_node_rendezvous") {
        nodes::publish_node_record(state, &net, &settings).await;
        nodes::spawn_record_refresh(state.clone(), net.clone());
    }
    tracing::info!(endpoint = %net.id(), "sharing endpoint started");
    Ok(net)
}

/// Re-binds the endpoint on a fresh random port every `share_port_rotate` minutes, jittered ±25%
/// and clamped to 6–12 hours so the interval itself is not a schedule a scanner can rely on. Never
/// cuts a live transfer: it waits for the in-flight count to reach zero first, and skips the round
/// if a transfer is still running after half an hour.
/// The background loops that ride on a running endpoint. Spawned once per process, however many
/// times the endpoint itself is rebuilt.
static SERVICES: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// Starts the endpoint and, the first time only, the loops that ride on it.
pub async fn start_services(state: &AppState) -> Result<(), String> {
    start(state).await?;
    // 022 US1/T011: a rebuild is how an interface's new address takes effect, and the record
    // published under the node code still names the old one until something republishes it. Do it
    // here, so every rebuild (address change, port rotation, settings save) republishes at once
    // instead of waiting for the hourly pass.
    if let Some(net) = handle().await {
        publish::publish_all(state, &net).await;
    }
    if SERVICES.set(()).is_ok() {
        blobs::spawn_prepare_worker(state.clone());
        helper::spawn(state.clone());
        publish::spawn_republisher(state.clone());
        spawn_port_rotation(state.clone());
    }
    Ok(())
}

/// Brings sharing back when the interface it is pinned to appears. A share bound to a VPN tunnel
/// must not serve from the physical link while that tunnel is down, so the endpoint waits instead
/// of falling back, and this retries until it can start.
/// Rate limit for moving back to the configured port, so a port held by something else does not
/// turn into an endpoint rebuild every fifteen seconds.
fn port_retry_due() -> bool {
    use std::sync::Mutex;
    use std::time::Instant;
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let Ok(mut last) = LAST.lock() else {
        return false;
    };
    let now = Instant::now();
    match *last {
        Some(previous) if now.duration_since(previous) < Duration::from_secs(60) => false,
        _ => {
            *last = Some(now);
            true
        }
    }
}

pub fn spawn_interface_watch(state: AppState) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            if let Some(running) = handle().await {
                // Still running, but a VPN reconnect may have moved the address it is bound to:
                // rebind when the interface no longer offers what this endpoint listens on.
                let settings = crate::share_settings::load(&state).await;
                // A rebuild that had to fall back to a random port (the socket was still held by
                // the endpoint it replaced) would otherwise keep that port until the next restart,
                // silently invalidating the user's port forward. Come back to the configured port
                // once it is free again, at most once a minute.
                if let Some(wanted) = settings.number("share_udp_port") {
                    let wanted = wanted as u16;
                    if wanted != 0 && wanted != running.udp_port && port_retry_due() {
                        tracing::info!(
                            port = wanted,
                            current = running.udp_port,
                            "the configured sharing port is free again; moving back to it"
                        );
                        let _ = start_services(&state).await;
                        continue;
                    }
                }
                let pinned = settings.text("share_bind_interface");
                if pinned.trim().is_empty() || running.bound_addrs.is_empty() {
                    continue;
                }
                let now: Vec<std::net::IpAddr> =
                    match interfaces::bind_addrs(pinned, running.udp_port) {
                        Some(addrs) => addrs.into_iter().map(|a| a.ip()).collect(),
                        None => Vec::new(),
                    };
                if now == running.bound_addrs {
                    continue;
                }
                tracing::info!(
                    interface = pinned,
                    "the sharing interface changed address; rebuilding the endpoint"
                );
                match start_services(&state).await {
                    Ok(()) => continue,
                    Err(error) => {
                        tracing::warn!(error = %error, "sharing endpoint could not be rebound");
                        continue;
                    }
                }
            }
            let settings = crate::share_settings::load(&state).await;
            let pinned = settings.text("share_bind_interface").to_string();
            if !pinned.trim().is_empty() && !interfaces::exists(&pinned) {
                continue;
            }
            match start_services(&state).await {
                Ok(()) => tracing::info!(interface = pinned, "sharing endpoint is back"),
                Err(error) => tracing::warn!(error = %error, "sharing endpoint could not start"),
            }
        }
    });
}

pub fn spawn_port_rotation(state: AppState) {
    tokio::spawn(async move {
        loop {
            let minutes = crate::share_settings::load(&state)
                .await
                .number("share_port_rotate")
                .unwrap_or(0);
            if minutes == 0 {
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
            let base = minutes * 60;
            let mut target = jittered(base);
            // Re-read the interval every few seconds instead of sleeping the whole wait: changing
            // the setting must take effect now, not after the round the old value already planned.
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let now = crate::share_settings::load(&state)
                    .await
                    .number("share_port_rotate")
                    .unwrap_or(0);
                if now != minutes {
                    break;
                }
                target = target.saturating_sub(15);
                if target == 0 {
                    break;
                }
            }
            if crate::share_settings::load(&state)
                .await
                .number("share_port_rotate")
                .unwrap_or(0)
                == 0
            {
                continue;
            }
            // A rebind drops every connection to this endpoint, so it waits not just for a
            // response being written but for any import this instance is running: those hold a
            // live connection to a sharer and would have to start over.
            let mut waited = 0u32;
            let mut announced = false;
            while (active_transfers() > 0 || importing(&state).await) && waited < 30 * 60 {
                if !announced {
                    tracing::info!(
                        "port rotation deferred: a transfer or an active claim is still running"
                    );
                    announced = true;
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
                waited += 10;
            }
            if active_transfers() > 0 || importing(&state).await {
                tracing::info!("port rotation skipped: a transfer is still running");
                continue;
            }
            match start(&state).await {
                Ok(net) => {
                    tracing::info!(udp = net.udp_port(), "sharing ports rotated");
                    // Publish the new addresses now: the periodic refresher is up to ten minutes
                    // away, and until it runs peers dial the port that just went away.
                    let settings = crate::share_settings::load(&state).await;
                    nodes::publish_node_record(&state, &net, &settings).await;
                }
                Err(e) => tracing::warn!(error = %e, "port rotation failed"),
            }
        }
    });
}

/// Whether this instance is in the middle of importing a share right now.
async fn importing(state: &AppState) -> bool {
    use lanrurugi_storage::shares::ImportState;
    import::repo(state)
        .list()
        .await
        .map(|all| {
            all.iter()
                .any(|r| matches!(r.state, ImportState::Transferring | ImportState::Rebuilding))
        })
        .unwrap_or(false)
}

/// A rotation interval in seconds, jittered ±25% with the clock's own sub-second noise as the
/// entropy, so the schedule is never a fixed period a scanner could wait out.
const ROTATE_MIN_SECS: u64 = 6 * 60 * 60;
const ROTATE_MAX_SECS: u64 = 12 * 60 * 60;

fn jittered(base: u64) -> u64 {
    let jitter = (base / 4).max(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(0);
    let drawn = base.saturating_sub(jitter) + nanos % (2 * jitter + 1);
    drawn.clamp(ROTATE_MIN_SECS, ROTATE_MAX_SECS)
}

/// Binds an endpoint serving the share protocol for `state`. Tests bind several in one process.
pub async fn bind(
    state: &AppState,
    settings: &ShareSettings,
    secret: [u8; 32],
    port: u16,
) -> Result<ShareNet, String> {
    let mut bound = lanrurugi_share::net::bind(lanrurugi_share::net::BindOptions {
        secret,
        port,
        relay_map: relay_urls(settings).into_iter().collect::<RelayMap>(),
        pinned: settings.text("share_bind_interface").to_string(),
        portmap: settings.flag("share_portmap"),
        external_addrs: external_addrs(settings, port),
    })
    .await?;
    let endpoint = bound.take_endpoint();
    let shared = bound.shared.clone();
    let router = Router::builder(endpoint)
        .accept(
            ALPN,
            server::ShareProtocol {
                state: state.clone(),
                shared,
            },
        )
        .spawn();
    Ok(ShareNet::from_bound(bound, router, port))
}

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

/// Records the one activity entry an import gets, on whichever side the bytes moved.
///
/// The detail (bytes, route, interface, tunnel, addresses) lives in `after`, which the activity
/// detail panel shows — deliberately one entry per import rather than one per connection or
/// piece, so the list stays readable and the noise stays where it belongs: the log.
pub(crate) async fn note_transfer(
    state: &AppState,
    sent: bool,
    share_id: &str,
    title: &str,
    bytes: u64,
    link: &link::Link,
    extra: serde_json::Value,
) {
    use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
    let mut after = serde_json::json!({
        "bytes": bytes,
        "route": link.route,
        "local": link.local,
        "remote": link.remote,
        "interface": link.interface,
        "vpn": link.tunnel,
        "technique": link.technique(),
    });
    if let (Some(map), Some(extra)) = (after.as_object_mut(), extra.as_object()) {
        for (k, v) in extra {
            map.insert(k.clone(), v.clone());
        }
    }
    let target = ActivityTarget {
        id: Some(share_id.to_string()),
        label: Some(title.to_string()),
        kind: Some("share".to_string()),
    };
    crate::activity::record_automatic(
        state,
        "share",
        if sent {
            action_types::SHARE_SEND
        } else {
            action_types::SHARE_RECEIVE
        },
        target,
        Outcome::Success,
        Some(after),
        None,
    )
    .await;
}

/// Reads one frame: the sender's engine and the message. The engine is read before the payload is
/// allocated, so a version check never depends on decoding a message this build may not know.
pub async fn read_frame<T: serde::de::DeserializeOwned>(
    recv: &mut RecvStream,
) -> Result<(ShareEngine, T), ShareNetError> {
    // Reading bytes is a transport concern: a reset stream, a dropped relay or a VPN rekey shows up
    // here, and 022 US2 requires those to be retried, not to fail the import. Only a frame this
    // build cannot decode (below) is `Protocol`.
    fn read_err(e: impl std::fmt::Display) -> ShareNetError {
        ShareNetError::Unreachable(e.to_string())
    }
    let mut prefix = [0u8; 4];
    recv.read_exact(&mut prefix).await.map_err(read_err)?;
    let total = framing::body_len(prefix).map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    let mut head = [0u8; framing::ENGINE_LEN_BYTES];
    recv.read_exact(&mut head).await.map_err(read_err)?;
    let engine_len = u16::from_be_bytes(head) as usize;
    if framing::ENGINE_LEN_BYTES + engine_len > total {
        return Err(ShareNetError::Protocol("malformed frame".into()));
    }
    let mut engine = vec![0u8; engine_len];
    recv.read_exact(&mut engine).await.map_err(read_err)?;
    let engine =
        framing::decode_engine(&engine).map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    let mut body = vec![0u8; total - framing::ENGINE_LEN_BYTES - engine_len];
    recv.read_exact(&mut body).await.map_err(read_err)?;
    let message =
        framing::decode_payload(&body).map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    Ok((engine, message))
}

pub async fn write_frame<T: serde::Serialize>(
    send: &mut SendStream,
    message: &T,
) -> Result<(), ShareNetError> {
    let frame = framing::encode(message).map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    send.write_all(&frame)
        .await
        .map_err(|e| ShareNetError::Unreachable(e.to_string()))
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
    write_frame(&mut send, &request).await?;
    send.finish()
        .map_err(|e| ShareNetError::Unreachable(e.to_string()))?;
    let (engine, response) = tokio::time::timeout(
        Duration::from_secs(ANSWER_TIMEOUT_SECS),
        read_frame::<Response>(&mut recv),
    )
    .await
    .map_err(|_| ShareNetError::Unreachable("the sharer did not answer in time".into()))??;
    // The responder's own version, checked on every answer: a peer that answers at all still has to
    // be speaking this engine, or the message it just sent is not one this build understands.
    if !engine.is_compatible_with(&ShareEngine::current()) {
        return Err(ShareNetError::Refused(RefusalReason::UnsupportedVersion {
            engine,
        }));
    }
    match response {
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
        .map_err(|e| ShareNetError::Unreachable(e.to_string()))?;
    if body.len() as u64 != len {
        // The peer announced more than it sent: the stream ended early, which is transmission loss
        // (retry), not a frame this build cannot parse.
        return Err(ShareNetError::Unreachable("truncated body".into()));
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

pub use lanrurugi_share::net::endpoint_addr;

pub use lanrurugi_share::nodes::{endpoint_hex, parse_endpoint_hex};
