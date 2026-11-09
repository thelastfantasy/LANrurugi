//! The running endpoint: what a rebuild replaces and what the protocol handlers borrow.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::protocol::Router;
use iroh::{Endpoint, EndpointId};
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
