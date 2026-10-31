//! Redis key names shared across crates (constitution Principle I: these must match legacy's own
//! key names exactly, since this Redis instance is reused as-is from the legacy deployment).
//! Each was previously a locally-duplicated `const`/bare string literal in several call sites
//! across `lanrurugi-api`/`lanrurugi-scanner`/`lanrurugi-search`; centralized here so the name
//! only needs to change in one place.

/// Legacy's global settings hash (`~/LANraragi/lib/LANraragi/Model/Config.pm::get_redis_conf`).
pub const CONFIG_KEY: &str = "LRR_CONFIG";

/// Maps a destination on-disk path to the archive ID that owns it — used to detect a
/// still-being-written file during ingestion (`~/LANraragi/lib/LANraragi/Shinobu.pm`'s own
/// filemap hash of the same name).
pub const FILEMAP_KEY: &str = "LRR_FILEMAP";

/// Running total of pages read across the whole library (`Controller/Api/Archive.pm`'s own
/// `incr("LRR_TOTALPAGESTAT")`), surfaced verbatim on `GET /info`.
pub const TOTAL_PAGES_STAT_KEY: &str = "LRR_TOTALPAGESTAT";

/// Additive, LANrurugi-only (no legacy equivalent — `LRR_`-prefixed keys above are all real
/// legacy names this Redis instance is reused as-is against; this one deliberately isn't, to
/// avoid ever colliding with a real legacy key a future upstream version might introduce).
/// Journal hash for `archives.rs::rename_archive`'s in-flight intent: `archive_id -> "old_path\n
/// new_path"`, written *before* the on-disk `fs::rename` and cleared only after the archive
/// record itself is durably saved with the new path — so a crash between those two steps leaves
/// an entry here that `main.rs::repair_zombie_archives` (or a rename-specific startup sweep) can
/// use to finish/verify the rename precisely, rather than guessing from `archive.name` alone
/// (which is itself part of what may not have been saved yet).
pub const PENDING_RENAME_KEY: &str = "LANRURUGI_PENDING_RENAME";

/// Additive, LANrurugi-only. Reverse index (a Redis Set) from one archive ID to every static
/// category it's currently a member of — `SET_<id>` members are the *forward* direction (a
/// category's own `archives` field), and until this index existed, answering "which categories is
/// this one archive in" (`GET /archives/{id}/categories`) meant `CategoryRepository::list_all()`
/// scanning and linearly `.contains()`-checking every category in the library (issue #67).
/// Maintained by `CategoryRepository::save`/`delete` themselves (diffed against the previous
/// membership on every save, so a caller updating a category's `archives` field never has to
/// remember to touch this index separately) — dynamic categories (a `search` predicate, no
/// `archives` list of their own) are never indexed here, matching `get_archive_categories`'s own
/// existing `!c.is_dynamic()` filter.
pub fn archive_categories_key(archive_id: &str) -> String {
    format!("LANRURUGI_ARCHIVE_CATEGORIES_{archive_id}")
}

/// Additive, LANrurugi-only. Same reverse-index reasoning as [`archive_categories_key`], but for
/// Tankoubon (`TANK_*`) membership — maintained by `GroupingRepository::save`/`delete`.
pub fn archive_tankoubons_key(archive_id: &str) -> String {
    format!("LANRURUGI_ARCHIVE_TANKOUBONS_{archive_id}")
}

/// Additive, LANrurugi-only. Per-day page-read counter, one key per UTC calendar day
/// (`LANRURUGI_READ_DAILY:2026-10-14` → pages read that day), written alongside legacy's
/// cumulative [`TOTAL_PAGES_STAT_KEY`] in `archives.rs::update_progress`.
///
/// Legacy has no time dimension on reading at all — the only per-archive timestamp is
/// `lastreadtime`, which retains just the *most recent* read, so a day-by-day history cannot be
/// reconstructed from it. Dated keys (rather than one hash) keep retention trivial: each key
/// expires on its own, with no sweep code to run and no unbounded hash to prune.
pub fn read_daily_key(date: &str) -> String {
    format!("LANRURUGI_READ_DAILY:{date}")
}

/// Additive, LANrurugi-only. The first `YYYY-MM-DD` the per-day counter was ever written for —
/// `SETNX`-ed once by `archives.rs::update_progress`, with no TTL of its own so it outlives the
/// per-day keys it describes. Without it the stats page can only say "the counter has data as far
/// back as the window I happened to read", which is exactly the wrong answer once the page lets a
/// reader pick that window themselves.
pub const READ_DAILY_SINCE_KEY: &str = "LANRURUGI_READ_DAILY_SINCE";

