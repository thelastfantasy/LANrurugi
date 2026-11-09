//! The knock record: the only thing outside services ever hold (FR-047, research §2). It names
//! the sharer's endpoint and the relays it can be reached through, sealed so only a code holder
//! can read it, inside a pkarr packet signed by the code-derived lookup key.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use pkarr::dns::rdata::{RData, TXT};
use pkarr::dns::{CharacterString, Name};
use pkarr::SignedPacket;
use serde::{Deserialize, Serialize};

use crate::derive::DerivedKeys;
use crate::messages::EndpointKey;

/// A neutral record name: nothing in the packet names the application.
const RECORD_NAME: &str = "_k";
const RECORD_AAD: &[u8] = b"lrrg-knock-v1";
/// DNS character strings hold at most 255 bytes.
const CHUNK: usize = 255;
pub const RECORD_TTL_SECS: u32 = 3600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnockV1 {
    pub endpoint_id: EndpointKey,
    pub relay_urls: Vec<String>,
    /// Where the node can be dialled directly, as it knows itself right now: empty when it only
    /// has the relay. Always serialised — the record is postcard, which has no field names to fall
    /// back on, so a `skip_serializing_if` here would write a packet its own reader cannot parse.
    pub direct_addrs: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("record could not be built: {0}")]
    Build(String),
    #[error("packet holds no knock record")]
    Missing,
    #[error("knock record does not decrypt with this code")]
    Undecryptable,
    #[error("knock record was written by a sharing engine this build cannot read")]
    EngineIncompatible,
}

/// The record shape written before `direct_addrs` existed. Kept so a code published by an older
/// build still resolves to an address instead of looking like it was never published.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct KnockV0 {
    endpoint_id: EndpointKey,
    relay_urls: Vec<String>,
}

/// XChaCha20-Poly1305 with a random nonce, returned as `nonce ‖ ciphertext`. Also used for the
/// sharer's re-displayable `code_enc`.
pub fn seal_bytes(key: &[u8; 32], plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).expect("32-byte key");
    let nonce_bytes: [u8; 24] = rand::random();
    let nonce = XNonce::from(nonce_bytes);
    let mut out = nonce_bytes.to_vec();
    out.extend(
        cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .expect("in-memory encryption cannot fail"),
    );
    out
}

