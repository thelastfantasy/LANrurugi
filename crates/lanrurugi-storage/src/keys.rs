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
