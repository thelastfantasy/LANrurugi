//! Following the user's own proxy (FR-051, research §16).
//!
//! iroh only speaks to an HTTP(S) proxy, and only when told one. Sharing therefore picks the proxy
//! the operating system already has — the proxy environment variables, else on macOS the system
//! proxy — and bridges a SOCKS5 proxy through a local HTTP CONNECT listener. There is no setting
//! of its own; a transparent (TUN) proxy needs none. Direct UDP between instances is never
//! proxied; LAN and mDNS traffic never reach a proxy.

use std::net::SocketAddr;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

/// The proxy to use, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    None,
    /// An `http://` or `https://` proxy, used as is.
    Http(Url),
    /// A `socks5://` or `socks5h://` proxy, reached through the local bridge.
    Socks(Url),
}

fn classify(url: Url) -> Choice {
    match url.scheme() {
        "http" | "https" => Choice::Http(url),
        "socks5" | "socks5h" => Choice::Socks(url),
        _ => Choice::None,
    }
}

fn env(names: &[&str]) -> Option<Url> {
    names
        .iter()
        .find_map(|n| std::env::var(n).ok().filter(|v| !v.trim().is_empty()))
        .and_then(|v| Url::parse(v.trim()).ok())
}

/// `HTTPS_PROXY`/`HTTP_PROXY`, then `ALL_PROXY`, then the macOS system proxy.
pub fn choose() -> Choice {
    if let Some(url) = env(&["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]) {
        return classify(url);
    }
    if let Some(url) = env(&["ALL_PROXY", "all_proxy"]) {
        return classify(url);
    }
    system::proxy().map(classify).unwrap_or(Choice::None)
}

/// Hosts that must bypass the proxy (`NO_PROXY`), for the pkarr HTTP client.
pub fn no_proxy() -> Option<reqwest::NoProxy> {
    reqwest::NoProxy::from_env()
}

/// A reqwest client for rendezvous relays that follows the same proxy.
pub fn http_client(timeout: std::time::Duration) -> reqwest::Client {
    let mut builder = reqwest::Client::builder().timeout(timeout);
    match choose() {
        Choice::Http(url) | Choice::Socks(url) => {
            if let Ok(proxy) = reqwest::Proxy::all(url.as_str()) {
                builder = builder.proxy(proxy.no_proxy(no_proxy()));
            }
        }
        // Without a chosen proxy, reqwest still honours the environment by default.
        Choice::None => {}
    }
    lanrurugi_core::http_client::build(builder)
        .unwrap_or_else(|_| lanrurugi_core::http_client::default_client())
}

/// Starts a local HTTP CONNECT proxy that forwards every tunnel through `socks`, and returns its
/// address for iroh's `proxy_url`. Bound to loopback only; it lives as long as the process.
pub async fn start_socks_bridge(socks: Url) -> std::io::Result<SocketAddr> {
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0))).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else {
                continue;
            };
            let socks = socks.clone();
            tokio::spawn(async move {
                if let Err(e) = bridge_one(client, &socks).await {
                    tracing::debug!(error = %e, "SOCKS bridge connection failed");
                }
            });
        }
    });
    Ok(addr)
}

async fn bridge_one(client: TcpStream, socks: &Url) -> std::io::Result<()> {
    let mut reader = BufReader::new(client);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await?;
    // Skip the remaining request headers.
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
    }
    let mut client = reader.into_inner();
    let target = parse_connect(&request_line).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "not a CONNECT request")
    })?;
    let proxy_addr = format!(
        "{}:{}",
        socks.host_str().unwrap_or("127.0.0.1"),
        socks.port().unwrap_or(1080)
    );
    let upstream = if socks.username().is_empty() {
        tokio_socks::tcp::Socks5Stream::connect(proxy_addr.as_str(), target.as_str()).await
    } else {
        tokio_socks::tcp::Socks5Stream::connect_with_password(
            proxy_addr.as_str(),
            target.as_str(),
            socks.username(),
            socks.password().unwrap_or(""),
        )
        .await
    };
    let mut upstream = match upstream {
        Ok(s) => s.into_inner(),
        Err(e) => {
            client
                .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                .await?;
            return Err(std::io::Error::other(e));
        }
    };
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;
    tokio::io::copy_bidirectional(&mut client, &mut upstream).await?;
    Ok(())
}

/// `CONNECT host:port HTTP/1.1` → `host:port`.
fn parse_connect(line: &str) -> Option<String> {
    let mut parts = line.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("CONNECT") {
        return None;
    }
    let target = parts.next()?;
    target.rsplit_once(':')?.1.parse::<u16>().ok()?;
    Some(target.to_string())
}

#[cfg(target_os = "macos")]
mod system {
    use system_configuration::core_foundation::base::CFType;
    use system_configuration::core_foundation::dictionary::CFDictionary;
    use system_configuration::core_foundation::number::CFNumber;
    use system_configuration::core_foundation::string::{CFString, CFStringRef};
    use system_configuration::dynamic_store::SCDynamicStoreBuilder;
    use system_configuration::sys::schema_definitions::{
        kSCPropNetProxiesHTTPEnable, kSCPropNetProxiesHTTPPort, kSCPropNetProxiesHTTPProxy,
        kSCPropNetProxiesHTTPSEnable, kSCPropNetProxiesHTTPSPort, kSCPropNetProxiesHTTPSProxy,
        kSCPropNetProxiesSOCKSEnable, kSCPropNetProxiesSOCKSPort, kSCPropNetProxiesSOCKSProxy,
    };

