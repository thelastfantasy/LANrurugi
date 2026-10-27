//! Keys derived from a share code (contracts/share-protocol.md "Code derivation", research §2).
//!
//! The sharer stores only `access_hash`; the knock record is published under a key pair and
//! encrypted with a key that only a code holder can derive. Argon2id makes each guess at a code
//! expensive for anyone trying codes against published records.

use argon2::{Algorithm, Argon2, Params, Version};
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

use crate::code::ShareCode;

const ACCESS_DOMAIN: &[u8] = b"lrrg-share-access-v1";
const LOOKUP_SALT: &[u8] = b"lrrg-share-lookup-v1";
const ARGON2_M_KIB: u32 = 64 * 1024;
const ARGON2_T: u32 = 3;
const ARGON2_P: u32 = 1;

/// `SHA-256("lrrg-share-access-v1" ‖ code)`: what an importer sends in `Open`, and what the sharer
/// stores as `code_hash`.
pub fn access_hash(code: &ShareCode) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(ACCESS_DOMAIN);
    hasher.update(code.as_str().as_bytes());
    hasher.finalize().into()
}

/// Everything derived from the slow Argon2id seed. Computing it takes a noticeable fraction of a
/// second and 64 MiB, so callers run it on a blocking thread.
#[derive(Clone)]
pub struct DerivedKeys {
    /// The pkarr key pair the knock record is published under.
    pub lookup_key: pkarr::Keypair,
    /// XChaCha20-Poly1305 key for the knock record.
    pub record_key: [u8; 32],
}

impl DerivedKeys {
    pub fn derive(code: &ShareCode) -> Self {
        let seed = argon2_seed(code);
        let hkdf = Hkdf::<Sha256>::new(None, &seed);
        let mut lookup_secret = [0u8; 32];
        let mut record_key = [0u8; 32];
        hkdf.expand(b"lookup", &mut lookup_secret)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        hkdf.expand(b"record", &mut record_key)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        Self {
            lookup_key: pkarr::Keypair::from_secret_key(&lookup_secret),
            record_key,
        }
    }

    pub fn lookup_public(&self) -> [u8; 32] {
        self.lookup_key.public_key().to_bytes()
    }

    /// `SHA-256(lookup public key)`, asked of LAN peers in `ResolveLocal`.
    pub fn lookup_hash(&self) -> [u8; 32] {
        Sha256::digest(self.lookup_public()).into()
    }
}

fn argon2_seed(code: &ShareCode) -> [u8; 32] {
    let params = Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, Some(32))
        .expect("fixed Argon2 parameters are valid");
    let mut seed = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(code.as_str().as_bytes(), LOOKUP_SALT, &mut seed)
        .expect("fixed Argon2 inputs are valid");
    seed
}
