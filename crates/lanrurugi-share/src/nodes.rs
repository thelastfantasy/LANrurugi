//! Node strings and the relays they name. Pure parsing and settings lookups: the rendezvous and
//! relay roles themselves are still built by the application.

use iroh::EndpointAddr;

use crate::net::endpoint_addr;
use crate::proxy;
use crate::publish::{pkarr_relays, RETRACTED_ENDPOINT_ID};
use crate::settings::ShareSettings;

/// 32 bytes as lowercase hex, the spelling used in node strings and `via` codes.
pub fn endpoint_hex(id: &[u8; 32]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses the 64-character lowercase-hex form back into the endpoint id bytes.
pub fn parse_endpoint_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// A configured node: `lrrg-node:<endpoint_id>@<host:port>[?relay=<https url>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeRef {
    pub endpoint_id: [u8; 32],
    pub addr: String,
    pub relay: Option<String>,
    /// Set when the entry was a bare node code (`NodeCode`): the address is then looked up from the
    /// record published under the code instead of being spelled out in the string.
    pub code: Option<String>,
}

pub fn parse_node(entry: &str) -> Option<NodeRef> {
    let entry = entry.trim();
    // A bare node code is the short form of the same thing: the address lives in the record that
    // code resolves to, so there is nothing to parse out of the string itself.
    if let Ok(code) = lanrurugi_share_proto::node_code::NodeCode::parse_prefixed(entry) {
        return Some(NodeRef {
            endpoint_id: [0u8; 32],
            addr: String::new(),
            relay: None,
            code: Some(code.as_str().to_string()),
        });
    }
    let rest = entry.strip_prefix("lrrg-node:")?;
    let (head, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (id, addr) = head.split_once('@')?;
    let endpoint_id = parse_endpoint_hex(id)?;
    if !addr.contains(':') {
        return None;
    }
    let raw = query
        .split('&')
        .filter_map(|p| p.split_once('='))
        .find(|(k, _)| *k == "relay")
        .map(|(_, v)| v.to_string());
    let relay = match raw.as_deref() {
        // See `node_string`: the shorthand for "the relay on the address's own host".
        Some("auto") => Some(format!("https://{}/", addr.split(':').next()?)),
        Some(v) if url::Url::parse(v).is_ok_and(|u| u.scheme() == "https") => Some(v.to_string()),
        _ => None,
    };
    Some(NodeRef {
        endpoint_id,
        addr: addr.to_string(),
        relay,
        code: None,
    })
}

pub fn configured_nodes(settings: &ShareSettings) -> Vec<NodeRef> {
    settings
        .list("share_nodes")
        .iter()
        .filter_map(|e| parse_node(e))
        .collect()
}

pub fn own_relay_url(settings: &ShareSettings) -> Option<String> {
    // Only what an administrator typed, never a guess derived from the public address: that
    // address is the web page's host, and when it sits behind a CDN the guessed relay would send
    // the relay's own traffic through the same proxy — or name a private address nothing outside
    // this LAN can reach.
    let explicit = settings.text("share_self_relay_url");
    (!explicit.is_empty()).then(|| explicit.to_string())
}

/// The pkarr records a node code resolves to live beside the share records, same relay, same
/// `/{public key}` layout.
pub fn node_relay_url(relay: &str, code: &lanrurugi_share_proto::node_code::NodeCode) -> String {
    format!(
        "{}/{}",
        relay.trim_end_matches('/'),
        code.lookup_keypair().public_key().to_z32()
    )
}

/// Resolves a node code to the address published under it.
///
/// Returns `None` both when nothing is published and when the record is unreadable by this build
/// (the caller cannot tell the two apart, but the log can).
pub async fn resolve_node_code(settings: &ShareSettings, code: &str) -> Option<EndpointAddr> {
    use lanrurugi_share_proto::node_code::NodeCode;
    let code = NodeCode::parse(code).ok()?;
    let relays = pkarr_relays(settings);
    let keys = lanrurugi_share_proto::derive::DerivedKeys {
        lookup_key: code.lookup_keypair(),
        record_key: code.record_key(),
    };
    let client = proxy::http_client(std::time::Duration::from_secs(20));
    let mut incompatible = false;
    for relay in relays {
        let url = node_relay_url(&relay, &code);
        let Ok(response) = client.get(&url).send().await else {
            continue;
        };
        let Ok(bytes) = response.bytes().await else {
            continue;
        };
        let Ok(packet) =
            pkarr::SignedPacket::from_relay_payload(&keys.lookup_key.public_key(), &bytes)
        else {
            continue;
        };
        let knock = match lanrurugi_share_proto::record::open(&keys, &packet) {
            Ok(knock) => knock,
            Err(lanrurugi_share_proto::record::RecordError::EngineIncompatible) => {
                incompatible = true;
                continue;
            }
            Err(_) => continue,
        };
        // A retracted identity: the code was destroyed and this record is all that is left.
        if knock.endpoint_id == RETRACTED_ENDPOINT_ID {
            return None;
        }
        let mut addr = endpoint_addr(&knock.endpoint_id, &knock.relay_urls, None)?;
        for direct in &knock.direct_addrs {
            if let Ok(socket) = direct.parse::<std::net::SocketAddr>() {
                addr = addr.with_ip_addr(socket);
            }
        }
        return Some(addr);
    }
    if incompatible {
        // Distinguishing this from "never published" matters: one is fixed by upgrading, the
        // other by asking the sharer to publish the code again.
        tracing::warn!(code = %code, "the share code was published by a newer sharing engine");
    }
    None
}

/// The endpoint address of a configured node, its port resolved by DNS.
pub async fn node_addr(settings: &ShareSettings, node: &NodeRef) -> Option<EndpointAddr> {
    // A code-based node carries no address: the record under the code supplies the endpoint id and
    // the relays, and iroh resolves the actual address through those.
    if let Some(code) = &node.code {
        return resolve_node_code(settings, code).await;
    }
    let direct = tokio::net::lookup_host(&node.addr)
        .await
        .ok()
        .and_then(|mut a| a.next());
    let relays: Vec<String> = node.relay.iter().cloned().collect();
    endpoint_addr(&node.endpoint_id, &relays, direct)
}
