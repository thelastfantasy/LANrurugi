//! A node's short, permanent public name.
//!
//! The full `lrrg-node:<endpoint id>@<host:port>?relay=…` string is exact but long, and half of it
//! goes stale (the address changes with DHCP or a new tunnel). A node code is the same idea as a
//! share code: a short random name whose *lookup key* is derived from the code itself, so whoever
//! holds the code can fetch the node's current record from the rendezvous service without any
//! central index, and the record carries today's addresses rather than the ones from when the code
//! was printed.
//!
//! 16 characters of Crockford base32 is 80 bits: a random collision needs on the order of 2^40
//! nodes, and the pkarr key derived from the code is unique for the same reason. The code lives in
//! the instance's own Redis, so it survives restarts and upgrades; rotating it (which deletes the
//! stored code and the node secret) is the only way to retire one.

use std::fmt;

use hkdf::Hkdf;
use sha2::{Digest, Sha256};

use crate::code::ALPHABET;

/// Characters in a node code.
pub const NODE_CODE_LEN: usize = 16;

const NODE_LOOKUP_SALT: &[u8] = b"lrrg-node-lookup-v1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NodeCodeError {
    #[error("a node code has {NODE_CODE_LEN} characters")]
    WrongLength,
    #[error("a node code contains only letters and digits")]
    InvalidCharacter,
}

/// A normalised node code (uppercase, no separators).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NodeCode(String);

impl NodeCode {
    /// A fresh random code: 80 bits.
    pub fn generate() -> Self {
        let mut code = String::with_capacity(NODE_CODE_LEN);
        while code.len() < NODE_CODE_LEN {
            // 32 bits per draw keeps the rejection loop short: 6 draws is 192 bits for 80 needed.
            let bits: u32 = rand::random();
            for i in 0..6 {
                if code.len() == NODE_CODE_LEN {
                    break;
                }
                code.push(ALPHABET[((bits >> (5 * i)) & 31) as usize] as char);
            }
        }
        Self(code)
    }

    /// Normalises user input the way a share code does: case-insensitive, `O`→`0`, `I`/`L`→`1`,
    /// dashes and spaces ignored.
    pub fn parse(input: &str) -> Result<Self, NodeCodeError> {
        let mut code = String::with_capacity(NODE_CODE_LEN);
        for c in input.chars() {
            if c == '-' || c.is_whitespace() {
                continue;
            }
            let c = match c.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
                return Err(NodeCodeError::InvalidCharacter);
            }
            code.push(c);
        }
        if code.len() != NODE_CODE_LEN {
            return Err(NodeCodeError::WrongLength);
        }
        Ok(Self(code))
    }

    /// A pasted `lrrg-node:` that is really a bare code.
    pub fn parse_prefixed(input: &str) -> Result<Self, NodeCodeError> {
        let rest = input.trim().strip_prefix("lrrg-node:").unwrap_or(input);
        // A full node string carries `@`; it is not a code.
        if rest.contains('@') {
            return Err(NodeCodeError::WrongLength);
        }
        Self::parse(rest)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `XXXX-XXXX-XXXX-XXXX`, which is how it is shown and copied.
    pub fn display(&self) -> String {
        self.0
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("-")
    }

    /// The pkarr key pair the node's record is published under and looked up by.
    pub fn lookup_keypair(&self) -> pkarr::Keypair {
        let hkdf = Hkdf::<Sha256>::new(Some(NODE_LOOKUP_SALT), self.0.as_bytes());
        let mut secret = [0u8; 32];
        hkdf.expand(b"node-lookup", &mut secret)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        pkarr::Keypair::from_secret_key(&secret)
    }

    /// `SHA-256(lookup public key)`, the same shape a share code uses for LAN resolution.
    pub fn lookup_hash(&self) -> [u8; 32] {
        Sha256::digest(self.lookup_keypair().public_key().to_bytes()).into()
    }
}

impl fmt::Display for NodeCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

impl fmt::Debug for NodeCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the raw code in logs: it is the whole capability.
        f.write_str("NodeCode(…)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip_through_display_and_parse() {
        let code = NodeCode::generate();
        assert_eq!(code.as_str().len(), NODE_CODE_LEN);
        assert_eq!(NodeCode::parse(&code.display()).unwrap(), code);
        assert_eq!(NodeCode::parse(code.as_str()).unwrap(), code);
        // The user-visible form tolerates the usual transcription mistakes.
        assert_eq!(
            NodeCode::parse(&code.display().to_lowercase()).unwrap(),
            code
        );
        assert_eq!(
            NodeCode::parse_prefixed(&format!("lrrg-node:{}", code.display())).unwrap(),
            code
        );
    }

    #[test]
    fn wrong_lengths_and_characters_are_rejected() {
        assert!(NodeCode::parse("ABC").is_err());
        assert!(NodeCode::parse("0123456789ABCDEG!").is_err());
        assert!(NodeCode::parse("lrrg-node:abc@host:1").is_err());
    }

    #[test]
    fn distinct_codes_derive_distinct_lookup_keys() {
        let a = NodeCode::generate();
        let b = NodeCode::generate();
        assert_ne!(a.lookup_hash(), b.lookup_hash());
        // Deterministic: the same code always finds the same record.
        assert_eq!(
            a.lookup_hash(),
            NodeCode::parse(&a.display()).unwrap().lookup_hash()
        );
    }
}
