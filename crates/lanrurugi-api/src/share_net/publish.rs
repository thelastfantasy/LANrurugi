//! Publishing and resolving knock records (research §2, FR-047).
//!
//! The only thing outside services hold: an encrypted `{endpoint id, relays}` record, signed by a
//! key derived from the share code. It goes to pkarr's HTTPS relays over plain HTTPS; the instance
//! never speaks the DHT itself (FR-045).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::Duration;

use bytes::Bytes;
use iroh::endpoint::Connection;
use iroh::{EndpointAddr, EndpointId};
use lanrurugi_core::ids::ShareId;
use lanrurugi_share_proto::code::{ShareCode, ShareRef, Via};
use lanrurugi_share_proto::derive::DerivedKeys;
use lanrurugi_share_proto::record::{self, KnockV1};
use lanrurugi_storage::shares::{Share, ShareStatus};

use super::{endpoint_addr, parse_endpoint_hex, ShareNet, ShareNetError, DEFAULT_PKARR_RELAYS};
use crate::share_settings::ShareSettings;
use crate::AppState;

/// Republish interval; records live an hour at the relays.
pub const REPUBLISH_EVERY: Duration = Duration::from_secs(60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the routes a code carries run alone before the rendezvous are asked as well.
const LOOKUP_HEAD_START: Duration = Duration::from_secs(3);
/// Enough for a public IPv6, a mapped IPv4 and a LAN address or two, without a dense QR code.
const MAX_HINT_ADDRS: usize = 6;

/// Code-derived keys are expensive (Argon2id), so each share's are derived once per process.
static DERIVED: Mutex<Option<HashMap<ShareId, DerivedKeys>>> = Mutex::new(None);

async fn derived_keys(state: &AppState, share: &Share) -> Option<DerivedKeys> {
    if let Some(keys) = DERIVED
        .lock()
        .expect("derived lock")
        .as_ref()
        .and_then(|m| m.get(&share.id).cloned())
    {
        return Some(keys);
    }
    let code = crate::shares::share_code(state, share).await?;
    let keys = tokio::task::spawn_blocking(move || DerivedKeys::derive(&code))
        .await
        .ok()?;
    DERIVED
        .lock()
        .expect("derived lock")
        .get_or_insert_with(HashMap::new)
        .insert(share.id.clone(), keys.clone());
    Some(keys)
}

/// An importer's code-derived keys, kept so resumes and helper heartbeats derive them once.
static CODE_KEYS: Mutex<Option<HashMap<String, DerivedKeys>>> = Mutex::new(None);
const CODE_KEYS_KEPT: usize = 64;

async fn keys_for_code(code: &ShareCode) -> Result<DerivedKeys, ShareNetError> {
    let cached = CODE_KEYS
        .lock()
        .expect("code keys lock")
        .as_ref()
        .and_then(|m| m.get(code.as_str()).cloned());
    if let Some(keys) = cached {
        return Ok(keys);
    }
    let owned = code.clone();
    let keys = tokio::task::spawn_blocking(move || DerivedKeys::derive(&owned))
        .await
        .map_err(|e| ShareNetError::Protocol(e.to_string()))?;
    let mut guard = CODE_KEYS.lock().expect("code keys lock");
    let map = guard.get_or_insert_with(HashMap::new);
    if map.len() >= CODE_KEYS_KEPT {
        map.clear();
    }
    map.insert(code.as_str().to_string(), keys.clone());
    Ok(keys)
}

/// The pkarr relays in use: configured ones, else pkarr's defaults while public services are on.
pub fn pkarr_relays(settings: &ShareSettings) -> Vec<String> {
    let configured: Vec<String> = settings
        .list("share_pkarr_relays")
        .iter()
        .filter_map(|r| crate::share_settings::canonical_https_url(r))
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

fn http_client() -> reqwest::Client {
    super::proxy::http_client(REQUEST_TIMEOUT)
}

/// Sends one request to a configured LANrurugi node.
async fn ask_node(
    net: &ShareNet,
    node: &super::nodes::NodeRef,
    request: lanrurugi_share_proto::messages::Request,
) -> Option<lanrurugi_share_proto::messages::Response> {
    let addr = super::nodes::node_addr(node).await?;
    let conn = super::connect(net, addr).await.ok()?;
    super::call(&conn, request).await.ok().map(|(r, _)| r)
}

fn node_label(node: &super::nodes::NodeRef) -> String {
    format!("lrrg-node:{}", node.addr)
}

fn relay_endpoint(relay: &str, keys: &DerivedKeys) -> String {
    format!(
        "{}/{}",
        relay.trim_end_matches('/'),
        keys.lookup_key.public_key().to_z32()
    )
}

/// The record's relay list: the relays this endpoint can currently be reached through.
fn advertised_relays(net: &ShareNet, settings: &ShareSettings) -> Vec<String> {
    let mut relays: Vec<String> = net
        .endpoint()
        .addr()
        .relay_urls()
        .map(|u| u.to_string())
        .collect();
    let own = settings.text("share_self_relay_url");
    if let Some(own) = crate::share_settings::canonical_https_url(own) {
        relays.push(own);
    }
    if relays.is_empty() {
        relays = super::relay_urls(settings)
            .iter()
            .map(|u| u.to_string())
            .collect();
    }
    let mut seen = std::collections::HashSet::new();
    relays.retain(|r| seen.insert(r.clone()));
    relays
}

/// Publishes one share's knock record to every configured rendezvous.
pub async fn publish_share(state: &AppState, net: &ShareNet, share: &Share) {
    let settings = crate::share_settings::load(state).await;
    let relays = pkarr_relays(&settings);
    let nodes = super::nodes::configured_nodes(&settings);
    if relays.is_empty() && nodes.is_empty() {
        return;
    }
    let Some(keys) = derived_keys(state, share).await else {
        return;
    };
    let knock = KnockV1 {
        endpoint_id: *net.id().as_bytes(),
        relay_urls: advertised_relays(net, &settings),
    };
    let packet = match record::seal(&keys, &knock) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "knock record could not be built");
            return;
        }
    };
    let payload = packet.to_relay_payload();
    let full = packet.as_bytes().to_vec();
    let node_puts = nodes.iter().map(|node| {
        let full = full.clone();
        async move {
            let ok = matches!(
                ask_node(
                    net,
                    node,
                    lanrurugi_share_proto::messages::Request::RendezvousPut {
                        signed_packet: full
                    }
                )
                .await,
                Some(lanrurugi_share_proto::messages::Response::Ok)
            );
            super::status::record_rendezvous(&node_label(node), ok);
        }
    });
    futures_util::future::join_all(node_puts).await;
    let client = http_client();
    let puts = relays.iter().map(|relay| {
        let url = relay_endpoint(relay, &keys);
        let body = payload.clone();
        let client = client.clone();
        async move {
            let ok = client
                .put(&url)
                .header("content-type", "application/pkarr.org/relays#payload")
                .body(body)
                .send()
                .await
                .is_ok_and(|r| r.status().is_success());
            super::status::record_rendezvous(relay, ok);
        }
    });
    futures_util::future::join_all(puts).await;
}

