//! The host's network interfaces, so the share endpoint can be pinned to one.
//!
//! Pinning is what makes "share only through the VPN tunnel" possible: an endpoint bound to
//! `tun0`'s address never sends a packet out of the physical link, even while the rest of the
//! process (and the library itself) still serves the LAN over it.

use std::net::{IpAddr, SocketAddr};

use serde::Serialize;

/// One interface and the addresses it currently holds.
#[derive(Debug, Clone, Serialize)]
pub struct NetInterface {
    pub name: String,
    pub addrs: Vec<String>,
}

/// Every interface except loopback, with its bindable addresses.
pub fn list() -> Vec<NetInterface> {
    let mut out: Vec<NetInterface> = Vec::new();
    let ifaces = match if_addrs::get_if_addrs() {
        Ok(ifaces) => ifaces,
        Err(e) => {
            tracing::warn!(error = %e, "network interfaces could not be listed");
            return out;
        }
    };
    for iface in ifaces {
        let addr = iface.ip();
        if addr.is_loopback() || is_unbindable(addr) {
            continue;
        }
        match out.iter_mut().find(|entry| entry.name == iface.name) {
            Some(entry) => entry.addrs.push(addr.to_string()),
            None => out.push(NetInterface {
                name: iface.name,
                addrs: vec![addr.to_string()],
            }),
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A link-local IPv6 address needs a scope id to bind, which `if-addrs` does not report.
fn is_unbindable(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V6(v6) => v6.is_unicast_link_local(),
        IpAddr::V4(_) => false,
    }
}

/// The addresses `name` should be bound to, or `None` when it is empty (bind everything) or gone.
pub fn bind_addrs(name: &str, port: u16) -> Option<Vec<SocketAddr>> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    let found = list().into_iter().find(|iface| iface.name == name)?;
    let addrs: Vec<SocketAddr> = found
        .addrs
        .iter()
        .filter_map(|addr| addr.parse::<IpAddr>().ok())
        .map(|addr| SocketAddr::new(addr, port))
        .collect();
    (!addrs.is_empty()).then_some(addrs)
}

/// Whether `name` is an interface this host has right now.
pub fn exists(name: &str) -> bool {
    list().iter().any(|iface| iface.name == name.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_skips_loopback_and_reports_bindable_addresses() {
        for iface in list() {
            assert!(!iface.name.is_empty());
            for addr in &iface.addrs {
                let ip: IpAddr = addr.parse().unwrap();
                assert!(!ip.is_loopback());
                assert!(!is_unbindable(ip));
            }
        }
    }

    #[test]
    fn an_empty_name_binds_everything_and_an_unknown_one_binds_nothing() {
        assert!(bind_addrs("", 11204).is_none());
        assert!(bind_addrs("  ", 11204).is_none());
        assert!(bind_addrs("definitely-not-an-interface", 11204).is_none());
        assert!(!exists("definitely-not-an-interface"));
    }

    #[test]
    fn a_known_interface_binds_its_own_addresses_on_the_given_port() {
        let Some(first) = list().into_iter().find(|iface| !iface.addrs.is_empty()) else {
            return;
        };
        let addrs = bind_addrs(&first.name, 11204).expect("the interface is listed");
        assert_eq!(addrs.len(), first.addrs.len());
        assert!(addrs.iter().all(|addr| addr.port() == 11204));
    }
}
