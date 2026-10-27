//! Request and response messages (contracts/share-protocol.md). Ids are plain strings on the wire;
//! the server converts them to its own newtypes.

use serde::{Deserialize, Serialize};

use crate::ticket::SignedTicket;

/// The protocol version carried by every request. ALPN is `h3`, so this is the only place the
/// application protocol is identified (FR-045).
/// 2: archive entries carry their table of contents (`ArchiveEntry::toc`).
/// The sharing engine's version: the wire protocol this build speaks, carried in every handshake.
/// Two instances must run the same one to import from, or help deliver to, each other; browsing a
/// share (page, card, covers, reading) is plain HTTP and never checks it.
pub const SHARE_ENGINE_VERSION: u16 = 3;

/// The transport generation this engine version speaks: iroh's QUIC/relay wire protocol is not
/// negotiated separately, so a bump that breaks it means a new `SHARE_ENGINE_VERSION` as well. The
/// settings page shows this string so two administrators can compare. A mismatch here cannot even
/// reach the version handshake — the connection itself fails — which is why the unreachable message
/// points at both values.
pub const SHARE_TRANSPORT: &str = "iroh 1.4.1";

/// Everything two instances must agree on before one imports from the other: a value with one field
/// per independent compatibility axis, not a single integer. The protocol epoch covers the message
/// schemas; the transport covers the QUIC/relay generation underneath them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareEngine {
    /// Wire protocol epoch: every message's shape, and this handshake.
    pub protocol: u16,
    /// Transport generation, e.g. `iroh 1.4.1`.
    pub transport: String,
}

impl ShareEngine {
    /// What this build speaks.
    pub fn current() -> Self {
        Self {
            protocol: SHARE_ENGINE_VERSION,
            transport: SHARE_TRANSPORT.to_string(),
        }
    }

    /// Every axis has to match. The transport cannot be negotiated — iroh has no version handshake —
    /// and the policy is that a wire-incompatible iroh bump moves the protocol epoch with it, so a
    /// differing transport means two builds that were never meant to talk.
    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.protocol == other.protocol && self.transport == other.transport
    }
}

impl std::fmt::Display for ShareEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "v{} ({})", self.protocol, self.transport)
    }
}

pub type Blake3Hash = [u8; 32];
/// An instance's ed25519 public key, i.e. its iroh `EndpointId`.
pub type EndpointKey = [u8; 32];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub import: bool,
    pub read: bool,
    pub download: bool,
}

impl Permissions {
    pub const ALL: Self = Self {
        import: true,
        read: true,
        download: true,
    };

    pub fn is_empty(&self) -> bool {
        !(self.import || self.read || self.download)
    }
}