/// Inverse of `seal_bytes`; `None` when the key or data is wrong.
pub fn open_bytes(key: &[u8; 32], sealed: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < 24 {
        return None;
    }
    let (nonce, ciphertext) = sealed.split_at(24);
    let cipher = XChaCha20Poly1305::new_from_slice(key).ok()?;
    let nonce = XNonce::try_from(nonce).ok()?;
    cipher
        .decrypt(
            &nonce,
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .ok()
}

/// Builds and signs the pkarr packet for `knock`.
pub fn seal(keys: &DerivedKeys, knock: &KnockV1) -> Result<SignedPacket, RecordError> {
    let plaintext = postcard::to_allocvec(knock).map_err(|e| RecordError::Build(e.to_string()))?;
    seal_plaintext(keys, &plaintext)
}

/// Signs an already-encoded payload. Split out so the tests can publish the previous record shape
/// through the same path a real publisher uses.
fn seal_plaintext(keys: &DerivedKeys, plaintext: &[u8]) -> Result<SignedPacket, RecordError> {
    let sealed = seal_bytes(&keys.record_key, plaintext, RECORD_AAD);
    let mut txt = TXT::new();
    for chunk in sealed.chunks(CHUNK) {
        let string = CharacterString::new(chunk).map_err(|e| RecordError::Build(e.to_string()))?;
        txt.add_char_string(string.into_owned());
    }
    let name = Name::new(RECORD_NAME).map_err(|e| RecordError::Build(e.to_string()))?;
    SignedPacket::builder()
        .txt(name, txt, RECORD_TTL_SECS)
        .sign(&keys.lookup_key)
        .map_err(|e| RecordError::Build(e.to_string()))
}

/// Reads the knock record out of a packet already verified as signed by `keys.lookup_key`.
pub fn open(keys: &DerivedKeys, packet: &SignedPacket) -> Result<KnockV1, RecordError> {
    if packet.public_key().to_bytes() != keys.lookup_public() {
        return Err(RecordError::Undecryptable);
    }
    let txt = packet
        .resource_records(RECORD_NAME)
        .find_map(|rr| match &rr.rdata {
            RData::TXT(txt) => Some(txt),
            _ => None,
        })
        .ok_or(RecordError::Missing)?;
    // `iter_raw` splits each string at its first `=`; rejoining restores the exact bytes.
    let mut sealed = Vec::new();
    for (key, value) in txt.iter_raw() {
        sealed.extend_from_slice(key);
        if let Some(value) = value {
            sealed.push(b'=');
            sealed.extend_from_slice(value);
        }
    }
    let plaintext =
        open_bytes(&keys.record_key, &sealed, RECORD_AAD).ok_or(RecordError::Undecryptable)?;
    match postcard::from_bytes::<KnockV1>(&plaintext) {
        Ok(knock) => Ok(knock),
        // A record without `direct_addrs` predates it: postcard is positional, so the old shape
        // simply runs out of fields and parses on the second attempt.
        Err(_) => postcard::from_bytes::<KnockV0>(&plaintext)
            .map(|old| KnockV1 {
                endpoint_id: old.endpoint_id,
                relay_urls: old.relay_urls,
                direct_addrs: Vec::new(),
            })
            .map_err(|_| RecordError::EngineIncompatible),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code::ShareCode;

    fn knock() -> KnockV1 {
        KnockV1 {
            endpoint_id: [5; 32],
            relay_urls: vec![
                "https://relay-one.example.org/".into(),
                "https://relay-two.example.org/".into(),
                "https://a-much-longer-relay-hostname.example.net:8443/".into(),
            ],
            direct_addrs: vec![],
        }
    }

    /// The knock payload crosses two versions at once: a record published by a newer build must
    /// still be readable field-by-field by an older one, and vice versa. Pinning the bytes of a
    /// known payload catches any accidental field reorder or encoding change in review.
    #[test]
    fn the_knock_payload_bytes_are_stable() {
        let bytes = postcard::to_allocvec(&knock()).unwrap();
        let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "0505050505050505050505050505050505050505050505050505050505050505031e68747470733a2f2f72656c61792d6f6e652e6578616d706c652e6f72672f1e68747470733a2f2f72656c61792d74776f2e6578616d706c652e6f72672f3668747470733a2f2f612d6d7563682d6c6f6e6765722d72656c61792d686f73746e616d652e6578616d706c652e6e65743a383434332f00");
    }

    #[test]
    fn a_record_round_trips_through_its_relay_payload() {
        let keys = DerivedKeys::derive(&ShareCode::parse("K7QM9XRT2H").unwrap());
        let packet = seal(&keys, &knock()).unwrap();
        let payload = packet.to_relay_payload();
        let back =
            SignedPacket::from_relay_payload(&keys.lookup_key.public_key(), &payload).unwrap();
        assert_eq!(open(&keys, &back).unwrap(), knock());
    }

    #[test]
    fn another_code_cannot_read_the_record_and_nothing_readable_leaks() {
        let keys = DerivedKeys::derive(&ShareCode::parse("K7QM9XRT2H").unwrap());
        let other = DerivedKeys::derive(&ShareCode::parse("K7QM9XRT2J").unwrap());
        let packet = seal(&keys, &knock()).unwrap();
        assert!(open(&other, &packet).is_err());
        let wire = packet.as_bytes();
        for needle in [&b"relay-one"[..], b"lrrg", b"lanrurugi", b"iroh"] {
            assert!(!wire.windows(needle.len()).any(|w| w == needle));
        }
    }

    #[test]
    fn a_record_from_before_direct_addrs_still_resolves() {
        let keys = DerivedKeys::derive(&ShareCode::parse("K7QM9XRT2H").unwrap());
        let plaintext = postcard::to_allocvec(&KnockV0 {
            endpoint_id: [9; 32],
            relay_urls: vec!["https://relay.example.org/".into()],
        })
        .unwrap();
        let packet = seal_plaintext(&keys, &plaintext).unwrap();
        let knock = open(&keys, &packet).unwrap();
        assert_eq!(knock.endpoint_id, [9; 32]);
        assert_eq!(
            knock.relay_urls,
            vec!["https://relay.example.org/".to_string()]
        );
        assert!(knock.direct_addrs.is_empty());
    }

    #[test]
    fn an_unreadable_record_says_the_engine_is_incompatible() {
        let keys = DerivedKeys::derive(&ShareCode::parse("K7QM9XRT2H").unwrap());
        let packet = seal_plaintext(&keys, &[0xff, 0x00, 0x7f]).unwrap();
        assert!(matches!(
            open(&keys, &packet),
            Err(RecordError::EngineIncompatible)
        ));
    }

    #[test]
    fn sealed_bytes_need_the_right_key_and_aad() {
        let sealed = seal_bytes(&[1; 32], b"secret", b"aad");
        assert_eq!(open_bytes(&[1; 32], &sealed, b"aad").unwrap(), b"secret");
        assert!(open_bytes(&[2; 32], &sealed, b"aad").is_none());
        assert!(open_bytes(&[1; 32], &sealed, b"other").is_none());
        assert!(open_bytes(&[1; 32], &sealed[..10], b"aad").is_none());
    }
}
