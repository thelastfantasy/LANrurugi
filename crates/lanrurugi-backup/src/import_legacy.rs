//! Imports metadata from a LANraragi (or LANrurugi) backup JSON file the user has downloaded
//! separately (e.g. via LANraragi's own `GET /api/backup`) and uploads through the frontend —
//! **not** a live connection to any remote Redis instance. That earlier design was rejected: the
//! official LANraragi Docker image bundles its own `valkey`/`valkey-cli` (Redis-protocol
//! compatible) process *inside* the same container, and its own `redis.conf` hardcodes `bind
//! 127.0.0.1` + `protected-mode yes` — the default deployment shape never exposes a Redis port
//! to the outside world at all, so "type in a remote Redis address" was never a workable
//! interaction to begin with.
//!
//! **Deliberately independent of [`crate::build`]/[`crate::restore`]** — despite superficially
//! similar field names, this module's own [`LegacyBackupDocument`] is not [`crate::build::
//! BackupDocument`], and this module never calls [`crate::restore::restore`]. The two features
//! serve different scenarios: `restore()` re-attaches a LANrurugi-authored backup to matching ids
//! on the *same* instance it came from (pure "does this id already exist, yes/no" semantics);
//! this module reconciles a *foreign* LANraragi library's export against archives that may have
//! been re-keyed by `rebuild-index`, need a filename-based fallback match, and require a
//! conflict-resolution policy for archives that already exist here — none of which `restore()`
//! was ever designed to do. Field-name similarity is not a reason to force two conceptually
//! different operations through one code path.

use std::collections::HashMap;

use lanrurugi_core::entities::{Category, Grouping, Stamp};
use lanrurugi_core::ids::{ArchiveId, CategoryId, StampId, TankId};
use lanrurugi_storage::repository::{
    ArchiveRepository, CategoryRepository, GroupingRepository, RepositoryError, StampRepository,
};
use serde::Deserialize;

use crate::build::{BackupArchive, BackupDocument};

/// One archive entry in a LANraragi backup JSON — field names verified against
/// `~/LANraragi/lib/LANraragi/Model/Backup.pm::build_backup_JSON` (`%arc = (arcid, title, tags,
/// summary, thumbhash, filename, stamps)`), not against this crate's own `BackupArchive` (see
/// this module's own top-of-file docs on why the two are deliberately not shared).
///
/// `stamps` (the per-archive field `build_backup_JSON` also writes, distinct from the top-level
/// `stamps[]` array) is deliberately *not* mapped here — the top-level array is the authoritative
/// source for this import; `serde` silently ignores the extra field, no explicit skip needed.
/// `title`/`tags`/`summary`/`filename` are `Option<String>`, not plain `String` with
/// `#[serde(default)]` — `#[serde(default)]` only fires when a key is *absent* from the JSON
/// object; it does nothing for a key that's present with an explicit `null` value, which a real
/// LANraragi export can and does contain (confirmed live against a real backup export,
/// 2026-08-29: 3 of 6228 archive records had every one of these four fields as JSON `null`,
/// presumably a legacy-side data-corruption artifact predating this import feature). Deserializing
/// those straight into `String` failed the whole document, not just those records — every other
/// use site below reads through [`LegacyArchive::title_or_empty`]/[`filename_or_empty`] etc.
/// rather than the raw field, so a `null` here degrades to "empty string" (same as an absent
/// field always did) instead of rejecting the entire import over a handful of corrupted records.
#[derive(Debug, Clone, Deserialize)]
pub struct LegacyArchive {
    pub arcid: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub tags: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub thumbhash: Option<String>,
    /// Basename only (no extension, no directory) — legacy's own `name` field, *not* a full disk
    /// path. `Model/Backup.pm` line 153 (`filename => $name`) confirms this; a real export never
    /// carries the full `file` path, so id-reconciliation fallback below can only ever match by
    /// basename, not full path. `None`/`null` means this record can never be matched by the
    /// basename fallback (only by exact `arcid`) — see [`LegacyArchive::filename_or_empty`].
    #[serde(default)]
    pub filename: Option<String>,
}

