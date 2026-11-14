//! The relay role: an embedded `iroh-relay` server, its TLS modes, limits and allow list.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use crate::nodes::{parse_endpoint_hex, parse_node};
use crate::settings::ShareSettings;

/// Starts the embedded relay for the relay role (FR-048, FR-055): ACME for the public URL's host,
/// a certificate pair from files, or plain HTTP on a local port behind a tunnel or reverse proxy
/// that terminates TLS.
pub async fn start_relay(
    settings: &ShareSettings,
    data_dir: &Path,
    own: iroh::EndpointId,
) -> Result<(iroh_relay::server::Server, u16), String> {
    use iroh_relay::server::{AcmeConfig, CertConfig, RelayConfig, ServerConfig, TlsConfig};
    // A fixed port by default, because the `proxy` mode expects a reverse proxy to point at it.
    // Empty still means "whatever is free". A fixed port already taken falls back to a free one
    // (and says so), which keeps the endpoint up; a proxy pointing at the old number would stop
    // working, and the warning is what says why.
    let configured = settings.text("share_node_relay_port").trim();
    let mut port = if configured.is_empty() {
        free_tcp_port()?
    } else {
        configured
            .parse::<u16>()
            .map_err(|_| format!("\"share_node_relay_port\" is not a port: {configured}"))?
    };
    if !configured.is_empty() && std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
        tracing::warn!(port, "the relay port is taken; picking a free one instead");
        port = free_tcp_port()?;
    }
    let mut relay = match settings.text("share_node_relay_tls") {
        "proxy" => RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], port))),
        mode => {
            // The captive-portal HTTP listener is irrelevant here; keep it local and ephemeral.
            let mut relay = RelayConfig::new(SocketAddr::from(([127, 0, 0, 1], 0)));
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let builder = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())?
                .with_no_client_auth();
            let cert = if mode == "files" {
                let certs = load_certs(Path::new(settings.text("share_node_relay_cert")))?;
                let key = load_key(Path::new(settings.text("share_node_relay_key")))?;
                CertConfig::Manual {
                    server_config: builder
                        .with_single_cert(certs, key)
                        .map_err(|e| e.to_string())?,
                }
            } else {
                let host = settings
                    .public_url()
                    .and_then(|u| url::Url::parse(&u).ok())
                    .and_then(|u| u.host_str().map(str::to_string))
                    .ok_or("the relay role's ACME mode needs a public address")?;
                CertConfig::LetsEncrypt {
                    acme_config: AcmeConfig::letsencrypt(true)
                        .domains(vec![host])
                        .cache_path(data_dir.join("acme")),
                    server_config_builder: builder,
                }
            };
            relay.tls = Some(TlsConfig::new(any_addr(port), cert));
            relay
        }
    };
    relay.limits = relay_limits(settings);
    let allowed = relay_allow_list(settings);
    if !allowed.is_empty() {
        relay.access = Arc::new(AllowListed {
            own,
            allowed: allowed.into_iter().collect(),
        });
    }
    let mut config = ServerConfig::default();
    // QUIC address discovery on the standard port, so peers behind NAT learn their public
    // address from this relay and can hole-punch. It needs the relay's own TLS, so not in `proxy`.
    if relay.tls.is_some() {
        config.quic = Some(iroh_relay::server::QuicConfig::new(any_addr(
            iroh_relay::defaults::DEFAULT_RELAY_QUIC_PORT,
        )));
    }
    config.relay = Some(relay);
    let server = iroh_relay::server::Server::spawn(config)
        .await
        .map_err(|e| e.to_string())?;
    Ok((server, port))
}

fn free_tcp_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
    listener
        .local_addr()
        .map_err(|e| e.to_string())
        .map(|a| a.port())
}

/// Per-client rate limit from `share_node_relay_rate` (bytes per second, empty for none). Every
/// relayed byte enters from one client, so this caps what any one peer can push through.
fn relay_limits(settings: &ShareSettings) -> iroh_relay::server::Limits {
    let mut limits = iroh_relay::server::Limits::default();
    let rate = settings
        .number("share_node_relay_rate")
        .map(|n| n.min(u64::from(u32::MAX)) as u32)
        .and_then(std::num::NonZeroU32::new);
    if let Some(rate) = rate {
        let mut client = iroh_relay::server::ClientRateLimit::new(rate);
        // A second's worth of burst keeps short handshakes snappy.
        client.max_burst_bytes = Some(rate);
        limits.client_rx = Some(client);
    }
    limits
}

/// Endpoint ids from `share_node_relay_allow`: bare hex ids or node strings.
pub fn relay_allow_list(settings: &ShareSettings) -> Vec<iroh::EndpointId> {
    settings
        .list("share_node_relay_allow")
        .iter()
        .filter_map(|entry| parse_allow_entry(entry))
        .collect()
}

pub fn parse_allow_entry(entry: &str) -> Option<iroh::EndpointId> {
    let entry = entry.trim();
    let bytes = match parse_node(entry) {
        Some(node) => node.endpoint_id,
        None => parse_endpoint_hex(entry)?,
    };
    iroh::EndpointId::from_bytes(&bytes).ok()
}

/// Admits this instance and the listed endpoints only. Everyone else, including importers of
/// this instance's own shares, has to use another relay.
#[derive(Debug)]
struct AllowListed {
    own: iroh::EndpointId,
    allowed: std::collections::HashSet<iroh::EndpointId>,
}

impl AllowListed {
    /// The rule on its own, so a rotation can be tested without a live relay: after the node is
    /// destroyed its new endpoint id is the `own` of the rebuilt relay, and the previous one is
    /// just another stranger — it is not carried over into `allowed`.
    fn admits(&self, id: &iroh::EndpointId) -> bool {
        id == &self.own || self.allowed.contains(id)
    }
}

impl iroh_relay::server::AccessControl for AllowListed {
    async fn on_connect(
        &self,
        request: &iroh_relay::server::ClientRequest,
    ) -> iroh_relay::server::Access {
        if self.admits(&request.endpoint_id()) {
            iroh_relay::server::Access::Allow
        } else {
            iroh_relay::server::Access::Deny { reason: None }
        }
    }
}

/// The wildcard address for a public listener: `[::]` where the host has IPv6 (on Linux it
/// accepts IPv4 too), else `0.0.0.0`.
fn any_addr(port: u16) -> SocketAddr {
    if std::net::UdpSocket::bind((std::net::Ipv6Addr::UNSPECIFIED, 0)).is_ok() {
        SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port))
    } else {
        SocketAddr::from(([0, 0, 0, 0], port))
    }
}

pub fn load_certs(path: &Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject;
    rustls::pki_types::CertificateDer::pem_file_iter(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn load_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    use rustls::pki_types::pem::PemObject;
    rustls::pki_types::PrivateKeyDer::from_pem_file(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rotated_away_endpoint_is_no_longer_admitted() {
        let key = |b: u8| {
            let mut seed = [0u8; 32];
            seed[0] = b;
            iroh::SecretKey::from_bytes(&seed).public()
        };
        let current = key(1);
        let listed = key(2);
        let retired = key(3);
        let relay = AllowListed {
            own: current,
            allowed: [listed].into_iter().collect(),
        };
        assert!(relay.admits(&current), "the node may use its own relay");
        assert!(
            relay.admits(&listed),
            "an allow-listed endpoint stays listed"
        );
        assert!(
            !relay.admits(&retired),
            "an endpoint id from before a rotation is a stranger"
        );
    }
}