/// Publishes every active share. Called on start, hourly, and when the endpoint's relays change.
pub async fn publish_all(state: &AppState, net: &ShareNet) {
    let now = crate::shares::now_secs();
    let shares = crate::shares::repo(state).list().await.unwrap_or_default();
    for share in shares {
        if matches!(
            share.status(now),
            ShareStatus::Active | ShareStatus::Exhausted
        ) && share.permissions.import
        {
            publish_share(state, net, &share).await;
        }
    }
}

/// Runs the hourly republisher for as long as the process lives.
pub fn spawn_republisher(state: AppState) {
    tokio::spawn(async move {
        loop {
            if let Some(net) = super::handle().await {
                publish_all(&state, &net).await;
            }
            tokio::time::sleep(REPUBLISH_EVERY).await;
        }
    });
}

/// The address a `via` hint names: its relays and every address that resolves.
async fn hinted_addr(via: &Via) -> Option<EndpointAddr> {
    let id = parse_endpoint_hex(&via.endpoint_id)?;
    let mut addr = endpoint_addr(&id, &via.relays, None)?;
    for text in &via.addrs {
        if let Some(direct) = tokio::net::lookup_host(text.as_str())
            .await
            .ok()
            .and_then(|mut found| found.next())
        {
            addr = addr.with_ip_addr(direct);
        }
    }
    Some(addr)
}