/// Additive, LANrurugi-only. Per-feature, per-UTC-day model usage counters (a hash of request,
/// elapsed, token and failure-class counts), written by `lanrurugi_llm::usage::record` for every
/// model call. Dated keys for the same reason as [`read_daily_key`]: retention is each key's TTL.
pub fn llm_usage_daily_key(feature: &str, date: &str) -> String {
    format!("LANRURUGI_LLM_USAGE:{feature}:{date}")
}

/// 40 days: a week window stays fully answerable with margin, and the family never grows unbounded.
pub const LLM_USAGE_DAILY_TTL_SECS: i64 = 40 * 24 * 60 * 60;

/// Additive, LANrurugi-only. One plugin-wizard generation's record of what the model was shown
/// (a list of attempts, `specs/014-ai-harness-hardening/contracts/generation-record.md`).
pub fn generation_record_key(id: &lanrurugi_core::ids::GenerationId) -> String {
    format!("LANRURUGI_GENERATION_RECORD:{id}")
}

/// 30 days: long enough to answer a real bug report, short enough not to become an audit log.
pub const GENERATION_RECORD_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Additive, LANrurugi-only. A cached grouping review (`specs/016-tankoubon-grouping-harness`),
/// keyed by suggestion, reviewer-prompt version and a hash of the members' titles and tags, so an
/// edit or a prompt change simply misses the cache.
pub fn grouping_review_key(
    id: &lanrurugi_core::ids::GroupSuggestionId,
    prompt_version: &str,
    content_hash: &str,
) -> String {
    format!("LANRURUGI_GROUPING_REVIEW:{id}:{prompt_version}:{content_hash}")
}

/// 30 days: re-opening the modal reuses reviews, and a stale review ages out on its own.
pub const GROUPING_REVIEW_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Additive, LANrurugi-only. The last time a suggestion was shown: members, pair evidence, the
/// settings that produced it and its review.
pub fn grouping_suggestion_key(id: &lanrurugi_core::ids::GroupSuggestionId) -> String {
    format!("LANRURUGI_GROUPING_SUGGESTION:{id}")
}

/// 90 days: long enough to explain a label recorded against it.
pub const GROUPING_SUGGESTION_TTL_SECS: i64 = 90 * 24 * 60 * 60;

/// Additive, LANrurugi-only. Pair labels from the user's grouping decisions (a hash, field
/// `<id_a>|<id_b>` sorted). No TTL: they are the evaluation's ground truth; the user deletes them.
pub const GROUPING_LABELS_KEY: &str = "LANRURUGI_GROUPING_LABELS";

/// Additive, LANrurugi-only. Members the reviewer rejected that the user kept (a hash, field
/// `<suggestion_id>|<archive_id>`). No TTL, like the labels.
pub const GROUPING_DISAGREEMENTS_KEY: &str = "LANRURUGI_GROUPING_REVIEWER_DISAGREEMENTS";

/// How long one [`read_daily_key`] survives — long enough for a "past year" reading-history chart,
/// short enough that the key family never grows without bound.
pub const READ_DAILY_TTL_SECS: i64 = 400 * 24 * 60 * 60;

/// Additive, LANrurugi-only. How many progress updates each archive has ever received — i.e. how
/// many page turns that archive has been part of while it was the archive being read. Legacy's
/// `progress` field only ever holds the *current* page, so "which works do I come back to?" (a
/// real preference signal, distinct from progress percentage and from recency) has no other
/// source.
///
/// A hash rather than one key per archive: it is only ever read as a whole for the stats page, and
/// a single `HGETALL` is one round trip regardless of library size. Bounded by archive count, so
/// nothing needs to prune it.
pub const READ_FREQ_STAT_KEY: &str = "LANRURUGI_READ_FREQ";

/// Additive, LANrurugi-only. One archive's own copy of its gallery's revision chain
/// (`specs/007`-era issue #107's `version_history`), as JSON `[{source, posted_at}]`.
///
/// Stored rather than re-fetched because discovering a chain costs one API call *per hop* on
/// E-Hentai (climb to the tip, then walk back to the root), and every sibling revision of the same
/// work would otherwise pay that walk again. With it stored, "is this candidate an older revision of
/// something the library already holds?" is answerable from Redis alone — no requests at all.
///
/// A chain belongs to the *family*, so each member carries its own copy; a lookup that finds the
/// work in any stored chain sees it. A snapshot goes stale as new children appear, which is harmless:
/// the child is itself downloaded once (paying the walk), stored, and supersedes the older copy.
pub fn archive_version_history_key(archive_id: &str) -> String {
    format!("LANRURUGI_ARCHIVE_VERSION_HISTORY_{archive_id}")
}

