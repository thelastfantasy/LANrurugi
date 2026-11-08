//! Where a share connection actually runs, as the kernel reports it.
//!
//! The activity log and the log line both say whether the bytes went over the LAN or through a
//! tunnel. This reads the selected path's own local socket address and maps it back to an
//! interface, so "through the VPN" is an observation rather than an inference from the settings.

use iroh::endpoint::{Connection, LocalTransportAddr};

/// The selected path of one live connection.
#[derive(Debug, Clone)]
pub struct Link {
    pub local: String,
    pub remote: String,
    pub route: &'static str,
    /// The local interface carrying it, when the kernel reports one.
    pub interface: Option<String>,
    /// True when that interface is a tunnel (`tun0`, `wg0`, `utun3`, …).
    pub tunnel: bool,
}

impl Link {
    /// The traversal technique in words, for the activity entry: how the two endpoints reached
    /// each other, and whether that path was itself inside a tunnel.
    pub fn technique(&self) -> String {
        let base = if self.route == "relay" {
            "QUIC relay"
        } else if self.remote == "-" {
            "unknown"
        } else if self.remote.contains('.') {
            "direct IPv4"
        } else if self.remote.contains(':') {
            "direct IPv6"
        } else {
            "direct"
        };
        if self.tunnel && base != "unknown" {
            format!("{base} over VPN")
        } else {
            base.to_string()
        }
    }
}

/// A link nobody could describe — the connection closed before it could be read, or the peer
/// vanished first. Recording it still beats dropping the entry that says how much moved.
pub fn unknown() -> Link {
    Link {
        local: "-".to_string(),
        remote: "-".to_string(),
        route: "unknown",
        interface: None,
        tunnel: false,
    }
}

/// Reads the connection's current path. Cheap enough for once per connection, wrong to call per
/// piece — the path can change mid-transfer, but a per-piece answer would say nothing new.
pub fn of(conn: &Connection) -> Link {
    let paths = conn.paths();
    let path = paths
        .iter()
        .find(|p| p.is_selected())
        .or_else(|| paths.iter().next());
    let Some(path) = path else {
        return Link {
            local: "-".to_string(),
            remote: "-".to_string(),
            route: "unknown",
            interface: None,
            tunnel: false,
        };
    };
    let route = if path.is_relay() { "relay" } else { "direct" };
    let remote = match path.remote_addr() {
        iroh::TransportAddr::Ip(addr) => addr.to_string(),
        iroh::TransportAddr::Relay(url) => url.to_string(),
        other => format!("{other:?}"),
    };
    let (local, interface) = match path.local_addr() {
        LocalTransportAddr::Ip(Some(ip)) => {
            let wanted = ip.to_string();
            let name = super::interfaces::list()
                .into_iter()
                .find(|iface| iface.addrs.iter().any(|addr| addr == &wanted))
                .map(|iface| iface.name);
            (wanted, name)
        }
        LocalTransportAddr::Ip(None) => ("-".to_string(), None),
        LocalTransportAddr::Relay(url) => (url.to_string(), None),
        LocalTransportAddr::Custom(_) => ("custom".to_string(), None),
        _ => ("-".to_string(), None),
    };
    let tunnel = interface.as_deref().is_some_and(is_tunnel);
    Link {
        local,
        remote,
        route,
        interface,
        tunnel,
    }
}

/// Interface names that mean the packets are encapsulated. A name is a guess, but the kernel
/// offers no better portable signal, and every one of these is a real tunnel type on Linux/BSD.
fn is_tunnel(name: &str) -> bool {
    const PREFIXES: [&str; 9] = [
        "tun",
        "tap",
        "wg",
        "ppp",
        "utun",
        "ipsec",
        "tailscale",
        "nebula",
        "zt",
    ];
    let name = name.to_ascii_lowercase();
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}