/// Dials a code's sharer two ways at once: the routes the code carries, which need no outside
/// service, and whatever the rendezvous know, for when the sharer's addresses have changed. The
/// lookup starts after a short head start, so a code whose own route works never pays for it.
pub async fn dial(
    state: &AppState,
    net: &ShareNet,
    reference: &ShareRef,
) -> Result<(EndpointId, Connection), ShareNetError> {
    let hinted_addr = match &reference.via {
        Some(via) => hinted_addr(via).await,
        None => None,
    };
    let head_start = if hinted_addr.is_some() {
        LOOKUP_HEAD_START
    } else {
        Duration::ZERO
    };
    let hinted = async {
        let Some(addr) = hinted_addr else {
            return Err(None);
        };
        let id = addr.id;
        let dialled: Result<_, Option<ShareNetError>> = super::connect(net, addr)
            .await
            .map(|conn| (id, conn))
            .map_err(Some);
        dialled
    };
    let looked_up = async {
        tokio::time::sleep(head_start).await;
        let addr = resolve(state, reference).await?;
        let id = addr.id;
        super::connect(net, addr).await.map(|conn| (id, conn))
    };
    tokio::pin!(hinted, looked_up);
    let (mut hinted_failed, mut lookup_failed) = (None, None);
    while hinted_failed.is_none() || lookup_failed.is_none() {
        tokio::select! {
            result = &mut hinted, if hinted_failed.is_none() => match result {
                Ok(dialled) => return Ok(dialled),
                Err(e) => hinted_failed = Some(e),
            },
            result = &mut looked_up, if lookup_failed.is_none() => match result {
                Ok(dialled) => return Ok(dialled),
                Err(e) => lookup_failed = Some(e),
            },
        }
    }
    let lookup_error = lookup_failed.expect("both routes have settled");
    // The code's own route failing says more than a rendezvous that never heard of it.
    Err(hinted_failed.flatten().unwrap_or(lookup_error))
}

/// The nodes a code names, with the code's relay on the node's own host as the node's relay.
fn nodes_in_code(reference: &ShareRef) -> Vec<super::nodes::NodeRef> {
    let relays = reference
        .via
        .as_ref()
        .map(|v| v.relays.as_slice())
        .unwrap_or_default();
    reference
        .nodes
        .iter()
        .filter_map(|text| super::nodes::parse_node(&format!("lrrg-node:{text}")))
        .map(|mut node| {
            let host = node
                .addr
                .rsplit_once(':')
                .map_or(node.addr.as_str(), |(h, _)| h)
                .trim_matches(['[', ']'])
                .to_string();
            node.relay = relays
                .iter()
                .find(|r| relay_host(r).as_deref() == Some(host.as_str()))
                .cloned();
            node
        })
        .collect()
}

fn relay_host(relay: &str) -> Option<String> {
    let url = url::Url::parse(relay).ok()?;
    Some(
        url.host_str()?
            .trim_end_matches('.')
            .trim_matches(['[', ']'])
            .to_string(),
    )
}

