//! Newtype wrappers around the raw `String` primary keys used throughout the domain (`Archive`,
//! `Category`, `Grouping`, `Stamp`). Before this module, every one of these was a plain `String`
//! field/parameter, which let e.g. a `CategoryId` be passed anywhere an `ArchiveId` was expected —
//! nothing caught that at compile time. These newtypes exist purely to close that gap; they carry
//! no new behavior and change no wire format (`#[serde(transparent)]` keeps every JSON/Redis
//! representation byte-identical to the plain `String` it replaces).
//!
//! Deliberately *not* implementing `redis::ToRedisArgs` here — that would pull a Redis dependency
//! into this otherwise storage-agnostic domain crate just for ergonomics. Callers passing an id to
//! a Redis command use `.as_str()` (or `&*id`, via `Deref`) instead.

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};

macro_rules! define_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_string(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                &self.0
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl From<$name> for String {
            fn from(id: $name) -> String {
                id.0
            }
        }

        impl PartialEq<str> for $name {
            fn eq(&self, other: &str) -> bool {
                self.0 == other
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }
    };
}

define_id!(
    ArchiveId,
    "An `Archive`'s primary key: either the legacy `SHA-1(first 512000 bytes)` or the new \
     size-aware `SHA-1(first 512000 bytes ++ u64 BE file size)` — both forms coexist (constitution \
     Principle I)."
);
define_id!(
    CategoryId,
    "A `Category`'s primary key (`SET_<10-digit-unix-timestamp>`)."
);
define_id!(
    TankId,
    "A `Grouping` (Tankoubon)'s primary key (`TANK_<10-digit-unix-timestamp>`)."
);
define_id!(
    StampId,
    "A `Stamp`'s primary key (`STAMPS_<page>_<millisecond-timestamp>`)."
);
define_id!(
    GroupSuggestionId,
    "One tankoubon-grouping suggestion's identity: the fingerprint of its sorted member ids plus \
     the existing tankoubon it would extend (`ignored_group_suggestions::suggestion_id`)."
);
define_id!(
    GenerationId,
    "One AI plugin-wizard generation's id, issued by `POST /plugin-wizard/generate/start`; keys \
     both the pending request and the generation record."
);
define_id!(
    ShareId,
    "A share's primary key: 16 random bytes in lowercase base32 (`random_base32_id`)."
);
define_id!(
    ClaimId,
    "One recipient claim against a share's download limit, same shape as `ShareId`."
);
define_id!(
    ShareImportId,
    "One import of another instance's share on the receiving side, same shape as `ShareId`."
);

macro_rules! impl_random_id {
    ($($name:ident),*) => {$(
        impl $name {
            pub fn generate() -> Self {
                Self(random_base32_id())
            }
        }
    )*};
}
impl_random_id!(ShareId, ClaimId, ShareImportId);

/// 16 random bytes as 26 characters of unpadded RFC 4648 base32, lowercased so the id is safe in
/// URLs, Redis keys and file names alike.
pub fn random_base32_id() -> String {
    base32_lower(uuid::Uuid::new_v4().as_bytes())
}

fn base32_lower(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod share_id_tests {
    use super::*;

    #[test]
    fn generated_ids_are_26_lowercase_base32_chars_and_distinct() {
        let a = ShareId::generate();
        let b = ShareId::generate();
        assert_eq!(a.len(), 26);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)));
        assert_ne!(a, b);
    }

    #[test]
    fn ids_round_trip_through_serde_as_plain_strings() {
        let id = ClaimId::generate();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
        assert_eq!(serde_json::from_str::<ClaimId>(&json).unwrap(), id);
    }

    #[test]
    fn base32_matches_rfc4648_vectors() {
        assert_eq!(base32_lower(b"foobar"), "mzxw6ytboi");
        assert_eq!(base32_lower(b"f"), "my");
    }
}
