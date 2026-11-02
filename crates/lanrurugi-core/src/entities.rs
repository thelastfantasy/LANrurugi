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

use crate::ids::{ArchiveId, CategoryId, StampId, TankId};

/// A single trackable manga/comic work. Redis hash keyed directly by `id` (40 hex chars).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Archive {
    /// Primary key: either the legacy `SHA-1(first 512000 bytes)` or the new size-aware
    /// `SHA-1(first 512000 bytes ++ u64 BE file size)`. Both forms coexist (Principle I); which
    /// algorithm produced a given ID is not itself stored.
    pub id: ArchiveId,
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
    pub stamp_ids: Vec<StampId>,
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
    /// Whether a sidecar `.patch.zip` (`lanrurugi_scanner::patch`, issue #77's own follow-on
    /// design) currently exists next to this archive's own file — persisted here (rather than a
    /// live `Path::exists` check on every read) so the library grid can show a per-card badge
    /// without paying a filesystem stat for every archive on every page load. Kept in sync by
    /// every code path that writes or removes a patch file (`download_queue.rs`'s
    /// `keep_side_b`/`overwrite_queue_item`), not derived automatically — there is no filesystem
    /// watch on patch files themselves, only on watched archive extensions
    /// (`watcher::is_watched_archive_path` explicitly excludes them).
    #[serde(default)]
    pub has_patch: bool,
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

    /// The `date_added:<unix_seconds>` namespace's value out of `tags` — there's no dedicated
    /// Redis hash field for this (unlike `pagecount`/`lastreadtime`), so it has to be scanned out
    /// of the comma-separated tags string, same logic `lanrurugi_search::engine`'s own
    /// `date_added:YYYY-MM-DD` range-query handling already does inline against a raw `tags`
    /// string read straight from Redis (that call site keeps its own copy rather than adopting
    /// this method — it never has a full `Archive` in hand, just the bare tags string). Tolerates
    /// a malformed/missing value (`None`) rather than erroring.
    pub fn date_added(&self) -> Option<u64> {
        self.tags.split(',').find_map(|t| {
            t.trim()
                .strip_prefix("date_added:")
                .and_then(|v| v.trim().parse().ok())
        })
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
    pub catid: CategoryId,
    pub name: String,
    /// If present, this is a dynamic/saved-search category (`archives` is not authoritative).
    pub search: Option<String>,
    /// Only meaningful for static categories (`search.is_none()`). Despite the field name and
    /// type, members are not restricted to real archive ids — legacy's own `add_to_category`
    /// (`Model::Category`) only checks `$redis->exists($id)`, a generic key-existence test that a
    /// `TANK_`-prefixed Tankoubon id satisfies just as well as an archive id, so legacy genuinely
    /// supports static categories containing Tankoubons. `ArchiveId` stays the field's type rather
    /// than introducing a dedicated archive-or-tank enum: it's a transparent `String` newtype with
    /// no validation of its own (see `ids.rs`'s own docs — the newtype exists to prevent
    /// *cross-signature* mixups, e.g. a `CategoryId` landing where an `ArchiveId` was expected, not
    /// to restrict what a given id conceptually refers to), so a `TankId`-shaped value inside it is
    /// not the type confusion that principle guards against.
    pub archives: Vec<ArchiveId>,
    pub pinned: bool,
    /// Whether an unauthenticated guest visitor (007-guest-restricted-access) can see archives
    /// belonging to this category. Absent on any category record predating that feature —
    /// defaults to `false` (see `CategoryRepository::get`'s own read-side handling).
    pub visible_to_guest: bool,
}

impl Category {
    pub fn is_dynamic(&self) -> bool {
        self.search.is_some()
    }
}

/// One entry in a Tankoubon's ordered chapter-name list. Separated from Grouping
/// so the JSON serialisation is always `[{id, name}]`, never a key-ordered `{id: name}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChapterNameEntry {
    pub id: String,
    pub name: String,
}

