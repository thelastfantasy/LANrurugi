//! The route hint a share code carries (`lrrg:<code>?r=…`): where the sharer can be dialled with
//! no outside service. It is sealed with a key derived from the code, so the text form shows no
//! address. Anyone holding the whole text form also holds the code and can read it, exactly as
//! connecting would tell them; the point is that an address is never readable at a glance or by
//! anything that scans text for one.

use std::net::SocketAddr;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::code::ShareCode;
use crate::messages::EndpointKey;
use crate::record::{open_bytes, seal_bytes};

const HINT_SALT: &[u8] = b"lrrg-share-hint-v1";
const HINT_AAD: &[u8] = b"lrrg-hint-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteHint {
    pub endpoint_id: EndpointKey,
    /// Addresses to dial directly, best first.
    pub addrs: Vec<SocketAddr>,
    /// https relays the sharer is reachable through.
    pub relays: Vec<String>,
    /// The sharer's LANrurugi nodes: `(endpoint id, host:port)`, rendezvous to ask when the routes
    /// above are stale.
    pub nodes: Vec<(EndpointKey, String)>,
}

/// A fast key: the hint travels next to its code, so a slow derivation would only add latency.
fn hint_key(code: &ShareCode) -> [u8; 32] {
    let mut key = [0u8; 32];
    Hkdf::<Sha256>::new(Some(HINT_SALT), code.as_str().as_bytes())
        .expand(b"hint", &mut key)
        .expect("32 bytes is a valid HKDF-SHA256 output length");
    key
}

impl RouteHint {
    /// `base64url(nonce ‖ ciphertext)` of the hint, for the `r` parameter.
    pub fn seal(&self, code: &ShareCode) -> String {
        let plaintext = postcard::to_allocvec(self).expect("a hint always serialises");
        URL_SAFE_NO_PAD.encode(seal_bytes(&hint_key(code), &plaintext, HINT_AAD))
    }

    /// Inverse of `seal`; `None` for anything that does not decrypt with this code.
    pub fn open(code: &ShareCode, param: &str) -> Option<Self> {
        let sealed = URL_SAFE_NO_PAD.decode(param.trim()).ok()?;
        let plaintext = open_bytes(&hint_key(code), &sealed, HINT_AAD)?;
        postcard::from_bytes(&plaintext).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hint() -> RouteHint {
        RouteHint {
            endpoint_id: [7; 32],
            addrs: vec![
                "[2001:db8::5]:11204".parse().unwrap(),
                "203.0.113.7:11204".parse().unwrap(),
            ],
            relays: vec!["https://relay.example.org/".into()],
            nodes: vec![([9; 32], "node.example.org:443".into())],
        }
    }

    #[test]
    fn a_hint_opens_only_with_its_own_code() {
        let code = ShareCode::parse("K7QM9XRT2H").unwrap();
        let sealed = hint().seal(&code);
        assert!(!sealed.contains("203.0.113.7"));
        assert_eq!(RouteHint::open(&code, &sealed), Some(hint()));
        let other = ShareCode::parse("K7QM9XRT2J").unwrap();
        assert_eq!(RouteHint::open(&other, &sealed), None);
        assert_eq!(RouteHint::open(&code, "not base64!"), None);
    }
}
