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