impl LegacyArchive {
    pub fn title_or_empty(&self) -> String {
        self.title.clone().unwrap_or_default()
    }
    pub fn tags_or_empty(&self) -> String {
        self.tags.clone().unwrap_or_default()
    }
    pub fn summary_or_empty(&self) -> String {
        self.summary.clone().unwrap_or_default()
    }
    pub fn filename_or_empty(&self) -> String {
        self.filename.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegacyCategory {
    pub catid: String,
    pub name: String,
    pub search: Option<String>,
    #[serde(default)]
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegacyTankoubon {
    pub tankid: String,
    pub name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub archives: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LegacyStamp {
    pub stamp_id: String,
    pub content: String,
    pub position: String,
    pub archive_id: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct LegacyBackupDocument {
    #[serde(default)]
    pub archives: Vec<LegacyArchive>,
    #[serde(default)]
    pub categories: Vec<LegacyCategory>,
    #[serde(default)]
    pub tankoubons: Vec<LegacyTankoubon>,
    #[serde(default)]
    pub stamps: Vec<LegacyStamp>,
}

/// How to reconcile a record that already exists on *this* instance (matched by unambiguous
/// basename for an archive, by name for a category/tankoubon, by `(archive, position)` for a
/// stamp — never by the legacy record's own id, see Pass 2/5's own docs) with the legacy record
/// for the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportConflictMode {
    /// `title`/`tags`/`summary`/`thumbhash` all overwritten with the legacy library's values.
    Overwrite,
    /// `tags` becomes the union of both libraries' tags (case-insensitively deduplicated);
    /// `title`/`summary`/`thumbhash` keep this instance's own current values rather than being
    /// replaced — avoids silently discarding an edit already made on this instance while still
    /// pulling in whatever tagging (e.g. a `rating:` tag) only the legacy side has.
    Merge,
    /// This archive's metadata is left completely untouched; only its membership in
    /// categories/tankoubons/stamps (rewritten to point at this instance's own id) is still
    /// imported.
    Skip,
}

#[derive(Debug, Default, Clone)]
pub struct ImportLegacySummary {
    pub archives_updated: usize,
    /// [`ImportConflictMode::Skip`]-only: matched (by unambiguous basename) but deliberately left
    /// untouched.
    pub archives_skipped_already_exists: usize,
    /// No basename match on this instance.
    pub archives_skipped_no_match: usize,
    /// The legacy record's basename matched *more than one* archive on this instance —
    /// accuracy over recall: never guessed, the record is excluded entirely (its id never enters
    /// the reconciliation map, so anything referencing it — categories, tankoubons, stamps — also
    /// silently drops the reference rather than pointing at a possibly-wrong archive).
    pub archives_ambiguous_match: usize,
    /// How many *distinct* legacy `arcid`s independently resolved (by basename) to the same
    /// archive on this instance — e.g. two legacy records for what turned out to be duplicate
    /// filenames pointing at one real file here. Each such legacy record still gets written (last
    /// one processed wins, same as any other repeated write to the same target), this count is
    /// purely informational so the caller can flag "N legacy records collapsed onto fewer targets
    /// than expected" rather than the import silently doing that with no visibility.
    pub archives_multiple_legacy_records_same_target: usize,
    /// How many `title`/`filename` fields were detected as mojibake (double-encoded UTF-8) and
    /// repaired — see [`repair_mojibake`]'s own docs for the detection criteria.
    pub titles_mojibake_repaired: usize,
    pub categories_restored: usize,
    /// [`ImportConflictMode::Skip`]-only: matched an existing category by name but deliberately
    /// left untouched.
    pub categories_skipped_already_exists: usize,
    pub tankoubons_restored: usize,
    /// [`ImportConflictMode::Skip`]-only: matched an existing tankoubon by name but deliberately
    /// left untouched.
    pub tankoubons_skipped_already_exists: usize,
    pub stamps_restored: usize,
    /// [`ImportConflictMode::Skip`]-only: matched an existing stamp (same archive + position) but
    /// deliberately left untouched.
    pub stamps_skipped_already_exists: usize,
    /// `catid`/`tankid`/`stamp_id` values on *this* instance — new records this import created
    /// that have no prior version, so [`import_from_legacy`]'s own snapshot (differential-apply,
    /// never deletes — see that function's own docs) cannot roll them back. Archives are never
    /// created fresh by this import (a legacy archive with no basename match is skipped entirely,
    /// never turned into a new archive record here — actual file ingestion is the scanner's job),
    /// so there is no `new_archive_ids` counterpart. Surfaced to the caller so the frontend can
    /// warn "N brand-new records were also created and won't be removed by restoring this
    /// snapshot" instead of implying a full undo.
    pub new_category_ids: Vec<String>,
    pub new_tankoubon_ids: Vec<String>,
    pub new_stamp_ids: Vec<String>,
}

/// Detects and repairs a single mojibake string (UTF-8 bytes that were mistakenly re-encoded as
/// Latin-1, e.g. "イジイセ" round-tripped into "ã¤ã¸ã¤ã»") — verified against a real LANraragi
/// export (`Utils/Redis.pm::redis_decode`'s own "Final Solution to the Unicode glitches" comment
/// confirms this is a known, real historical data-quality issue in LANraragi's own Redis data,
/// not an artifact of any particular export/import path).
///
/// Returns `Some(repaired)` only when **all** of the following hold — deliberately strict, since
/// the accuracy requirement here means a false-positive "repair" (corrupting an already-correct
/// string) is worse than leaving a real mojibake string alone:
/// 1. Every character's code point is `<= 0xFF` (a necessary — not sufficient — trait of
///    double-encoded text: genuine multi-byte UTF-8 content, e.g. real CJK text, decodes to code
///    points far above this range and could never satisfy this by chance).
/// 2. Re-packing those code points as raw bytes and decoding *that* as UTF-8 succeeds (produces
///    valid UTF-8) — a string that fails this step was never double-encoded to begin with.
/// 3. The repaired string contains at least one character `> 0x7F` — rules out plain ASCII input
///    (e.g. an ordinary English title), which trivially satisfies steps 1-2 without actually
///    being mojibake (re-decoding pure ASCII bytes as UTF-8 just yields the same ASCII string
///    back, not a "repair").
///
/// Returns `None` (caller keeps the original string, unchanged) if any step fails.
pub fn repair_mojibake(s: &str) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let code_points: Vec<u32> = s.chars().map(|c| c as u32).collect();
    if !code_points.iter().all(|&cp| cp <= 0xFF) {
        return None;
    }
    let bytes: Vec<u8> = code_points.iter().map(|&cp| cp as u8).collect();
    let repaired = String::from_utf8(bytes).ok()?;
    if repaired.chars().any(|c| (c as u32) > 0x7F) {
        Some(repaired)
    } else {
        None
    }
}

/// A real archive/category/tankoubon/stamp id is a short, single-line, plain-text string — a real
/// LANraragi export's own id generators (content-hash hex, `SET_<timestamp>`, `TANK_<timestamp>`,
/// `STAMPS_<page>_<millis>`) never produce anything else. This is deliberately loose (no charset
/// restriction beyond "no control characters") rather than a strict format check tied to any one
/// generator's exact shape — the goal is only to reject something that could corrupt a Redis key
/// or blow up a `HashMap` this module builds from every record, not to validate that an id was
/// really generated by LANraragi's own code.
const MAX_ID_LEN: usize = 256;

fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && !id.chars().any(|c| c.is_control())
}

/// Maps a lowercased tag (the de-duplication key — `uploader:ChO_Hentai` and `uploader:cho_hentai`
/// are the same tag) to the *original*, case-preserved tag text. Storing only the lowercased form
/// (the previous implementation) silently mangled every mixed-case tag's actual saved value —
/// caught via external code review against a real backup export, 2026-08-29.
fn normalized_tags(tags: &str) -> HashMap<String, String> {
    tags.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .map(|t| (t.to_ascii_lowercase(), t))
        .collect()
}

/// Removes and returns the one `rating:` tag from a normalized tag map (there should only ever be
/// one on either side going into a merge — this app's own UI never writes more than one, and by
/// the time this runs, `convert_legacy_rating_tags` has already collapsed legacy's own star-repeat
/// form to this app's decimal form too), so the caller can decide which side's rating — not both —
/// survives the merge, instead of a plain set union silently keeping every distinct `rating:` tag
/// side by side.
fn extract_rating_tag(tags: &mut HashMap<String, String>) -> Option<String> {
    let key = tags.keys().find(|k| k.starts_with("rating:")).cloned()?;
    tags.remove(&key)
}

/// Rewrites any `rating:` tag from legacy's own star-repeat encoding (`rating:⭐⭐⭐`, N repetitions
/// of the star emoji, whole numbers only — see `~/LANraragi/public/js/reader.js`'s Raty widget)
/// into this app's own decimal encoding (`rating:3`) — same conversion `apps/frontend/src/lib/
/// utils/rating.ts::parseRating` already does on *read*, done here on *write* instead so an
/// imported archive's `rating:` tag is byte-identical to one this app's own UI would have written,
/// not a foreign format this app merely happens to still tolerate reading. Any other tag (not
/// starting with `rating:`) passes through unchanged. A `rating:` tag already in decimal form
/// (`rating:4` or `rating:4.5`) also passes through unchanged — this only rewrites the star form.
fn convert_legacy_rating_tags(tags: &str) -> String {
    tags.split(',')
        .map(|raw| {
            let trimmed = raw.trim();
            let Some(value) = trimmed
                .strip_prefix("rating:")
                .or_else(|| trimmed.strip_prefix("Rating:"))
            else {
                return trimmed.to_string();
            };
            let star_count = value.chars().filter(|&c| c == '⭐').count();
            if star_count > 0 && value.chars().all(|c| c == '⭐') {
                format!("rating:{star_count}")
            } else {
                trimmed.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// If `tags` has a `source:` tag, returns a version containing only its `source:`/`rating:` tags
/// (in their original relative order) — discarding every other tag (artist/language/category/
/// uploader/etc.) — since a `source:` URL is enough to re-derive the rest via a metadata plugin
/// re-scan, and a user opting into this wants the smaller, higher-signal tag set rather than
/// dragging over a foreign library's full (often noisier, namespace-inconsistent) tagging
/// wholesale. If `tags` has no `source:` tag at all (no way to re-derive anything from it later),
/// returns `tags` completely unchanged — minimizing would just be data loss with no recovery
/// path. Only ever applied to the *legacy* side, before merge/overwrite — never to this
/// instance's own existing tags.
fn minimize_legacy_tags(tags: &str) -> String {
    let has_source = tags
        .split(',')
        .any(|t| t.trim().to_ascii_lowercase().starts_with("source:"));
    if !has_source {
        return tags.to_string();
    }
    tags.split(',')
        .map(str::trim)
        .filter(|t| {
            let lower = t.to_ascii_lowercase();
            lower.starts_with("source:") || lower.starts_with("rating:")
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Reconciles a parsed [`LegacyBackupDocument`] against archives already on this instance and
/// writes the result — see this module's own top-of-file docs for why this doesn't delegate to
/// [`crate::restore::restore`].
///
/// `minimize_tags`: when true, applies [`minimize_legacy_tags`] to every legacy archive's `tags`
/// (after mojibake repair and rating conversion, before either `Overwrite` replaces this
/// instance's tags outright or `Merge` unions them in) — has no effect under
/// [`ImportConflictMode::Skip`], which never reads a legacy archive's `tags` at all.
///
/// Returns, alongside the usual [`ImportLegacySummary`], a [`BackupDocument`] snapshot of every
/// record this call is *about to overwrite*, captured immediately before each write — a
/// Time-Machine-style rollback point the caller (`queue_import_legacy`) persists and the frontend
/// surfaces for one-click download/delete, so a bad import can be undone by feeding this same
/// document straight back into the existing `POST /database/restore` endpoint (`restore()` is
/// already a pure differential apply — see that module's own docs — so a snapshot containing only
/// the handful of records this import actually touches restores cleanly with zero risk to
/// anything this import never touched). Deliberately *pre*-write values only: a record this call
/// creates fresh (no prior version on this instance at all) has nothing to roll back to and is
/// correctly absent from the snapshot — `restore()`'s own differential-apply semantics never
/// delete a record anyway, so there would be no way to express "undo a creation" through it even
/// if this function tried to.
///
/// This whole operation is **not** transactional — it issues many independent Redis writes across
/// four different repositories, with no MULTI/EXEC wrapping them (Redis transactions don't support
/// the read-then-conditionally-write logic this function needs anyway — see Pass 5's own docs on
/// matching-then-writing). If a write fails partway through (`Err` case below), every write that
/// already happened stays applied — this function does not attempt to undo them. What it *does* do
/// is return the [`ImportLegacySummary`]/[`BackupDocument`] accumulated up to the failure point
/// (not just discard them the way a bare `?` would), so the caller can still surface "N records
/// were written before this failed" to the user and the partial snapshot is still available as a
/// rollback point for whatever *did* get written, rather than silently losing that information to
/// an early return.
pub async fn import_from_legacy(
    mut doc: LegacyBackupDocument,
    on_existing: ImportConflictMode,
    minimize_tags: bool,
    archives: &ArchiveRepository,
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
    stamps: &StampRepository,
) -> Result<
    (ImportLegacySummary, BackupDocument),
    (RepositoryError, ImportLegacySummary, BackupDocument),
> {
    let mut summary = ImportLegacySummary::default();
    let mut snapshot = BackupDocument {
        archives: Vec::new(),
        categories: Vec::new(),
        tankoubons: Vec::new(),
        stamps: Vec::new(),
        bookmarks: Vec::new(),
    };

    match import_from_legacy_inner(
        &mut doc,
        on_existing,
        minimize_tags,
        archives,
        categories,
        groupings,
        stamps,
        &mut summary,
        &mut snapshot,
    )
    .await
    {
        Ok(()) => Ok((summary, snapshot)),
        Err(e) => Err((e, summary, snapshot)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn import_from_legacy_inner(
    doc: &mut LegacyBackupDocument,
    on_existing: ImportConflictMode,
    minimize_tags: bool,
    archives: &ArchiveRepository,
    categories: &CategoryRepository,
    groupings: &GroupingRepository,
    stamps: &StampRepository,
    summary: &mut ImportLegacySummary,
    snapshot: &mut BackupDocument,
) -> Result<(), RepositoryError> {
    // Pass -1: drop any record whose own id-shaped field (`arcid`/`catid`/`tankid`/`stamp_id`,
    // plus a category/tankoubon's own `archives` references and a stamp's `archive_id` — every
    // string this module later hands to a Redis key or uses as a `HashMap` lookup key) isn't a
    // plausible Redis-key-safe string. A real LANraragi export never produces any of these
    // malformed (ids come from its own `compute_id`/random-suffix generators, never free text),
    // but this is untrusted file input a user uploads — a hand-edited or corrupted JSON file
    // could otherwise smuggle a newline/NUL byte into a Redis key, or an implausibly large string
    // into a `HashMap` this function builds from every record up front.
    doc.archives.retain(|a| is_valid_id(&a.arcid));
    // A malformed *reference* inside `archives` (e.g. one bad entry in an otherwise fine
    // category's member list) only drops that one reference, not the whole category/tankoubon —
    // same "exclude the specific bad thing, keep the rest" posture Pass 2's own ambiguous-match
    // handling already takes, rather than discarding an entire record over one bad reference.
    doc.categories.retain_mut(|c| {
        c.archives.retain(|a| is_valid_id(a));
        is_valid_id(&c.catid)
    });
    doc.tankoubons.retain_mut(|t| {
        t.archives.retain(|a| is_valid_id(a));
        is_valid_id(&t.tankid)
    });
    doc.stamps
        .retain(|s| is_valid_id(&s.stamp_id) && is_valid_id(&s.archive_id));

    // Pass 0: normalize a `null` JSON value on any of these four fields to an empty string —
    // `#[serde(default)]` on an `Option<String>` field only fires for an *absent* key, not an
    // explicit `null` (a real LANraragi export can and does contain both — see `LegacyArchive`'s
    // own docs). Doing this once up front means every pass below can keep working with plain
    // `String`s exactly as before, rather than threading `Option` through the rest of this
    // function's logic.
    for archive in &mut doc.archives {
        if archive.title.is_none() {
            archive.title = Some(String::new());
        }
        if archive.tags.is_none() {
            archive.tags = Some(String::new());
        }
        if archive.summary.is_none() {
            archive.summary = Some(String::new());
        }
        if archive.filename.is_none() {
            archive.filename = Some(String::new());
        }
    }

    // Pass 1: mojibake repair, per-field, per-record — never a whole-document decision (see
    // `repair_mojibake`'s own docs on why this must be conservative).
    for archive in &mut doc.archives {
        let title = archive.title.get_or_insert_with(String::new);
        if let Some(repaired) = repair_mojibake(title) {
            *title = repaired;
            summary.titles_mojibake_repaired += 1;
        }
        let filename = archive.filename.get_or_insert_with(String::new);
        if let Some(repaired) = repair_mojibake(filename) {
            *filename = repaired;
            summary.titles_mojibake_repaired += 1;
        }
        // A real LANraragi export's `tags` string can itself be mojibake-corrupted the same way
        // `title`/`filename` are (confirmed live against a real backup export, 2026-08-29) — most
        // visibly in a `rating:⭐⭐⭐` tag's star characters, which `convert_legacy_rating_tags`
        // below can only recognize once they're valid UTF-8 again, so this repair must run first.
        let tags = archive.tags.get_or_insert_with(String::new);
        if let Some(repaired) = repair_mojibake(tags) {
            *tags = repaired;
            summary.titles_mojibake_repaired += 1;
        }
        let tags = archive.tags_or_empty();
        let tags = convert_legacy_rating_tags(&tags);
        let tags = if minimize_tags {
            minimize_legacy_tags(&tags)
        } else {
            tags
        };
        archive.tags = Some(tags);
    }
    // Categories/tankoubons carry their own free-text `name` (and a tankoubon's own `tags`/
    // `summary`, mirroring an archive's own fields) that can be mojibake-corrupted exactly the
    // same way an archive's `title` can — confirmed live against a real backup export,
    // 2026-08-29, alongside the `tags`-field fix above. Root cause traced to legacy's own export
    // code: `~/LANraragi/lib/LANraragi/Model/Backup.pm::build_backup_JSON` calls `redis_decode`
    // (`Utils/Redis.pm` — a deliberate *double* `decode_utf8`, its own comment calls it the "Final
    // Solution to the Unicode glitches" for data that got double-encoded on write) on an archive's
    // `name`/`title`/`tags`/`summary` and a category's `name`/`search`, but never once calls it on
    // a tankoubon's `name`/`summary`/`tags` at all (compare that function's category-backup loop,
    // which does call it, against its tankoubon-backup loop just below, which doesn't) — so a
    // tankoubon's own fields are *structurally* more likely to reach this import already
    // mojibake-corrupted than an archive's are, not just possibly so.
    for legacy_category in &mut doc.categories {
        if let Some(repaired) = repair_mojibake(&legacy_category.name) {
            legacy_category.name = repaired;
            summary.titles_mojibake_repaired += 1;
        }
    }
    for legacy_tank in &mut doc.tankoubons {
        if let Some(repaired) = repair_mojibake(&legacy_tank.name) {
            legacy_tank.name = repaired;
            summary.titles_mojibake_repaired += 1;
        }
        if let Some(repaired) = repair_mojibake(&legacy_tank.tags) {
            legacy_tank.tags = repaired;
            summary.titles_mojibake_repaired += 1;
        }
        if let Some(repaired) = repair_mojibake(&legacy_tank.summary) {
            legacy_tank.summary = repaired;
            summary.titles_mojibake_repaired += 1;
        }
    }
    // A stamp's own `content` (often a single emoji character) *does* go through legacy's
    // `redis_decode` on export (`Model/Backup.pm`'s stamp-backup loop calls it, unlike the
    // tankoubon loop above) — but `redis_decode`'s own double-`decode_utf8` is a best-effort
    // repair, not a guarantee (the same reason a real archive `title` was still observed
    // mojibake-corrupted in a real export despite that field also going through it), so this is
    // still checked rather than assumed clean.
    for legacy_stamp in &mut doc.stamps {
        if let Some(repaired) = repair_mojibake(&legacy_stamp.content) {
            legacy_stamp.content = repaired;
            summary.titles_mojibake_repaired += 1;
        }
    }

    // Pass 2: resolve every legacy archive to its id on *this* instance, by basename only.
    // `arcid` is deliberately never compared — a legacy Perl LANraragi's own content-hash id
    // algorithm and this app's default `size_aware_id` (content hash + file size, see
    // `lanrurugi-storage::id`) produce *different* strings for the same file even when both sides
    // independently scanned it, so an `arcid` match here would either almost never fire, or (worse)
    // coincidentally collide with an unrelated archive that happens to share a 40-char hex string —
    // basename is the only field with actual cross-library meaning (confirmed live via external
    // code review, 2026-08-29).
    let mut id_map: HashMap<String, String> = HashMap::new();

    for legacy_archive in &doc.archives {
        let matches = archives
            .find_all_by_filename(&legacy_archive.filename_or_empty())
            .await?;
        match matches.len() {
            0 => summary.archives_skipped_no_match += 1,
            1 => {
                let matched_id = matches[0].id.as_str().to_string();
                id_map.insert(legacy_archive.arcid.clone(), matched_id);
            }
            _ => summary.archives_ambiguous_match += 1,
        }
    }
    // Detect multiple *distinct* legacy records collapsing onto the same target — purely
    // informational (see `archives_multiple_legacy_records_same_target`'s own docs), doesn't
    // change what gets written.
    {
        let mut target_counts: HashMap<&str, usize> = HashMap::new();
        for target in id_map.values() {
            *target_counts.entry(target.as_str()).or_insert(0) += 1;
        }
        summary.archives_multiple_legacy_records_same_target =
            target_counts.values().filter(|&&count| count > 1).sum();
    }

    // Pass 3: apply `on_existing` to matched archives, filter unmatched ones out entirely.
    let mut archives_to_write: Vec<(ArchiveId, String, String, String, Option<String>)> =
        Vec::new();
    for legacy_archive in &doc.archives {
        let Some(new_id) = id_map.get(&legacy_archive.arcid).cloned() else {
            continue; // no match, or ambiguous — excluded, not guessed
        };
        let target_id = ArchiveId(new_id);
        match on_existing {
            ImportConflictMode::Overwrite => {
                archives_to_write.push((
                    target_id,
                    legacy_archive.title_or_empty(),
                    legacy_archive.tags_or_empty(),
                    legacy_archive.summary_or_empty(),
                    legacy_archive.thumbhash.clone(),
                ));
            }
            ImportConflictMode::Merge => {
                if let Some(current) = archives.get(&target_id).await? {
                    // `rating:` is handled outside the ordinary tag-union path below —
                    // a plain set union would otherwise keep *both* sides' `rating:` tags
                    // side by side (e.g. `rating:4,rating:3`), which is a contradiction, not
                    // useful extra information the way two different `artist:` tags can be
                    // (reported live, 2026-08-29). This instance's own rating wins when it has
                    // one at all — a user opting into "merge" is trusting this library's own
                    // metadata over the incoming one for anything already rated here.
                    let mut current_tags = normalized_tags(&current.tags);
                    let mut legacy_tags = normalized_tags(&legacy_archive.tags_or_empty());
                    let current_rating = extract_rating_tag(&mut current_tags);
                    let legacy_rating = extract_rating_tag(&mut legacy_tags);

                    let mut merged = current_tags;
                    merged.extend(legacy_tags);
                    if let Some(rating) = current_rating.or(legacy_rating) {
                        // Unlike an ordinary tag (where preserving the original casing is the
                        // whole point — see `normalized_tags`'s own docs), a `rating:` tag's own
                        // prefix is normalized to lowercase here even if the source had e.g.
                        // `Rating:5` — this app's own UI never writes anything but a lowercase
                        // `rating:` prefix (`convert_legacy_rating_tags`'s own docs), so a
                        // survived-the-merge rating tag should look exactly like one this app
                        // wrote itself, not carry through a foreign side's casing quirk.
                        let value = rating.split_once(':').map(|(_, v)| v).unwrap_or_default();
                        let normalized = format!("rating:{value}");
                        merged.insert(normalized.clone(), normalized);
                    }
                    let mut merged: Vec<String> = merged.into_values().collect();
                    merged.sort();
                    archives_to_write.push((
                        target_id,
                        current.title,
                        merged.join(","),
                        current.summary,
                        current.thumbhash,
                    ));
                }
            }
            ImportConflictMode::Skip => {
                summary.archives_skipped_already_exists += 1;
            }
        }
    }

    for (id, title, tags, summary_text, thumbhash) in archives_to_write {
        if let Some(mut archive) = archives.get(&id).await? {
            snapshot.archives.push(BackupArchive {
                arcid: archive.id.as_str().to_string(),
                title: archive.title.clone(),
                tags: archive.tags.clone(),
                summary: (!archive.summary.is_empty()).then(|| archive.summary.clone()),
                thumbhash: archive.thumbhash.clone(),
                filename: archive.name.clone(),
            });
            archive.title = title;
            archive.tags = tags;
            archive.summary = summary_text;
            archive.thumbhash = thumbhash;
            archives.save(&archive).await?;
            summary.archives_updated += 1;
        }
    }

    // Pass 4: rewrite every archive-id reference using `id_map`, dropping references with no
    // usable mapping (no match, or ambiguous) rather than pointing at a guessed id.
    for category in &mut doc.categories {
        category.archives = category
            .archives
            .iter()
            .filter_map(|old_id| id_map.get(old_id).cloned())
            .collect();
    }
    for tank in &mut doc.tankoubons {
        tank.archives = tank
            .archives
            .iter()
            .filter_map(|old_id| id_map.get(old_id).cloned())
            .collect();
    }
    doc.stamps
        .retain_mut(|stamp| match id_map.get(&stamp.archive_id) {
            Some(new_id) => {
                stamp.archive_id = new_id.clone();
                true
            }
            None => false,
        });

    // Pass 5: write categories/tankoubons/stamps — no `restore()` call, this module implements
    // its own persistence (see top-of-file docs). Matched by *name* (category/tankoubon) or
    // *(mapped archive, position)* (stamp), never by the legacy record's own id — same reasoning
    // as Pass 2's archive matching: a legacy `catid`/`tankid`/`stamp_id` has no meaningful
    // correspondence to this instance's own independently-generated ids (confirmed live via
    // external code review, 2026-08-29). Each write is preceded by a pre-write snapshot read for
    // whatever this call is about to overwrite, via `crate::build`'s own `to_backup_*` converters
    // (`pub(crate)`) rather than hand-duplicating the field mapping here.
    let existing_categories = categories.list_all().await?;
    let category_by_name: HashMap<String, Category> = existing_categories
        .into_iter()
        .map(|c| (c.name.clone(), c))
        .collect();
    let mut next_catid_attempt = unix_seconds();
    for legacy_category in &doc.categories {
        let existing = category_by_name.get(&legacy_category.name).cloned();
        if existing.is_some() && on_existing == ImportConflictMode::Skip {
            summary.categories_skipped_already_exists += 1;
            continue;
        }
        let (catid, pinned, visible_to_guest) = match &existing {
            Some(existing) => {
                snapshot
                    .categories
                    .push(crate::build::to_backup_category(existing.clone()));
                (
                    existing.catid.clone(),
                    existing.pinned,
                    existing.visible_to_guest,
                )
            }
            None => {
                let catid = allocate_category_id(categories, &mut next_catid_attempt).await?;
                summary.new_category_ids.push(catid.as_str().to_string());
                (catid, false, false)
            }
        };
        categories
            .save(&Category {
                catid,
                name: legacy_category.name.clone(),
                search: legacy_category.search.clone(),
                archives: legacy_category
                    .archives
                    .iter()
                    .cloned()
                    .map(ArchiveId)
                    .collect(),
                pinned,
                visible_to_guest,
            })
            .await?;
        summary.categories_restored += 1;
    }

    let existing_tankoubons = groupings.list_all().await?;
    let tankoubon_by_name: HashMap<String, Grouping> = existing_tankoubons
        .into_iter()
        .map(|g| (g.name.clone(), g))
        .collect();
    let mut next_tankid_attempt = unix_seconds();
    for legacy_tank in &doc.tankoubons {
        let existing = tankoubon_by_name.get(&legacy_tank.name).cloned();
        if existing.is_some() && on_existing == ImportConflictMode::Skip {
            summary.tankoubons_skipped_already_exists += 1;
            continue;
        }
        let (tankid, progress, thumbnail_manual, thumbnail_source_archive, thumbnail_source_page) =
            match &existing {
                Some(existing) => {
                    snapshot
                        .tankoubons
                        .push(crate::build::to_backup_tankoubon(existing.clone()));
                    (
                        existing.tankid.clone(),
                        existing.progress,
                        existing.thumbnail_manual,
                        existing.thumbnail_source_archive.clone(),
                        existing.thumbnail_source_page,
                    )
                }
                None => {
                    let tankid = allocate_tank_id(groupings, &mut next_tankid_attempt).await?;
                    summary.new_tankoubon_ids.push(tankid.as_str().to_string());
                    (tankid, 0, false, None, None)
                }
            };
        groupings
            .save(&Grouping {
                tankid,
                name: legacy_tank.name.clone(),
                summary: legacy_tank.summary.clone(),
                tags: legacy_tank.tags.clone(),
                progress,
                archives: legacy_tank
                    .archives
                    .iter()
                    .cloned()
                    .map(ArchiveId)
                    .collect(),
                thumbnail_manual,
                thumbnail_source_archive,
                thumbnail_source_page,
                chapter_names: Default::default(),
                created_at: None,
                updated_at: None,
            })
            .await?;
        summary.tankoubons_restored += 1;
    }

    // Stamps are matched by `(mapped archive id, position)` — the same page/spot on the same
    // archive is the same stamp, regardless of what key either side happened to store it under.
    // Unlike the category/tankoubon "one flat list" case above, existing stamps must be looked up
    // per-archive (a global `list_all` would be wasteful and `StampRepository` has no such method
    // anyway — stamps are commonly numerous and archive-scoped).
    let mut stamp_ids_by_archive: HashMap<ArchiveId, Vec<StampId>> = HashMap::new();
    let mut next_stamp_millis = unix_seconds() * 1000;
    for legacy_stamp in &doc.stamps {
        let archive_id = ArchiveId(legacy_stamp.archive_id.clone());
        let Some(archive) = archives.get(&archive_id).await? else {
            continue; // stamp's own archive has no id-map entry left after Pass 4's filter
        };
        let existing_stamp =
            find_stamp_by_position(stamps, &archive.stamp_ids, &legacy_stamp.position).await?;
        if existing_stamp.is_some() && on_existing == ImportConflictMode::Skip {
            summary.stamps_skipped_already_exists += 1;
            if let Some(existing) = &existing_stamp {
                stamp_ids_by_archive
                    .entry(archive_id.clone())
                    .or_default()
                    .push(existing.stamp_id.clone());
            }
            continue;
        }
        let (stamp_id, icon, rect) = match &existing_stamp {
            Some(existing) => {
                snapshot
                    .stamps
                    .push(crate::build::to_backup_stamp(existing.clone()));
                (
                    existing.stamp_id.clone(),
                    existing.icon.clone(),
                    existing.rect.clone(),
                )
            }
            None => {
                let page = page_of(&legacy_stamp.stamp_id).unwrap_or(0);
                let stamp_id = StampId(format!(
                    "STAMPS_{page}_{}",
                    unix_millis_monotonic(&mut next_stamp_millis)
                ));
                summary.new_stamp_ids.push(stamp_id.as_str().to_string());
                (stamp_id, String::new(), String::new())
            }
        };
        stamps
            .restore_raw(&Stamp {
                stamp_id: stamp_id.clone(),
                content: legacy_stamp.content.clone(),
                position: legacy_stamp.position.clone(),
                archive_id: archive_id.clone(),
                icon,
                rect,
            })
            .await?;
        stamp_ids_by_archive
            .entry(archive_id)
            .or_default()
            .push(stamp_id);
        summary.stamps_restored += 1;
    }
    // Merge, never replace outright — `archive.stamp_ids` may carry stamps this import never
    // touched at all (a page this legacy export has no record for), and clobbering the whole list
    // would silently delete them (reported live via external code review, 2026-08-29).
    for (archive_id, imported_stamp_ids) in stamp_ids_by_archive {
        if let Some(mut archive) = archives.get(&archive_id).await? {
            let mut merged = archive.stamp_ids.clone();
            for id in imported_stamp_ids {
                if !merged.contains(&id) {
                    merged.push(id);
                }
            }
            archive.stamp_ids = merged;
            archives.save(&archive).await?;
        }
    }

    Ok(())
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Bumps `*attempt` by 1 on every call so repeated invocations within the same import (which can
/// easily land in the same wall-clock second) never propose the same id twice, without needing to
/// re-read the clock — mirrors `categories.rs::create_category`'s own collision-avoidance loop.
async fn allocate_category_id(
    categories: &CategoryRepository,
    attempt: &mut u64,
) -> Result<CategoryId, RepositoryError> {
    loop {
        let candidate = CategoryId(format!("SET_{attempt}"));
        *attempt += 1;
        if categories.get(&candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
}

async fn allocate_tank_id(
    groupings: &GroupingRepository,
    attempt: &mut u64,
) -> Result<TankId, RepositoryError> {
    loop {
        let candidate = TankId(format!("TANK_{attempt}"));
        *attempt += 1;
        if groupings.get(&candidate).await?.is_none() {
            return Ok(candidate);
        }
    }
}

/// `stamp_id`'s own `STAMPS_<page>_<millis>` shape (see `StampRepository`'s own docs) — parses out
/// just the page component, mirroring `lanrurugi_api::stamps::page_of` (not reused directly: that
/// function is `pub(crate)` to `lanrurugi-api`, and this crate deliberately doesn't depend on it —
/// see this module's own top-of-file docs on staying independent of the live app's own request
/// handlers).
fn page_of(stamp_id: &str) -> Option<u32> {
    stamp_id
        .strip_prefix("STAMPS_")
        .and_then(|rest| rest.split('_').next())
        .and_then(|p| p.parse().ok())
}

/// Finds, among `stamp_ids` (an archive's own current stamp list), the one whose `position`
/// matches `position` exactly — same page/spot, the definition of "the same stamp" used
/// throughout this module's stamp-matching logic.
async fn find_stamp_by_position(
    stamps: &StampRepository,
    stamp_ids: &[StampId],
    position: &str,
) -> Result<Option<Stamp>, RepositoryError> {
    for id in stamp_ids {
        if let Some(stamp) = stamps.get(id).await? {
            if stamp.position == position {
                return Ok(Some(stamp));
            }
        }
    }
    Ok(None)
}

/// Monotonic millisecond counter seeded from the wall clock, incremented on every call — a new
/// stamp's id (`STAMPS_<page>_<millis>`) only needs to be unique, not an accurate timestamp, and a
/// tight per-import loop can easily call this more than once within the same clock millisecond.
fn unix_millis_monotonic(counter: &mut u64) -> u64 {
    *counter += 1;
    *counter
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_core::entities::Archive;
    use std::collections::HashSet;

    /// Builds a real mojibake sample the same way a double-encoding bug actually produces one —
    /// encode as UTF-8 bytes, then reinterpret each byte as a Latin-1 code point (this is exactly
    /// what happened to the real strings observed in a real LANraragi export, e.g. "イジイセ"
    /// becoming "ã\x82¤ã\x82¸..."), rather than hardcoding an escaped literal that would be
    /// harder to verify by eye.
    fn mojibake_of(s: &str) -> String {
        s.as_bytes().iter().map(|&b| b as char).collect()
    }

    #[test]
    fn repair_mojibake_fixes_a_real_double_encoded_japanese_string() {
        let original = "テスト";
        let corrupted = mojibake_of(original);
        assert_eq!(repair_mojibake(&corrupted).as_deref(), Some(original));
    }

    #[test]
    fn repair_mojibake_leaves_plain_ascii_untouched() {
        assert_eq!(repair_mojibake("Sample Title"), None);
        assert_eq!(repair_mojibake("some-site-000000"), None);
    }

    #[test]
    fn repair_mojibake_leaves_healthy_multibyte_utf8_untouched() {
        // A real, un-corrupted CJK string has code points far above 0xFF — must never be
        // "repaired" into garbage.
        assert_eq!(repair_mojibake("テスト"), None);
        assert_eq!(repair_mojibake("中文标题"), None);
    }

    #[test]
    fn repair_mojibake_handles_empty_and_single_char_strings() {
        assert_eq!(repair_mojibake(""), None);
        assert_eq!(repair_mojibake("A"), None);
        assert_eq!(repair_mojibake(&mojibake_of("あ")), Some("あ".to_string()));
    }

    #[test]
    fn convert_legacy_rating_tags_rewrites_star_repeat_to_decimal() {
        assert_eq!(
            convert_legacy_rating_tags("artist:foo,rating:⭐⭐⭐,language:english"),
            "artist:foo,rating:3,language:english"
        );
    }

    #[test]
    fn convert_legacy_rating_tags_leaves_already_decimal_ratings_untouched() {
        assert_eq!(
            convert_legacy_rating_tags("artist:foo,rating:4.5"),
            "artist:foo,rating:4.5"
        );
    }

    #[test]
    fn convert_legacy_rating_tags_leaves_tags_without_a_rating_untouched() {
        assert_eq!(
            convert_legacy_rating_tags("artist:foo,language:english"),
            "artist:foo,language:english"
        );
    }

    #[test]
    fn convert_legacy_rating_tags_handles_empty_string() {
        assert_eq!(convert_legacy_rating_tags(""), "");
    }

    #[test]
    fn minimize_legacy_tags_keeps_only_source_and_rating_when_source_is_present() {
        assert_eq!(
            minimize_legacy_tags(
                "artist:foo,source:example.com/g/123/abc,language:english,rating:4"
            ),
            "source:example.com/g/123/abc,rating:4"
        );
    }

    #[test]
    fn minimize_legacy_tags_leaves_tags_unchanged_when_no_source_tag() {
        assert_eq!(
            minimize_legacy_tags("artist:foo,language:english,rating:4"),
            "artist:foo,language:english,rating:4"
        );
    }

    #[test]
    fn minimize_legacy_tags_keeps_source_alone_when_no_rating_present() {
        assert_eq!(
            minimize_legacy_tags("artist:foo,source:example.com/g/123/abc,language:english"),
            "source:example.com/g/123/abc"
        );
    }

    #[test]
    fn minimize_legacy_tags_handles_empty_string() {
        assert_eq!(minimize_legacy_tags(""), "");
    }

    async fn test_pool() -> Option<deadpool_redis::Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let url = format!("{}/0", base.trim_end_matches('/'));
        lanrurugi_storage::test_support::test_pool_for_url(&url).await
    }

    /// `name` is the extension-less basename `find_all_by_filename` actually matches against
    /// (see `Archive::name`'s own docs) — deliberately a separate parameter from `title`, since
    /// this module's own basename-matching logic depends on it and a real archive's `name` and
    /// `title` are two independent fields that just happen to often start out equal.
    fn test_archive(id: &str, name: &str, title: &str, tags: &str, file: &str) -> Archive {
        Archive {
            id: ArchiveId(id.to_string()),
            name: name.to_string(),
            title: title.to_string(),
            file: file.to_string(),
            tags: tags.to_string(),
            summary: String::new(),
            arcsize: 1,
            pagecount: 1,
            isnew: false,
            lastreadpage: 0,
            lastreadtime: 0,
            thumbhash: None,
            toc: vec![],
            stamp_ids: vec![],
            heal_failed_at: None,
            corrupted_pages: vec![],
            has_patch: false,
        }
    }

    #[tokio::test]
    async fn basename_match_overwrite_mode_replaces_metadata() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        // `arcid` deliberately does *not* match anything on this instance — only basename
        // (`x-overwrite`) does, exercising the matching path this module actually uses (see
        // Pass 2's own docs on why a legacy `arcid` is never compared). Each `basename_match_*`
        // test in this module uses its own distinct basename — `cargo test` runs them in
        // parallel against the same Redis instance, and a basename shared across tests would make
        // `find_all_by_filename` see more than one match, tipping every one of them into the
        // ambiguous-match path instead of the single-match path each is actually testing
        // (confirmed live, 2026-08-29, after switching this module off exact-id matching).
        let id = "1".repeat(40);
        archives
            .save(&test_archive(
                &id,
                "x-overwrite",
                "Current Title",
                "artist:current",
                "/x-overwrite.zip",
            ))
            .await
            .unwrap();

        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: "9".repeat(40),
                title: Some("Legacy Title".into()),
                tags: Some("artist:legacy".into()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("x-overwrite".into()),
            }],
            ..Default::default()
        };

        let (summary, _snapshot) = import_from_legacy(
            doc,
            ImportConflictMode::Overwrite,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 1);
        assert_eq!(summary.archives_skipped_no_match, 0);
        assert_eq!(summary.archives_ambiguous_match, 0);

        let updated = archives.get(&ArchiveId(id.clone())).await.unwrap().unwrap();
        assert_eq!(updated.title, "Legacy Title");
        assert_eq!(updated.tags, "artist:legacy");

        archives.delete(&ArchiveId(id)).await.unwrap();
    }

    /// The rollback [`BackupDocument`] snapshot [`import_from_legacy`] returns alongside its
    /// summary must capture the *pre-write* value of everything this call actually overwrites —
    /// an archive and a category here — and must *not* include a category this same call creates
    /// fresh (nothing existed before it to roll back to; its id instead lands in
    /// `summary.new_category_ids`, the frontend's own signal that a full undo can't remove it).
    /// This is the test that exercises the snapshot-collection code added alongside the rating/
    /// mojibake fixes above; every other test in this module discards its own snapshot as
    /// `_snapshot`.
    #[tokio::test]
    async fn snapshot_captures_pre_write_values_and_skips_freshly_created_records() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        let archive_id = "8".repeat(40);
        archives
            .save(&test_archive(
                &archive_id,
                "snap",
                "Current Title",
                "artist:current",
                "/snap.zip",
            ))
            .await
            .unwrap();

        // Matched by *name*, not `catid` — the existing category's own `catid` is irrelevant to
        // the legacy record's `catid`, deliberately different here to prove that.
        let existing_catid = "SET_8800000001".to_string();
        categories
            .save(&Category {
                catid: CategoryId(existing_catid.clone()),
                name: "Existing Category".to_string(),
                search: None,
                archives: vec![],
                pinned: false,
                visible_to_guest: false,
            })
            .await
            .unwrap();

        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: "9".repeat(40),
                title: Some("Legacy Title".into()),
                tags: Some("artist:legacy".into()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("snap".into()),
            }],
            categories: vec![
                LegacyCategory {
                    catid: "SET_LEGACY_EXISTING".to_string(),
                    name: "Existing Category".to_string(),
                    search: None,
                    archives: vec![archive_id.clone()],
                },
                LegacyCategory {
                    catid: "SET_LEGACY_NEW".to_string(),
                    name: "Brand New Category".to_string(),
                    search: None,
                    archives: vec![],
                },
            ],
            ..Default::default()
        };

        let (summary, snapshot) = import_from_legacy(
            doc,
            ImportConflictMode::Overwrite,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();

        // The archive's pre-write state is captured, not its post-write state.
        assert_eq!(snapshot.archives.len(), 1);
        assert_eq!(snapshot.archives[0].arcid, archive_id);
        assert_eq!(snapshot.archives[0].title, "Current Title");
        assert_eq!(snapshot.archives[0].tags, "artist:current");

        // Only the category that already existed (matched by name) is snapshotted, with its
        // pre-write name preserved — the brand-new one has no prior state and correctly has
        // nothing to roll back to, but its freshly-allocated id is surfaced separately.
        assert_eq!(snapshot.categories.len(), 1);
        assert_eq!(snapshot.categories[0].catid, existing_catid);
        assert_eq!(snapshot.categories[0].name, "Existing Category");
        assert_eq!(summary.new_category_ids.len(), 1);

        assert!(snapshot.tankoubons.is_empty());
        assert!(snapshot.stamps.is_empty());

        archives.delete(&ArchiveId(archive_id)).await.unwrap();
        categories
            .delete(&CategoryId(existing_catid))
            .await
            .unwrap();
        for new_catid in &summary.new_category_ids {
            categories
                .delete(&CategoryId(new_catid.clone()))
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn basename_match_merge_mode_unions_tags_keeps_current_title() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        let id = "2".repeat(40);
        archives
            .save(&test_archive(
                &id,
                "x-merge-union",
                "Current Title",
                "artist:current,Rating:5",
                "/x-merge-union.zip",
            ))
            .await
            .unwrap();

        // Legacy's own rating (3) deliberately differs from this instance's (5) — the whole point
        // of this test is to confirm the merge keeps exactly one `rating:` tag, not both side by
        // side (`rating:5,rating:3` would be a self-contradicting result, reported live,
        // 2026-08-29), and that this instance's own rating is the one that survives.
        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: "9".repeat(40),
                title: Some("Legacy Title".into()),
                tags: Some("artist:legacy,rating:3".into()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("x-merge-union".into()),
            }],
            ..Default::default()
        };

        let (summary, _snapshot) = import_from_legacy(
            doc,
            ImportConflictMode::Merge,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 1);

        let updated = archives.get(&ArchiveId(id.clone())).await.unwrap().unwrap();
        assert_eq!(updated.title, "Current Title");
        let mut tags: Vec<&str> = updated.tags.split(',').collect();
        tags.sort();
        assert_eq!(tags, vec!["artist:current", "artist:legacy", "rating:5"]);

        archives.delete(&ArchiveId(id)).await.unwrap();
    }

    #[tokio::test]
    async fn basename_match_merge_mode_takes_legacy_rating_when_current_has_none() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        let id = "7".repeat(40);
        archives
            .save(&test_archive(
                &id,
                "x-merge-takes-legacy-rating",
                "Current Title",
                "artist:current",
                "/x-merge-takes-legacy-rating.zip",
            ))
            .await
            .unwrap();

        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: "9".repeat(40),
                title: Some("Legacy Title".into()),
                tags: Some("artist:legacy,rating:4".into()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("x-merge-takes-legacy-rating".into()),
            }],
            ..Default::default()
        };

        import_from_legacy(
            doc,
            ImportConflictMode::Merge,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();

        let updated = archives.get(&ArchiveId(id.clone())).await.unwrap().unwrap();
        let mut tags: Vec<&str> = updated.tags.split(',').collect();
        tags.sort();
        assert_eq!(tags, vec!["artist:current", "artist:legacy", "rating:4"]);

        archives.delete(&ArchiveId(id)).await.unwrap();
    }

    #[tokio::test]
    async fn basename_match_skip_mode_leaves_metadata_untouched() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        let id = "3".repeat(40);
        archives
            .save(&test_archive(
                &id,
                "x-skip",
                "Current Title",
                "artist:current",
                "/x-skip.zip",
            ))
            .await
            .unwrap();

        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: "9".repeat(40),
                title: Some("Legacy Title".into()),
                tags: Some("artist:legacy".into()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("x-skip".into()),
            }],
            ..Default::default()
        };

        let (summary, _snapshot) = import_from_legacy(
            doc,
            ImportConflictMode::Skip,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 0);
        assert_eq!(summary.archives_skipped_already_exists, 1);

        let untouched = archives.get(&ArchiveId(id.clone())).await.unwrap().unwrap();
        assert_eq!(untouched.title, "Current Title");
        assert_eq!(untouched.tags, "artist:current");

        archives.delete(&ArchiveId(id)).await.unwrap();
    }

    #[tokio::test]
    async fn unambiguous_basename_match_remaps_id_and_rewrites_category_reference() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        // Simulates an archive this instance already re-keyed via `rebuild-index`: same
        // filename, different id than the legacy record still references.
        let legacy_id = "4".repeat(40);
        let current_id = "4".repeat(39) + "1";
        archives
            .save(&test_archive(
                &current_id,
                "unique_basename",
                "Current Title",
                "",
                "/dir/unique_basename.zip",
            ))
            .await
            .unwrap();

        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: legacy_id.clone(),
                title: Some("Legacy Title".into()),
                tags: Some("artist:legacy".into()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("unique_basename".into()),
            }],
            categories: vec![LegacyCategory {
                catid: "SET_LEGACY_4".to_string(),
                name: "Legacy Cat".to_string(),
                search: None,
                archives: vec![legacy_id.clone()],
            }],
            ..Default::default()
        };

        let (summary, _snapshot) = import_from_legacy(
            doc,
            ImportConflictMode::Overwrite,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 1);
        assert_eq!(summary.archives_ambiguous_match, 0);
        assert_eq!(summary.categories_restored, 1);
        assert_eq!(summary.new_category_ids.len(), 1);

        let updated = archives
            .get(&ArchiveId(current_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.title, "Legacy Title");

        let new_catid = CategoryId(summary.new_category_ids[0].clone());
        let restored_cat = categories.get(&new_catid).await.unwrap().unwrap();
        assert_eq!(restored_cat.name, "Legacy Cat");
        assert_eq!(restored_cat.archives, vec![ArchiveId(current_id.clone())]);

        archives.delete(&ArchiveId(current_id)).await.unwrap();
        categories.delete(&new_catid).await.unwrap();
    }

    #[tokio::test]
    async fn ambiguous_basename_match_is_excluded_not_guessed() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        // Two different archives (different directories) share the same basename.
        let dup_a = "5".repeat(40);
        let dup_b = "5".repeat(39) + "1";
        archives
            .save(&test_archive(&dup_a, "dup", "A", "", "/dir_a/dup.zip"))
            .await
            .unwrap();
        archives
            .save(&test_archive(&dup_b, "dup", "B", "", "/dir_b/dup.zip"))
            .await
            .unwrap();

        let legacy_id = "5".repeat(38) + "22";
        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: legacy_id.clone(),
                title: Some("Legacy Title".into()),
                tags: Some(String::new()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("dup".into()),
            }],
            categories: vec![LegacyCategory {
                catid: "SET_LEGACY_5".to_string(),
                name: "Ambiguous Cat".to_string(),
                search: None,
                archives: vec![legacy_id],
            }],
            ..Default::default()
        };