/// One chapter: its 1-based first page and its name (a library preset identifier or free text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chapter {
    pub page: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveEntry {
    pub archive_id: String,
    pub title: String,
    pub tags: String,
    pub summary: String,
    pub pagecount: u32,
    pub filename: String,
    pub size: u64,
    /// Present only in a claim manifest, and only once the sharer has fingerprinted the archive;
    /// `None` there means "send `Prepare` first".
    pub blake3: Option<Blake3Hash>,
    /// Names of the sharer's static categories holding the archive; only in a claim manifest,
    /// so link recipients never see them.
    #[serde(default)]
    pub categories: Vec<String>,
    /// The archive's chapters, for the recipient's reader and an import. `default` reads import
    /// records saved before chapters were carried.
    #[serde(default)]
    pub toc: Vec<Chapter>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManifestItem {
    Archive(ArchiveEntry),
    Tankoubon {
        id: String,
        name: String,
        tags: String,
        summary: String,
        /// In reading order.
        members: Vec<ArchiveEntry>,
    },
    /// The item was deleted from the sharer's library.
    Unavailable {
        title: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareManifest {
    pub share_id: String,
    pub title: String,
    pub permissions: Permissions,
    pub items: Vec<ManifestItem>,
}

impl ShareManifest {
    /// Every archive entry, in manifest order, including tankoubon members.
    pub fn archives(&self) -> impl Iterator<Item = &ArchiveEntry> {
        self.items.iter().flat_map(|item| match item {
            ManifestItem::Archive(a) => std::slice::from_ref(a).iter(),
            ManifestItem::Tankoubon { members, .. } => members.iter(),
            ManifestItem::Unavailable { .. } => [].iter(),
        })
    }
}

/// A span of BLAKE3 chunks (1 KiB each), end exclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChunkSpan {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimKind {
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelpState {
    Active,
    Paused,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HelpReason {
    Exhausted,
    Revoked,
    Expired,
    HelpingDisabled,
}

/// A helper as the sharer hands it to importers: enough to dial it without any lookup service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperAddr {
    pub endpoint_id: EndpointKey,
    pub relay_urls: Vec<String>,
    pub addrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperStatus {
    pub state: HelpState,
    pub reason: Option<HelpReason>,
    /// Unix seconds when this state began.
    pub at: u64,
    /// Sharer archive ids no longer in the share.
    pub removed_archives: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum RefusalReason {
    #[error("not found")]
    NotFound,
    #[error("revoked")]
    Revoked,
    #[error("expired")]
    Expired,
    #[error("downloads used up")]
    Exhausted,
    #[error("permission denied")]
    PermissionDenied,
    #[error("address could not be verified")]
    AddressUnverifiable,
    #[error("too many attempts")]
    RateLimited,
    /// Own-share uploads are paused; `until` is unix seconds, absent for a manual pause.
    #[error("sharer paused uploads")]
    Paused { until: Option<u64> },
    #[error("unsupported protocol version")]
    UnsupportedVersion { engine: ShareEngine },
    #[error("internal error")]
    Internal,
}

impl RefusalReason {
    /// The stable code used in HTTP errors and the UI.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
            Self::Exhausted => "exhausted",
            Self::PermissionDenied => "permission_denied",
            Self::AddressUnverifiable => "address_unverifiable",
            Self::RateLimited => "rate_limited",
            Self::Paused { .. } => "paused",
            Self::UnsupportedVersion { .. } => "unsupported_version",
            Self::Internal => "internal",
        }
    }
}

/// The first and only message a client writes on a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    ResolveLocal {
        lookup_hash: [u8; 32],
    },
    Open {
        access_hash: [u8; 32],
    },
    Thumbnail {
        session: String,
        archive_id: String,
    },
    Claim {
        session: String,
        kind: ClaimKind,
    },
    Prepare {
        session: String,
        claim_id: String,
        archive_id: String,
    },
    Renew {
        claim_id: String,
    },
    Range {
        ticket: SignedTicket,
        blake3: Blake3Hash,
        spans: Vec<ChunkSpan>,
    },
    Outboard {
        ticket: SignedTicket,
        blake3: Blake3Hash,
    },
    /// `archives` are the sharer's archive ids the helper holds; `relay_urls` and `addrs` are
    /// where importers can dial it.
    HelperRegister {
        share_id: String,
        archives: Vec<String>,
        relay_urls: Vec<String>,
        addrs: Vec<String>,
    },
    HelperHeartbeat {
        share_id: String,
    },
    RendezvousPut {
        signed_packet: Vec<u8>,
    },
    RendezvousGet {
        lookup_pubkey: [u8; 32],
    },
}

/// The response header. `Thumbnail`, `Range` and `Outboard` are followed by `len` raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    Found {
        share_id: String,
    },
    NotHere,
    Opened {
        session: String,
        manifest: ShareManifest,
        public_url: Option<String>,
        /// Whether this sharer lets other instances help deliver; the import dialog only offers
        /// "help deliver" when it does.
        allow_helpers: bool,
    },
    Bytes {
        content_type: Option<String>,
        len: u64,
    },
    Claimed {
        claim_id: String,
        manifest: ShareManifest,
        ticket: SignedTicket,
        helpers: Vec<HelperAddr>,
    },
    Prepared {
        blake3: Blake3Hash,
        size: u64,
    },
    /// A renewal: the fresh ticket plus the helpers active right now.
    Renewed {
        ticket: SignedTicket,
        helpers: Vec<HelperAddr>,
    },
    HelperStatus(HelperStatus),
    Ok,
    Packet {
        signed_packet: Vec<u8>,
    },
    Refused(RefusalReason),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str) -> ArchiveEntry {
        ArchiveEntry {
            archive_id: id.into(),
            title: id.into(),
            tags: String::new(),
            summary: String::new(),
            pagecount: 1,
            filename: format!("{id}.zip"),
            size: 1,
            blake3: None,
            categories: Vec::new(),
            toc: vec![Chapter {
                page: 1,
                name: "cover".into(),
            }],
        }
    }

    #[test]
    fn manifest_archives_walks_tankoubon_members_in_order() {
        let manifest = ShareManifest {
            share_id: "s".into(),
            title: "t".into(),
            permissions: Permissions::ALL,
            items: vec![
                ManifestItem::Archive(entry("a")),
                ManifestItem::Unavailable { title: None },
                ManifestItem::Tankoubon {
                    id: "TANK_1".into(),
                    name: "n".into(),
                    tags: String::new(),
                    summary: String::new(),
                    members: vec![entry("b"), entry("c")],
                },
            ],
        };
        let ids: Vec<_> = manifest.archives().map(|a| a.archive_id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn a_share_engine_reports_every_axis() {
        let current = ShareEngine::current();
        assert_eq!(current.protocol, SHARE_ENGINE_VERSION);
        assert_eq!(current.transport, SHARE_TRANSPORT);
        assert!(current.is_compatible_with(&ShareEngine::current()));
        assert!(!current.is_compatible_with(&ShareEngine {
            protocol: SHARE_ENGINE_VERSION + 1,
            transport: SHARE_TRANSPORT.to_string(),
        }));
        // Same protocol, different transport: two builds that were never meant to talk.
        assert!(!current.is_compatible_with(&ShareEngine {
            protocol: SHARE_ENGINE_VERSION,
            transport: "iroh 9.9.9".to_string(),
        }));
        assert_eq!(
            current.to_string(),
            format!("v{SHARE_ENGINE_VERSION} ({SHARE_TRANSPORT})")
        );
    }

    #[test]
    fn refusal_codes_are_stable() {
        assert_eq!(RefusalReason::Paused { until: None }.code(), "paused");
        assert_eq!(
            RefusalReason::AddressUnverifiable.code(),
            "address_unverifiable"
        );
    }
}
