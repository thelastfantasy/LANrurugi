//! What a `via` code may advertise: only addresses this endpoint actually serves.

use std::net::{IpAddr, SocketAddr};

use lanrurugi_share_proto::code::Via;

use crate::net::ShareNet;
use crate::nodes::endpoint_hex;

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
