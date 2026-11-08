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
    /// The kernel driver behind it (`iwlwifi`, `r8152`, …) when the platform reports one — the
    /// picker shows it so `wlan0` and `tun0` are told apart by what they actually are, not only by
    /// a name the user has to already know.
    pub driver: Option<String>,
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
            None => {
                let driver = driver_of(&iface.name);
                out.push(NetInterface {
                    name: iface.name,
                    addrs: vec![addr.to_string()],
                    driver,
                })
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// `/sys/class/net/<name>/device/uevent` carries `DRIVER=…` on Linux. Virtual interfaces
/// (`tun0`, `lo`, bridges) have no device directory at all, so `None` is the normal answer and not
/// an error.
fn driver_of(name: &str) -> Option<String> {
    let uevent = std::fs::read_to_string(format!("/sys/class/net/{name}/device/uevent")).ok()?;
    uevent
        .lines()
        .find_map(|line| line.strip_prefix("DRIVER="))
        .map(|driver| driver.trim().to_string())
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

/// The interface the kernel would use for internet-bound traffic, if it can be told.
///
/// The picker shows this next to the "system default" choice so the user can see what they are
/// actually getting before pinning something else — and so `wlan0` being the default route is
/// visible even though the list itself is in no particular order.
pub fn default_route() -> Option<String> {
    std::fs::read_to_string("/proc/net/route")
        .ok()
        .and_then(|text| default_route_v4(&text))
        .or_else(|| {
            std::fs::read_to_string("/proc/net/ipv6_route")
                .ok()
                .and_then(|text| default_route_v6(&text))
        })
}

/// `Iface Destination Gateway Flags …` — the default route is destination and mask both zero;
/// among several, the lowest metric wins.
fn default_route_v4(proc_route: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for line in proc_route.lines().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 8 {
            continue;
        }
        let (iface, dest, mask) = (fields[0], fields[1], fields[7]);
        if dest != "00000000" || mask != "00000000" {
            continue;
        }
        let metric = fields[6].parse::<u32>().unwrap_or(u32::MAX);
        if best.as_ref().is_none_or(|(m, _)| metric < *m) {
            best = Some((metric, iface.to_string()));
        }
    }
    best.map(|(_, iface)| iface)
}

/// `dest prefixlen src prefixlen nexthop metric … flags iface` — a default route has an all-zero
/// destination with a zero prefix length.
fn default_route_v6(proc_route: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    for line in proc_route.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        let (dest, dest_len, iface) = (fields[0], fields[1], fields[9]);
        let all_zero = dest.chars().all(|c| c == '0');
        if !all_zero || dest_len != "00" || iface.is_empty() {
            continue;
        }
        let metric = fields[5].parse::<u32>().unwrap_or(u32::MAX);
        if best.as_ref().is_none_or(|(m, _)| metric < *m) {
            best = Some((metric, iface.to_string()));
        }
    }
    best.map(|(_, iface)| iface)
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
    fn the_default_route_is_the_lowest_metric_default() {
        let v4 =
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n\
                  wlp3s0\t00000000\t0101A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0\n\
                  tun0\t00000000\t00000000\t0001\t0\t0\t50\t00000000\t0\t0\t0\n\
                  wlp3s0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\n";
        assert_eq!(default_route_v4(v4).as_deref(), Some("tun0"));
    }

    #[test]
    fn a_lan_route_is_not_mistaken_for_a_default_one() {
        let v4 = "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\n\
                  wlp3s0\t0001A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\n";
        assert_eq!(default_route_v4(v4), None);
    }

    #[test]
    fn the_ipv6_default_needs_a_zero_prefix_length() {
        let v6 = "00000000000000000000000000000000 00 00000000000000000000000000000000 00 \
                  00000000000000000000000000000000 00000400 00000001 00000000 00000003 tun0\n\
                  20010000000000000000000000000000 40 00000000000000000000000000000000 00 \
                  00000000000000000000000000000000 00000400 00000001 00000000 00000003 wlp3s0\n";
        assert_eq!(default_route_v6(v6).as_deref(), Some("tun0"));
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