/// A Tankoubon (volume grouping). Redis **ZSET** keyed by `TANK_<10-digit-timestamp>` (15 chars) —
/// see module docs for the packed-metadata layout.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grouping {
    pub tankid: TankId,
    pub name: String,
    pub summary: String,
    pub tags: String,
    /// The tank's own reading-progress marker (a global page number spanning all archives, per
    /// legacy `translate_global_page`) — distinct from any single archive's `lastreadpage`.
    pub progress: u32,
    /// Ordered archive IDs (order is significant: volume order).
    pub archives: Vec<ArchiveId>,
    /// `true` once someone has explicitly picked a cover via `PUT /tankoubons/{id}/thumbnail`
    /// (the reader overview overlay's "set as cover" action). Additive beyond legacy (which has no
    /// such flag): while `false`, the tank's own cached cover thumbnail is kept in sync with its
    /// first member archive's cover whenever the archive list changes — matches the same "use the
    /// first volume's cover" default a freshly-generated thumbnail already has, just kept live
    /// instead of going stale the moment volume order changes. Once `true`, the cover is sticky
    /// across archive-list edits — *unless* `thumbnail_source_archive` itself stops being a member
    /// (see that field's own docs), which forces a reset back to auto-follow regardless of this
    /// flag; a manually-picked cover whose source page no longer exists in the tank at all isn't
    /// "sticky", it's stale.
    pub thumbnail_manual: bool,
    /// Which member archive `thumbnail_manual`'s cover was extracted from, and which of that
    /// archive's own local pages — both only meaningful while `thumbnail_manual` is `true`
    /// (`None` otherwise). Kept as a full "recipe" (archive *and* page), not just the archive,
    /// even though only the archive half is needed to answer "is the source still valid" — the
    /// page is what would let the exact same cover be regenerated later (cache invalidation, a
    /// thumbnail-format change, ...) without re-asking the user, and storing an incomplete recipe
    /// once it's already this cheap to store the real one isn't a savings worth making.
    ///
    /// Checked (against `archives`, and against the source archive's own *current* `pagecount` —
    /// deliberately not stored redundantly here, since it can change out from under a stale
    /// snapshot on a rescan) whenever the archive list changes: if the source archive is no longer
    /// a member, or its own page count has shrunk past the stored page, the manual cover is no
    /// longer valid for *this* Tankoubon and gets reset — back to auto-following the new first
    /// member's own cover if one remains, or cleared entirely (falling back to the placeholder) if
    /// the tank is now empty.
    pub thumbnail_source_archive: Option<ArchiveId>,
    pub thumbnail_source_page: Option<u32>,
    /// Per-member chapter names in archive order — an ordered list, not a map,
    /// so the JSON serialisation preserves the same order as the archives array.
    #[serde(default)]
    pub chapter_names: Vec<ChapterNameEntry>,
    /// Unix timestamp of creation (from `TANK_` ID prefix or explicit set).
    #[serde(default)]
    pub created_at: Option<u64>,
    /// Unix timestamp of last modification — refreshed on every PUT. Drives
    /// `date_added`/`timestamp` sort for Tankoubons so a recently-edited one
    /// surfaces to the top.
    #[serde(default)]
    pub updated_at: Option<u64>,
}

/// A user-placed annotation ("stamp") on a specific page of an archive. Redis hash keyed by
/// `STAMPS_<page>_<millisecond-timestamp>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stamp {
    pub stamp_id: StampId,
    pub content: String,
    pub position: String,
    pub archive_id: ArchiveId,
    /// User-picked icon shown in place of the default marker pin — either a literal emoji
    /// character, or a Font Awesome class name prefixed `fa:` (e.g. `fa:fa-heart`, disambiguating
    /// it from a literal emoji string at render time). Empty string (the zero value, so every
    /// pre-existing stamp created before this field existed round-trips as "no custom icon" without
    /// a migration) falls back to the default marker.
    #[serde(default)]
    pub icon: String,
    /// Optional selection rectangle a stamp can carry in addition to its plain `position` point —
    /// `"x,y,width,height,anchor,color"` (`x`/`y`/`width`/`height` percent of the page image,
    /// `anchor` one of 8 short codes for where the icon sits on the rect's own border — `tl`/`t`/
    /// `tr`/`r`/`br`/`b`/`bl`/`l` — and `color` a `#rrggbb` hex string for the rect's own outline).
    /// Empty string (same zero-value convention as `icon`) means this stamp is a plain point with
    /// no rectangle, which is how every stamp existed before this field was added.
    #[serde(default)]
    pub rect: String,
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
