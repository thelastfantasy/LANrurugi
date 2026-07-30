//! Domain types mirroring the legacy LANraragi Redis shape (constitution Principle I).
//!
//! Field names and key formats here were verified against `~/LANraragi`'s
//! `lib/LANraragi/{Utils/Database.pm,Model/{Category,Tankoubon,Stamp,Backup}.pm}`, not assumed
//! from `data-model.md` alone. Two corrections from that document, load-bearing for
//! `lanrurugi-storage`'s repository mappers:
//! - `Grouping` (Tankoubon) is a Redis **ZSET**, not a hash: metadata is packed as members
//!   `name_<v>`/`summary_<v>`/`tags_<v>`/`progress_<v>` at scores 0/-1/-2/-3, and archive IDs are
//!   members at positive scores (1..N) giving volume order.
//! - `ReadingProgress` is not a separate Redis key; it is just the `progress` (last-read page)
//!   and `lastreadtime` fields living directly on the `Archive` hash.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// A single trackable manga/comic work. Redis hash keyed directly by `id` (40 hex chars).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Archive {
    /// Primary key: either the legacy `SHA-1(first 512000 bytes)` or the new size-aware
    /// `SHA-1(first 512000 bytes ++ u64 BE file size)`. Both forms coexist (Principle I); which
    /// algorithm produced a given ID is not itself stored.
    pub id: String,
    /// Original filename (without extension), decoded. Legacy field `name`.
    pub name: String,
    /// User- or plugin-set display title. Falls back to `name` if blank (legacy `build_json`
    /// behavior).
    pub title: String,
    /// Absolute path on disk. Legacy field `file`.
    pub file: String,
    /// Comma-separated `namespace:value` tag string, kept flat (not normalized) to match legacy
    /// storage; namespace:tag parsing is a read-time concern for `lanrurugi-search`.
    pub tags: String,
    pub summary: String,
    /// File size in bytes at last scan/update. Legacy field `arcsize`.
    pub arcsize: u64,
    pub pagecount: u32,
    /// "Unread"/new marker. Legacy field `isnew` (stored as the literal strings `"true"`/`"false"`).
    pub isnew: bool,
    /// Last-read page number (0 if never read). Legacy field `progress`.
    pub lastreadpage: u32,
    /// Unix timestamp of last read. Legacy field `lastreadtime`.
    pub lastreadtime: u64,
    /// Hash of the thumbnail image, used for thumbnail cache invalidation.
    pub thumbhash: Option<String>,
    /// Table of contents: page -> chapter name.
    pub toc: Vec<TocEntry>,
    /// IDs of `Stamp`s attached to this archive (legacy `stamps` field, JSON array of stamp keys).
    pub stamp_ids: Vec<String>,
    /// Unix timestamp of the last time an automatic `pagecount`/`arcsize` heal attempt
    /// (`lanrurugi_scanner::full_scan::heal_pagecounts`) failed for this archive — `None` means
    /// either never attempted or the most recent attempt succeeded. Prevents the heal scan from
    /// retrying the same permanently-broken archive on every run (a startup-scan-triggered retry
    /// loop) — set once a heal attempt fails, cleared only by a fresh catalogue of this exact
    /// archive ID (e.g. re-downloading and overwriting it), never by another heal attempt itself.
    #[serde(default)]
    pub heal_failed_at: Option<u64>,
    /// Entry names (matching `archive_format::list_pages`'s own output, and `GET .../page`'s own
    /// `path` query param — e.g. `"page03.jpg"`) whose image bytes were found to be undecodable —
    /// the reader serves a placeholder image for these instead of retrying decode on every request
    /// or letting a corrupt byte stream reach the browser raw. Keyed by entry name (not a numeric
    /// page index) since that's exactly what both the detection site
    /// (`archives::generate_page_thumbnails`, which already iterates `list_pages`'s entry names)
    /// and the lookup site (`archives::fetch_page`, which only ever has the entry name from its
    /// own `path` param, never an index) naturally have on hand — avoids a second `list_pages` call
    /// on every page request just to translate an index back to a name. Empty for the overwhelming
    /// majority of archives that have no corrupt pages at all.
    #[serde(default)]
    pub corrupted_pages: Vec<String>,
}

impl Archive {
    /// Container format, derived from the file extension (never stored, per legacy `build_json`).
    pub fn extension(&self) -> String {
        self.file
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TocEntry {
    pub page: u32,
    pub name: String,
}

/// A saved grouping of archives. Redis hash keyed by `SET_<10-digit-unix-timestamp>` (14 chars).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Category {
    pub catid: String,
    pub name: String,
    /// If present, this is a dynamic/saved-search category (`archives` is not authoritative).
    pub search: Option<String>,
    /// Only meaningful for static categories (`search.is_none()`).
    pub archives: Vec<String>,
    pub pinned: bool,
}

impl Category {
    pub fn is_dynamic(&self) -> bool {
        self.search.is_some()
    }
}

/// A Tankoubon (volume grouping). Redis **ZSET** keyed by `TANK_<10-digit-timestamp>` (15 chars) —
/// see module docs for the packed-metadata layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grouping {
    pub tankid: String,
    pub name: String,
    pub summary: String,
    pub tags: String,
    /// The tank's own reading-progress marker (a global page number spanning all archives, per
    /// legacy `translate_global_page`) — distinct from any single archive's `lastreadpage`.
    pub progress: u32,
    /// Ordered archive IDs (order is significant: volume order).
    pub archives: Vec<String>,
}

/// A user-placed annotation ("stamp") on a specific page of an archive. Redis hash keyed by
/// `STAMPS_<page>_<millisecond-timestamp>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub stamp_id: String,
    pub content: String,
    pub position: String,
    pub archive_id: String,
}

impl Stamp {
    /// The page number a stamp belongs to is encoded in its own key (`STAMPS_<page>_<ts>`), not
    /// stored as a separate field, matching legacy `filter_stamps_by_page`.
    pub fn page(&self) -> Option<u32> {
        self.stamp_id
            .strip_prefix("STAMPS_")
            .and_then(|rest| rest.split('_').next())
            .and_then(|page| page.parse().ok())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionType {
    Metadata,
    Login,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionState {
    Disabled,
    Enabled,
    Running,
    Succeeded,
    Failed,
    TimedOut,
}

/// A metadata/login/download plugin, executed as a sandboxed Deno subprocess (Principle IV).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extension {
    pub namespace: String,
    #[serde(rename = "type")]
    pub kind: ExtensionType,
    pub parameters: HashMap<String, String>,
    pub enabled: bool,
    /// Capability grants declared up front (e.g. allowed network hosts), passed to Deno as
    /// `--allow-net=<hosts>` etc. New in LANrurugi — legacy Perl plugins had no permission model.
    pub declared_permissions: Vec<String>,
}