    /// The system proxy from System Settings → Network, which programs do not see as variables.
    pub fn proxy() -> Option<url::Url> {
        let store = SCDynamicStoreBuilder::new("lanrurugi").build()?;
        let map = store.get_proxies()?;
        let pick = |scheme: &str, enabled, host, port| {
            entry(&map, enabled, host, port)
                .and_then(|hp| url::Url::parse(&format!("{scheme}://{hp}")).ok())
        };
        // SAFETY: these are constant CFString keys exported by the SystemConfiguration framework.
        unsafe {
            pick(
                "http",
                kSCPropNetProxiesHTTPSEnable,
                kSCPropNetProxiesHTTPSProxy,
                kSCPropNetProxiesHTTPSPort,
            )
            .or_else(|| {
                pick(
                    "http",
                    kSCPropNetProxiesHTTPEnable,
                    kSCPropNetProxiesHTTPProxy,
                    kSCPropNetProxiesHTTPPort,
                )
            })
            .or_else(|| {
                pick(
                    "socks5h",
                    kSCPropNetProxiesSOCKSEnable,
                    kSCPropNetProxiesSOCKSProxy,
                    kSCPropNetProxiesSOCKSPort,
                )
            })
        }
    }

    fn entry(
        map: &CFDictionary<CFString, CFType>,
        enabled: CFStringRef,
        host: CFStringRef,
        port: CFStringRef,
    ) -> Option<String> {
        let on = map
            .find(enabled)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i32())
            .unwrap_or(0)
            == 1;
        if !on {
            return None;
        }
        let host = map
            .find(host)
            .and_then(|v| v.downcast::<CFString>())
            .map(|v| v.to_string())?;
        let port = map
            .find(port)
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|v| v.to_i32());
        Some(match port {
            Some(p) => format!("{host}:{p}"),
            None => host,
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod system {
    /// Linux and FreeBSD desktops and session managers export proxies as environment variables.
    pub fn proxy() -> Option<url::Url> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn connect_lines_parse() {
        assert_eq!(
            parse_connect("CONNECT relay.example.org:443 HTTP/1.1\r\n").as_deref(),
            Some("relay.example.org:443")
        );
        assert_eq!(parse_connect("GET / HTTP/1.1").as_deref(), None);
        assert_eq!(parse_connect("CONNECT nohost HTTP/1.1").as_deref(), None);
    }

    #[test]
    fn schemes_pick_the_right_path() {
        assert!(matches!(
            classify(Url::parse("socks5h://127.0.0.1:7890").unwrap()),
            Choice::Socks(_)
        ));
        assert!(matches!(
            classify(Url::parse("http://127.0.0.1:7890").unwrap()),
            Choice::Http(_)
        ));
        assert_eq!(classify(Url::parse("ftp://x").unwrap()), Choice::None);
    }

    /// A minimal SOCKS5 server (no auth) that connects to the requested address.
    async fn socks5_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 262];
                    s.read_exact(&mut buf[..2]).await.unwrap();
                    let n = buf[1] as usize;
                    s.read_exact(&mut buf[..n]).await.unwrap();
                    s.write_all(&[5, 0]).await.unwrap();
                    s.read_exact(&mut buf[..4]).await.unwrap();
                    let target = match buf[3] {
                        1 => {
                            let mut a = [0u8; 6];
                            s.read_exact(&mut a).await.unwrap();
                            format!(
                                "{}.{}.{}.{}:{}",
                                a[0],
                                a[1],
                                a[2],
                                a[3],
                                u16::from_be_bytes([a[4], a[5]])
                            )
                        }
                        3 => {
                            let mut l = [0u8; 1];
                            s.read_exact(&mut l).await.unwrap();
                            let mut host = vec![0u8; l[0] as usize + 2];
                            s.read_exact(&mut host).await.unwrap();
                            let port =
                                u16::from_be_bytes([host[host.len() - 2], host[host.len() - 1]]);
                            format!(
                                "{}:{port}",
                                String::from_utf8_lossy(&host[..host.len() - 2])
                            )
                        }
                        _ => return,
                    };
                    let mut up = TcpStream::connect(target).await.unwrap();
                    s.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).await.unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut s, &mut up).await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn the_bridge_tunnels_connect_through_socks5() {
        let echo = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let echo_addr = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = echo.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });
        let socks = socks5_server().await;
        let bridge = start_socks_bridge(Url::parse(&format!("socks5://{socks}")).unwrap())
            .await
            .unwrap();

        let mut client = TcpStream::connect(bridge).await.unwrap();
        client
            .write_all(
                format!("CONNECT {echo_addr} HTTP/1.1\r\nHost: {echo_addr}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut reader = BufReader::new(&mut client);
        let mut status = String::new();
        reader.read_line(&mut status).await.unwrap();
        assert!(status.contains("200"), "{status}");
        let mut blank = String::new();
        reader.read_line(&mut blank).await.unwrap();
        client.write_all(b"hello").await.unwrap();
        let mut back = [0u8; 5];
        client.read_exact(&mut back).await.unwrap();
        assert_eq!(&back, b"hello");
    }
}