        let (summary, _snapshot) = import_from_legacy(
            doc,
            ImportConflictMode::Overwrite,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 0);
        assert_eq!(summary.archives_ambiguous_match, 1);

        // Neither candidate's metadata was touched.
        let a = archives
            .get(&ArchiveId(dup_a.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(a.title, "A");
        let b = archives
            .get(&ArchiveId(dup_b.clone()))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(b.title, "B");

        // The category was still restored, but with the ambiguous reference dropped, not guessed.
        assert_eq!(summary.new_category_ids.len(), 1);
        let new_catid = CategoryId(summary.new_category_ids[0].clone());
        let restored_cat = categories.get(&new_catid).await.unwrap().unwrap();
        assert!(restored_cat.archives.is_empty());

        archives.delete(&ArchiveId(dup_a)).await.unwrap();
        archives.delete(&ArchiveId(dup_b)).await.unwrap();
        categories.delete(&new_catid).await.unwrap();
    }

    #[tokio::test]
    async fn no_match_at_all_is_skipped() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        let doc = LegacyBackupDocument {
            archives: vec![LegacyArchive {
                arcid: "6".repeat(40),
                title: Some("Ghost".into()),
                tags: Some(String::new()),
                summary: Some(String::new()),
                thumbhash: None,
                filename: Some("nowhere_on_this_instance".into()),
            }],
            ..Default::default()
        };

        let (summary, _snapshot) = import_from_legacy(
            doc,
            ImportConflictMode::Overwrite,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();
        assert_eq!(summary.archives_updated, 0);
        assert_eq!(summary.archives_skipped_no_match, 1);
    }

    /// A real (small, hand-picked) excerpt of an actual LANraragi backup export — see
    /// `.env.example`'s own comment on `LANRURUGI_TEST_IMPORT_LEGACY_FIXTURE_PATH` for how this
    /// file was built and what real-world defects it was picked to exercise: a `title`/`filename`
    /// double-encoded the same way `repair_mojibake_fixes_a_real_double_encoded_japanese_string`
    /// above tests synthetically, but also a `tags` string whose `rating:⭐⭐⭐` star characters are
    /// *themselves* mojibake-corrupted (only readable as a rating after `tags` itself is repaired
    /// first — this fixture is what surfaced that ordering requirement), and `category`/
    /// `tankoubon` `name` fields with the same corruption `title` has, none of which any purely
    /// hand-written synthetic fixture had exercised before this.
    #[tokio::test]
    async fn import_from_legacy_handles_a_real_lanraragi_backup_excerpt() {
        let Some(fixture_path) = std::env::var("LANRURUGI_TEST_IMPORT_LEGACY_FIXTURE_PATH").ok()
        else {
            eprintln!("skipping: LANRURUGI_TEST_IMPORT_LEGACY_FIXTURE_PATH not set");
            return;
        };
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let archives = ArchiveRepository::new(pool.clone());
        let categories = CategoryRepository::new(pool.clone());
        let groupings = GroupingRepository::new(pool.clone());
        let stamps = StampRepository::new(pool.clone());

        // `cargo test`/`cargo test -p <crate>` both run this test binary with its CWD set to
        // this crate's own manifest directory, NOT the workspace root — a relative `testdata/...`
        // path from `.env.local` (which reads correctly relative to the repo root in every other
        // context) silently fails to resolve here specifically. Anchoring on `CARGO_MANIFEST_DIR`
        // (known at compile time) sidesteps needing to know which invocation shape actually ran —
        // same fix as `tankoubon_grouping.rs`'s own `tankoubon_grouping_real_data_score_matrix`
        // test, which hit this identically.
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let raw = std::fs::read_to_string(workspace_root.join(&fixture_path))
            .unwrap_or_else(|e| panic!("failed to read {fixture_path}: {e}"));
        let doc: LegacyBackupDocument =
            serde_json::from_str(&raw).unwrap_or_else(|e| panic!("failed to parse fixture: {e}"));

        // Register a current-instance archive for every legacy record *except* whichever
        // filename(s) the fixture happens to have more than one record for (never hardcode which
        // filename that is — the fixture is a real, opaque excerpt, expected to change over time;
        // detecting duplicates structurally is what `find_all_by_filename` itself does at runtime
        // too). Those excluded records' whole point is to test the "not present on this instance
        // at all under either of its own ids, only discoverable by filename" path — using
        // `legacy_archive.arcid` directly, mirroring `unambiguous_basename_match_remaps_id_and_
        // rewrites_category_reference`'s own exact-id-not-used, filename-only-match setup, but two
        // *different* current-instance ids sharing one duplicated filename so the match is
        // genuinely ambiguous, not resolvable. A prefix ("cur-") on every other registered id
        // keeps `find_all_by_filename` from ever accidentally matching a legacy record's own
        // `arcid` as if it were this instance's id.
        //
        // Registered under the *repaired* basename, not the fixture's own raw (possibly
        // mojibake-corrupted) one — `import_from_legacy` repairs `filename` in its own Pass 1
        // before ever calling `find_all_by_filename`, so a current-instance archive registered
        // under the pre-repair basename would never actually match once basename became the
        // *only* matching path (this test previously passed only because exact-id matching
        // papered over the mismatch — caught live, 2026-08-29, once that path was removed).
        let mut filename_counts: HashMap<String, usize> = HashMap::new();
        for legacy_archive in &doc.archives {
            let basename = repair_mojibake(&legacy_archive.filename_or_empty())
                .unwrap_or_else(|| legacy_archive.filename_or_empty());
            *filename_counts.entry(basename).or_insert(0) += 1;
        }
        let duplicated_filenames: HashSet<String> = filename_counts
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(filename, _)| filename.clone())
            .collect();
        assert!(
            !duplicated_filenames.is_empty(),
            "fixture must contain at least one filename shared by more than one record, to \
             exercise the ambiguous-match path"
        );
        let expected_ambiguous: usize = duplicated_filenames
            .iter()
            .map(|f| filename_counts[f])
            .sum();

        // `ArchiveRepository::list_all` (which `find_all_by_filename` is built on) only ever
        // enumerates keys matching its own `ARCHIVE_KEY_GLOB` — exactly 40 hex characters, the
        // shape every real archive id actually has. A `"cur-<arcid>"`-prefixed id (this test's
        // previous approach, harmless back when exact-id matching via a direct `GET` was still
        // the primary path) is invisible to `list_all` entirely, so once this module became
        // basename-only (Pass 2's own docs), every archive registered under such an id could
        // never be found by `find_all_by_filename` no matter how correct its `name` was — caught
        // live, 2026-08-29, after switching this module off exact-id matching. Every id below
        // must be a real 40-hex-char string; `test_archive_id` derives one deterministically from
        // whatever distinguishing string the caller already has on hand (a legacy arcid, or an
        // ambiguous-pair index/side) via SHA-1, purely so two different inputs reliably produce
        // two different ids without this test needing its own collision bookkeeping.
        fn test_archive_id(seed: &str) -> String {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            seed.hash(&mut hasher);
            // `DefaultHasher` only yields 16 hex chars (a u64) — repeat to reach the 40 a real
            // archive id always has (`ARCHIVE_KEY_GLOB`'s own exact-length match, see this
            // function's own call site docs), not because 40 has any other significance here.
            format!("{:016x}", hasher.finish()).repeat(3)[..40].to_string()
        }

        let mut registered_ids: HashMap<String, String> = HashMap::new();
        for legacy_archive in &doc.archives {
            let basename = repair_mojibake(&legacy_archive.filename_or_empty())
                .unwrap_or_else(|| legacy_archive.filename_or_empty());
            if duplicated_filenames.contains(&basename) {
                continue;
            }
            let current_id = test_archive_id(&format!("cur-{}", legacy_archive.arcid));
            archives
                .save(&test_archive(
                    &current_id,
                    &basename,
                    "placeholder",
                    "",
                    &format!("/library/{basename}.zip"),
                ))
                .await
                .unwrap();
            registered_ids.insert(legacy_archive.arcid.clone(), current_id);
        }
        // For each duplicated filename, register two distinct current-instance archives that both
        // happen to share it — so the match is ambiguous rather than simply absent.
        let mut ambiguous_ids: Vec<String> = Vec::new();
        for (i, filename) in duplicated_filenames.iter().enumerate() {
            for side in ["a", "b"] {
                let current_id = test_archive_id(&format!("cur-ambiguous-{i}-{side}"));
                archives
                    .save(&test_archive(
                        &current_id,
                        filename,
                        "placeholder",
                        "",
                        &format!("/library/{side}/{filename}.zip"),
                    ))
                    .await
                    .unwrap();
                ambiguous_ids.push(current_id);
            }
        }

        let (summary, _snapshot) = import_from_legacy(
            doc.clone(),
            ImportConflictMode::Overwrite,
            false,
            &archives,
            &categories,
            &groupings,
            &stamps,
        )
        .await
        .unwrap();

        // Every non-ambiguous record matched by filename and got its metadata written.
        assert_eq!(summary.archives_updated, registered_ids.len());
        // Every legacy record sharing one of the fixture's duplicated filenames is excluded
        // rather than guessed.
        assert_eq!(summary.archives_ambiguous_match, expected_ambiguous);
        assert_eq!(summary.archives_skipped_no_match, 0);
        // At least the records with a real mojibake title/filename got repaired (exact count
        // depends on the fixture's own contents, checked as a lower bound rather than hardcoding
        // the fixture's exact composition here, which would make this test brittle to fixture
        // edits).
        assert!(summary.titles_mojibake_repaired > 0);

        // A star-repeat `rating:` tag (itself mojibake-corrupted in this real export — the whole
        // point of this fixture, see this test's own top-of-function docs) came out as this app's
        // own decimal format on the matched current-instance archive — found structurally
        // (whichever fixture record actually has a `rating:` tag with exactly N repeated star
        // characters as its value, *after* repair) rather than by a hardcoded arcid, so this test
        // stays valid if the fixture's own contents are ever refreshed from a new real export.
        // Must search the *repaired* tags, not `doc`'s own raw ones — one real `⭐` mojibake-
        // corrupts into three separate Latin-1-reinterpreted chars (`â`, U+00AD, U+0090), not one,
        // so counting/matching against raw `⭐` characters in the uncorrupted fixture data can
        // never succeed (caught live, 2026-08-29, once this test started actually exercising the
        // basename-matching path enough to reach this assertion at all).
        fn find_legacy_with_star_rating(
            doc: &LegacyBackupDocument,
            star_count: usize,
        ) -> Option<&LegacyArchive> {
            doc.archives.iter().find(|a| {
                let tags = repair_mojibake(&a.tags_or_empty()).unwrap_or_else(|| a.tags_or_empty());
                tags.split(',').any(|t| {
                    t.trim().strip_prefix("rating:").is_some_and(|v| {
                        v.chars().count() == star_count && v.chars().all(|c| c == '⭐')
                    })
                })
            })
        }

        let four_star_legacy = find_legacy_with_star_rating(&doc, 4)
            .expect("fixture must contain at least one archive with a 4-star rating: tag");
        let four_star_current_id = registered_ids
            .get(&four_star_legacy.arcid)
            .expect("4-star archive must have been registered");
        let four_star_updated = archives
            .get(&ArchiveId(four_star_current_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            four_star_updated.tags.split(',').any(|t| t == "rating:4"),
            "expected a rating:4 tag, got: {}",
            four_star_updated.tags
        );
        assert!(
            !four_star_updated.tags.contains('⭐'),
            "star-repeat rating should have been converted, got: {}",
            four_star_updated.tags
        );

        let five_star_legacy = find_legacy_with_star_rating(&doc, 5)
            .expect("fixture must contain at least one archive with a 5-star rating: tag");
        let five_star_current_id = registered_ids
            .get(&five_star_legacy.arcid)
            .expect("5-star archive must have been registered");
        let five_star_updated = archives
            .get(&ArchiveId(five_star_current_id.clone()))
            .await
            .unwrap()
            .unwrap();
        assert!(
            five_star_updated.tags.split(',').any(|t| t == "rating:5"),
            "expected a rating:5 tag, got: {}",
            five_star_updated.tags
        );

        // The mojibake-corrupted title on that same 4-star archive was repaired to real Japanese
        // text (not left as raw Latin-1-reinterpreted garbage).
        assert!(
            !four_star_updated.title.chars().all(|c| (c as u32) <= 0xFF),
            "title should have been repaired to real multi-byte text, got: {}",
            four_star_updated.title
        );

        // Categories/tankoubons are matched by name now (see Pass 5's own docs), so look them up
        // by the *repaired* name the import just wrote, not by the legacy record's own catid/
        // tankid (which this app's own instance never uses as a key).
        async fn find_category_by_name(
            categories: &CategoryRepository,
            name: &str,
        ) -> Option<Category> {
            categories
                .list_all()
                .await
                .ok()?
                .into_iter()
                .find(|c| c.name == name)
        }
        async fn find_tankoubon_by_name(
            groupings: &GroupingRepository,
            name: &str,
        ) -> Option<Grouping> {
            groupings
                .list_all()
                .await
                .ok()?
                .into_iter()
                .find(|g| g.name == name)
        }

        // Every category's own `name` (not just an archive's `title`) that was itself
        // mojibake-corrupted in the fixture came out repaired too.
        for legacy_category in &doc.categories {
            let Some(repaired_name) = repair_mojibake(&legacy_category.name) else {
                continue;
            };
            let restored = find_category_by_name(&categories, &repaired_name)
                .await
                .expect("category referenced by the fixture must have been restored");
            assert!(
                !restored.name.chars().all(|c| (c as u32) <= 0xFF),
                "category name should have been repaired, got: {}",
                restored.name
            );
        }

        // Stronger check than "no longer mojibake-shaped" above: when a companion fixture of
        // known-correct repaired values is available, assert the repair produced the *actual*
        // real title/name, not merely some other still-plausible-looking string a subtly wrong
        // double-decode could in principle also produce. Optional — skipped (not failed) if the
        // env var isn't set, same convention as the primary fixture path itself.
        if let Ok(repaired_path) =
            std::env::var("LANRURUGI_TEST_IMPORT_LEGACY_FIXTURE_REPAIRED_PATH")
        {
            #[derive(serde::Deserialize)]
            struct RepairedExpectations {
                archive_titles: HashMap<String, String>,
                category_names: HashMap<String, String>,
                tankoubon_names: HashMap<String, String>,
            }
            let raw = std::fs::read_to_string(workspace_root.join(&repaired_path))
                .unwrap_or_else(|e| panic!("failed to read {repaired_path}: {e}"));
            let expected: RepairedExpectations = serde_json::from_str(&raw)
                .unwrap_or_else(|e| panic!("failed to parse repaired-expectations fixture: {e}"));
            assert!(
                !expected.archive_titles.is_empty(),
                "repaired-expectations fixture must cover at least one archive title"
            );

            if let Some(expected_title) = expected.archive_titles.get(&four_star_legacy.arcid) {
                assert_eq!(
                    &four_star_updated.title, expected_title,
                    "repaired title must match the known-correct value exactly, not just \
                     \"no longer mojibake-shaped\""
                );
            }

            for legacy_category in &doc.categories {
                let Some(expected_name) = expected.category_names.get(&legacy_category.catid)
                else {
                    continue;
                };
                let restored = find_category_by_name(&categories, expected_name)
                    .await
                    .expect("category referenced by the fixture must have been restored");
                assert_eq!(
                    &restored.name, expected_name,
                    "repaired category name must match the known-correct value exactly"
                );
            }

            for legacy_tank in &doc.tankoubons {
                let Some(expected_name) = expected.tankoubon_names.get(&legacy_tank.tankid) else {
                    continue;
                };
                let restored = find_tankoubon_by_name(&groupings, expected_name)
                    .await
                    .expect("tankoubon referenced by the fixture must have been restored");
                assert_eq!(
                    &restored.name, expected_name,
                    "repaired tankoubon name must match the known-correct value exactly"
                );
            }
        } else {
            eprintln!(
                "note: LANRURUGI_TEST_IMPORT_LEGACY_FIXTURE_REPAIRED_PATH not set — skipping \
                 exact-value repair assertions (only \"no longer mojibake-shaped\" was checked \
                 above)"
            );
        }

        // Cleanup — every registered current-instance archive, plus every ambiguous-pair archive
        // and any category/tankoubon this import created fresh (all of them, in this fixture —
        // none of these names pre-existed on this test's own instance).
        for current_id in registered_ids.values() {
            let _ = archives.delete(&ArchiveId(current_id.clone())).await;
        }
        for ambiguous_id in &ambiguous_ids {
            let _ = archives.delete(&ArchiveId(ambiguous_id.clone())).await;
        }
        for new_catid in &summary.new_category_ids {
            let _ = categories.delete(&CategoryId(new_catid.clone())).await;
        }
        for new_tankid in &summary.new_tankoubon_ids {
            let _ = groupings.delete(&TankId(new_tankid.clone())).await;
        }
    }
}