/// Finds where a code's sharer can be dialled through the rendezvous (FR-010, FR-029).
pub async fn resolve(
    state: &AppState,
    reference: &ShareRef,
) -> Result<EndpointAddr, ShareNetError> {
    let settings = crate::share_settings::load(state).await;
    let relays = pkarr_relays(&settings);
    let mut nodes = super::nodes::configured_nodes(&settings);
    for node in nodes_in_code(reference) {
        if !nodes.iter().any(|n| n.endpoint_id == node.endpoint_id) {
            nodes.push(node);
        }
    }
    let keys = keys_for_code(&reference.code).await?;
    let net = super::handle().await.ok_or(ShareNetError::NotRunning)?;
    let lookup_pubkey = keys.lookup_public();
    let node_gets = nodes.iter().map(|node| {
        let net = net.clone();
        async move {
            let answer = ask_node(
                &net,
                node,
                lanrurugi_share_proto::messages::Request::RendezvousGet { lookup_pubkey },
            )
            .await;
            super::status::record_rendezvous(&node_label(node), answer.is_some());
            match answer {
                Some(lanrurugi_share_proto::messages::Response::Packet { signed_packet })
                    if signed_packet.len() > 32 =>
                {
                    Some(Bytes::copy_from_slice(&signed_packet[32..]))
                }
                _ => None,
            }
        }
    });
    let from_nodes = futures_util::future::join_all(node_gets).await;
    let client = http_client();
    let gets = relays.iter().map(|relay| {
        let url = relay_endpoint(relay, &keys);
        let client = client.clone();
        let relay = relay.clone();
        async move {
            let response = client.get(&url).send().await;
            let ok = response
                .as_ref()
                .is_ok_and(|r| r.status().is_success() || r.status() == 404);
            super::status::record_rendezvous(&relay, ok);
            let body: Bytes = response.ok()?.error_for_status().ok()?.bytes().await.ok()?;
            Some(body)
        }
    });
    let answers = futures_util::future::join_all(gets).await;
    for payload in from_nodes.into_iter().chain(answers).flatten() {
        let Ok(packet) =
            pkarr::SignedPacket::from_relay_payload(&keys.lookup_key.public_key(), &payload)
        else {
            continue;
        };
        if let Ok(knock) = record::open(&keys, &packet) {
            return endpoint_addr(&knock.endpoint_id, &knock.relay_urls, None)
                .ok_or_else(|| ShareNetError::Protocol("malformed knock record".into()));
        }
    }
    // No rendezvous knew the code: ask the instances on this network directly (FR-024).
    if let Some(addr) = resolve_local(&net, keys.lookup_hash()).await {
        return Ok(addr);
    }
    if relays.is_empty() && nodes.is_empty() {
        return Err(ShareNetError::Unreachable(
            "no rendezvous is configured and no local peer has the share".into(),
        ));
    }
    Err(ShareNetError::Refused(
        lanrurugi_share_proto::messages::RefusalReason::NotFound,
    ))
}

/// Asks every endpoint mDNS has found whether it holds the share.
async fn resolve_local(
    net: &std::sync::Arc<ShareNet>,
    lookup_hash: [u8; 32],
) -> Option<EndpointAddr> {
    let peers: Vec<iroh::EndpointId> = net
        .lan_peers
        .lock()
        .expect("lan peers lock")
        .iter()
        .copied()
        .collect();
    let asks = peers.into_iter().map(|peer| {
        let net = net.clone();
        async move {
            let conn = super::connect(&net, EndpointAddr::new(peer)).await.ok()?;
            match super::call(
                &conn,
                lanrurugi_share_proto::messages::Request::ResolveLocal { lookup_hash },
            )
            .await
            {
                Ok((lanrurugi_share_proto::messages::Response::Found { .. }, _)) => {
                    Some(EndpointAddr::new(peer))
                }
                _ => None,
            }
        }
    });
    futures_util::future::join_all(asks)
        .await
        .into_iter()
        .flatten()
        .next()
}

/// The `via` hint every code carries: this endpoint, the addresses a peer may dial (configured
/// external ones, then public, then private) and the relays it is connected to.
pub fn via_hint(net: &ShareNet, settings: &ShareSettings) -> Option<Via> {
    let endpoint = net.endpoint().addr();
    let mut found: Vec<SocketAddr> = endpoint
        .ip_addrs()
        .copied()
        .filter(|a| dialable(a.ip()))
        .collect();
    found.sort_by_key(|a| address_rank(a.ip()));
    let mut addrs: Vec<String> = Vec::new();
    for addr in settings.external_addrs().into_iter().chain(found) {
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
        endpoint_id: super::endpoint_hex(&net.id()),
        addrs,
        relays,
    })
}

/// Whether another machine could ever reach `ip`: not loopback, unspecified or link-local.
fn dialable(ip: IpAddr) -> bool {
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
fn address_rank(ip: IpAddr) -> u8 {
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
    fn a_code_node_takes_the_code_relay_on_its_own_host() {
        let id = "ab".repeat(32);
        let mut reference = ShareRef::parse(&format!(
            "lrrg:K7QM9XRT2H?via=cd@&relay=https://other.example/,https://node.example.org/\
             &node={id}@node.example.org:443"
        ))
        .unwrap();
        let nodes = nodes_in_code(&reference);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].relay.as_deref(), Some("https://node.example.org/"));
        reference.via = None;
        assert_eq!(nodes_in_code(&reference)[0].relay, None);
    }
}