// Sharing (specs/017-archive-sharing data-model.md). Additive, LANrurugi-only.

/// One share's hash.
pub fn share_key(id: &lanrurugi_core::ids::ShareId) -> String {
    format!("LANRURUGI_SHARE:{id}")
}

/// Zset of every share id, scored by creation time.
pub const SHARES_INDEX_KEY: &str = "LANRURUGI_SHARES";

/// `access_hash` (hex) → share id, for O(1) code lookup.
pub fn share_by_code_key(code_hash: &str) -> String {
    format!("LANRURUGI_SHARE_BY_CODE:{code_hash}")
}

/// Set of share ids that reference one archive or tankoubon.
pub fn share_item_refs_key(item_id: &str) -> String {
    format!("LANRURUGI_SHARE_ITEM_REFS:{item_id}")
}

/// Set of distinct addresses seen by a share with `ip_first_n`.
pub fn share_ips_key(id: &lanrurugi_core::ids::ShareId) -> String {
    format!("LANRURUGI_SHARE_IPS:{id}")
}

/// One recipient claim's hash (24 h sliding TTL).
pub fn share_claim_key(id: &lanrurugi_core::ids::ClaimId) -> String {
    format!("LANRURUGI_SHARE_CLAIM:{id}")
}

/// A share's claim history zset; members are JSON and outlive the claim hashes.
pub fn share_claims_index_key(id: &lanrurugi_core::ids::ShareId) -> String {
    format!("LANRURUGI_SHARE_CLAIMS:{id}")
}

pub const SHARE_CLAIM_TTL_SECS: i64 = 24 * 60 * 60;

/// A browser recipient's token, keyed by the SHA-256 of the token (12 h sliding TTL).
pub fn share_rtoken_key(token_sha256_hex: &str) -> String {
    format!("LANRURUGI_SHARE_RTOKEN:{token_sha256_hex}")
}

pub const SHARE_RTOKEN_TTL_SECS: i64 = 12 * 60 * 60;

/// Helper endpoint ids of one share, scored by last heartbeat.
pub fn share_helpers_key(id: &lanrurugi_core::ids::ShareId) -> String {
    format!("LANRURUGI_SHARE_HELPERS:{id}")
}

/// Where each helper of one share can be dialled and which archives it holds (JSON per endpoint).
pub fn share_helper_info_key(id: &lanrurugi_core::ids::ShareId) -> String {
    format!("LANRURUGI_SHARE_HELPER_INFO:{id}")
}

/// Endpoint ids that made an import claim on one share; only they may register as helpers.
pub fn share_importers_key(id: &lanrurugi_core::ids::ShareId) -> String {
    format!("LANRURUGI_SHARE_IMPORTERS:{id}")
}

/// An archive's BLAKE3 fingerprint record.
pub fn share_blob_key(archive_id: &str) -> String {
    format!("LANRURUGI_SHARE_BLOB:{archive_id}")
}

/// Zset of archive ids awaiting fingerprinting, scored by enqueue time.
pub const SHARE_PREPARE_KEY: &str = "LANRURUGI_SHARE_PREPARE";

/// This instance's iroh secret key.
pub const SHARE_NODE_SECRET_KEY: &str = "LANRURUGI_SHARE_NODE_SECRET";

/// One import of another instance's share (receiving side).
pub fn share_import_key(id: &lanrurugi_core::ids::ShareImportId) -> String {
    format!("LANRURUGI_SHARE_IMPORT:{id}")
}

/// Zset of import ids, scored by creation time.
pub const SHARE_IMPORTS_INDEX_KEY: &str = "LANRURUGI_SHARE_IMPORTS";

/// A knock packet stored by this instance's rendezvous role (2 h TTL).
pub fn share_rdv_key(lookup_pubkey_hex: &str) -> String {
    format!("LANRURUGI_SHARE_RDV:{lookup_pubkey_hex}")
}

pub const SHARE_RDV_TTL_SECS: i64 = 2 * 60 * 60;
