//! What a `via` code may advertise: only addresses this endpoint actually serves.

use std::net::{IpAddr, SocketAddr};

use lanrurugi_share_proto::code::Via;

use crate::net::ShareNet;
use crate::nodes::endpoint_hex;
use crate::settings::{canonical_https_url, ShareSettings};

/// Enough for a public IPv6, a mapped IPv4 and a LAN address or two, without a dense QR code.
const MAX_HINT_ADDRS: usize = 6;

pub fn via_hint(net: &ShareNet) -> Option<Via> {
    let endpoint = net.endpoint().addr();
    let bound = net.bound_addrs();
    let mut found: Vec<SocketAddr> = endpoint
        .ip_addrs()
        .copied()
        // 022 US1: with an interface pinned, only that interface's own addresses are dialable
        // through it. iroh can still see the host's other addresses (a public IPv6 on the physical
        // link was the historical leak), and publishing one sends peers to an address this
        // endpoint does not serve.
        .filter(|a| address_allowed(bound, a.ip()))
        .filter(|a| dialable(a.ip()))
        .collect();
    found.sort_by_key(|a| address_rank(a.ip()));
    let mut addrs: Vec<String> = Vec::new();
    for addr in found {
        let text = addr.to_string();
        if !addrs.contains(&text) && addrs.len() < MAX_HINT_ADDRS {
            addrs.push(text);
        }
    }
    let relays: Vec<String> = endpoint.relay_urls().map(|u| u.to_string()).collect();
    if addrs.is_empty() && relays.is_empty() {
        return None;
    }
    Some(Via {
        endpoint_id: endpoint_hex(&net.id()),
        addrs,
        relays,
    })
}

/// 022 US1: whether `ip` may be published when the endpoint is bound to `bound`. An empty `bound`
/// means the endpoint follows the host, so anything dialable it listens on may be published; a
/// pinned endpoint may only advertise the addresses of the interface it is pinned to.
pub fn address_allowed(bound: &[IpAddr], ip: IpAddr) -> bool {
    bound.is_empty() || bound.contains(&ip)
}

/// Whether another machine could ever reach `ip`: not loopback, unspecified or link-local.
pub fn dialable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_unspecified() || v4.is_link_local()),
        IpAddr::V6(v6) => {
            !(v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xffc0) == 0xfe80)
        }
    }
}

fn is_shared(v4: std::net::Ipv4Addr) -> bool {
    v4.octets()[0] == 100 && v4.octets()[1] & 0xc0 == 64
}

/// Public addresses first, then home LANs (`192.168/16`) ahead of the ranges container bridges
/// tend to take.
pub fn address_rank(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v4) if v4.octets()[0] == 192 && v4.octets()[1] == 168 => 1,
        // Private, or carrier-grade NAT's 100.64/10.
        IpAddr::V4(v4) if v4.is_private() || is_shared(v4) => 2,
        IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00 => 2,
        _ => 0,
    }
}

/// The public pkarr relays used when public services are on and nothing is configured.
pub const DEFAULT_PKARR_RELAYS: [&str; 2] = ["https://pkarr.pubky.app", "https://relay.pkarr.org"];

pub fn pkarr_relays(settings: &ShareSettings) -> Vec<String> {
    let configured: Vec<String> = settings
        .list("share_pkarr_relays")
        .iter()
        .filter_map(|r| canonical_https_url(r))
        .collect();
    if !configured.is_empty() {
        return configured;
    }
    if settings.flag("share_public_services") {
        DEFAULT_PKARR_RELAYS.iter().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    }
}

/// Where a share's record lives at a pkarr relay: the relay URL plus the lookup key.
pub fn relay_endpoint(relay: &str, keys: &lanrurugi_share_proto::derive::DerivedKeys) -> String {
    format!(
        "{}/{}",
        relay.trim_end_matches('/'),
        keys.lookup_key.public_key().to_z32()
    )
}

/// The record's relay list: the relays this endpoint can currently be reached through.
pub fn advertised_relays(net: &ShareNet, settings: &ShareSettings) -> Vec<String> {
    let mut relays: Vec<String> = net
        .endpoint()
        .addr()
        .relay_urls()
        .map(|u| u.to_string())
        .collect();
    let own = settings.text("share_self_relay_url");
    if let Some(own) = crate::settings::canonical_https_url(own) {
        relays.push(own);
    }
    if relays.is_empty() {
        relays = crate::net::relay_urls(settings)
            .iter()
            .map(|u| u.to_string())
            .collect();
    }
    let mut seen = std::collections::HashSet::new();
    relays.retain(|r| seen.insert(r.clone()));
    // Publicly reachable relays first: a peer that is not on this LAN cannot use
    // `https://192.168.x.y/`, and iroh only gives up on an unreachable relay after a timeout. The
    // private ones stay in the list for the same-NAT case, where they are the cheap path.
    relays.sort_by_key(|r| !relay_is_public(r));
    relays
}

/// Whether a relay URL names a host the public internet can reach. Only the obvious private
/// shapes are detected — a public hostname that resolves to a private address is not.
pub fn relay_is_public(relay: &str) -> bool {
    let Ok(url) = url::Url::parse(relay) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    if host.eq_ignore_ascii_case("localhost") || host.ends_with(".local") {
        return false;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => {
            !(v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified())
        }
        Ok(std::net::IpAddr::V6(v6)) => {
            !(v6.is_loopback() || v6.is_unspecified() || v6.is_unique_local())
        }
        // A name: assume it is routable, which is what a relay hostname normally is.
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_addresses_skip_unreachable_ones_and_put_public_first() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        for unreachable in ["127.0.0.1", "0.0.0.0", "169.254.1.2", "::1", "fe80::1"] {
            assert!(!dialable(ip(unreachable)), "{unreachable}");
        }
        let mut found = [
            "10.88.0.1",
            "192.168.3.105",
            "2001:db8::5",
            "203.0.113.7",
            "fd00::1",
        ]
        .map(ip)
        .to_vec();
        found.sort_by_key(|a| address_rank(*a));
        assert_eq!(
            found[..3],
            [ip("2001:db8::5"), ip("203.0.113.7"), ip("192.168.3.105")]
        );
        assert_eq!(address_rank(ip("100.72.1.1")), 2);
    }

    #[test]
    fn a_pinned_endpoint_publishes_only_its_interface_addresses() {
        let tunnel: IpAddr = "10.8.0.2".parse().unwrap();
        let public_v6: IpAddr = "2001:db8::1".parse::<std::net::Ipv6Addr>().unwrap().into();
        let bound = vec![tunnel];
        assert!(address_allowed(&bound, tunnel));
        assert!(!address_allowed(&bound, public_v6));
    }

    #[test]
    fn an_unpinned_endpoint_publishes_whatever_it_listens_on() {
        let bound: Vec<IpAddr> = Vec::new();
        assert!(address_allowed(&bound, "192.168.3.20".parse().unwrap()));
        assert!(address_allowed(&bound, "2001:db8::1".parse().unwrap()));
    }
}
