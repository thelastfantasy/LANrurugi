//! The HTTP listener. `[::]` binds one dual-stack socket, so `localhost` — which browsers try as
//! `::1` first — also works behind port forwarders that pass IPv6 through to an IPv4-only listener
//! and then reset it (rootless Podman's `-p`). A host without IPv6 falls back to `0.0.0.0`.

use std::net::{Ipv4Addr, SocketAddr, TcpListener};

use socket2::{Domain, Protocol, Socket, Type};

pub fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    match open(addr) {
        Err(e) if addr.is_ipv6() && addr.ip().is_unspecified() => {
            tracing::warn!(error = %e, "IPv6 unavailable; listening on IPv4 only");
            open(SocketAddr::from((Ipv4Addr::UNSPECIFIED, addr.port())))
        }
        result => result,
    }
}

fn open(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if addr.is_ipv6() {
        // Explicit, since the default differs by system (FreeBSD defaults to IPv6-only).
        socket.set_only_v6(false)?;
    }
    // Matches `tokio::net::TcpListener::bind`, so a restart does not wait out TIME_WAIT.
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv6Addr, TcpStream};

    #[test]
    fn the_unspecified_ipv6_address_accepts_both_families() {
        let listener = bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))).unwrap();
        let port = listener.local_addr().unwrap().port();
        TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        // Only where the host has IPv6 at all (it fell back to IPv4 otherwise).
        if listener.local_addr().unwrap().is_ipv6() {
            TcpStream::connect((Ipv6Addr::LOCALHOST, port)).unwrap();
        }
    }

    #[test]
    fn an_ipv4_address_binds_as_given() {
        let listener = bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
        assert!(listener.local_addr().unwrap().is_ipv4());
    }
}
