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
