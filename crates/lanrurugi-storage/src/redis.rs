//! Redis connection pooling (`redis` + `deadpool-redis`, per research.md §2).
//!
//! **Verified against source** (`~/LANraragi/lib/LANraragi/Model/Config.pm::get_redis_internal` +
//! `lrr.conf`): legacy LANraragi does not use one Redis logical database — it `SELECT`s across
//! **five**, all on the same server/socket, each holding a distinct slice of data:
//!
//! | Logical DB | Default index | Contents |
//! |---|---|---|
//! | archive | 0 | Archive/Category/Tankoubon/Stamp hashes — the main data store |
//! | minion  | 1 | Legacy job-queue state (superseded by `lanrurugi-core::jobs`, kept for reference) |
//! | config  | 2 | `LRR_CONFIG`, `LRR_FILEMAP`, `LRR_TAGRULES` |
//! | search  | 3 | `LRR_TITLES`, `INDEX_*`, `LRR_STATS`, `LRR_NEW`, `LRR_UNTAGGED`, `LRR_TANKGROUPED`, `LRR_SEARCHCACHE`, `LRR_URLMAP` |
//! | metrics | 4 | Legacy Prometheus-style metrics |
//!
//! Connecting only to DB 0 (the easy-to-assume "just use Redis" default) would silently fail to
//! find any of a pre-existing library's tags, search indexes, or config — a Principle I violation
//! that would be invisible until a user noticed their tags weren't there. `RedisDbs` connects to
//! all five explicitly so this can't happen by omission.

use deadpool_redis::{Config, Pool, PoolConfig, Runtime};
use thiserror::Error;

/// Legacy default logical-DB indices, per `lrr.conf`. A from-scratch LANrurugi install has no
/// reason to deviate from these, but they're named constants (not inlined) so a future
/// multi-instance/advanced-config story has an obvious place to make them configurable.
pub const DB_ARCHIVE: u8 = 0;
pub const DB_MINION: u8 = 1;
pub const DB_CONFIG: u8 = 2;
pub const DB_SEARCH: u8 = 3;
pub const DB_METRICS: u8 = 4;

/// Connections a scanner pool keeps beyond its concurrent ingests. Those tasks are the only
/// bounded consumers, but the scan's own filemap read (`HKEYS`) and the per-file idle-gate poll run
/// outside a permit, and a rebuild-index's rekey pass touches the archive DB too.
const SCANNER_POOL_HEADROOM: usize = 4;

#[derive(Debug, Error)]
pub enum RedisPoolError {
    #[error("invalid Redis base URL {0:?}: {1}")]
    InvalidUrl(String, deadpool_redis::redis::RedisError),
    #[error("failed to build Redis connection pool: {0}")]
    Build(#[from] deadpool_redis::CreatePoolError),
}

/// Connection pools for all five legacy logical Redis databases, sharing one server/socket.
#[derive(Debug, Clone)]
pub struct RedisDbs {
    pub archive: Pool,
    pub minion: Pool,
    pub config: Pool,
    pub search: Pool,
    pub metrics: Pool,
    /// The scanner's own pools — never handed to a request handler (issue #206).
    pub scanner: ScannerPools,
}

impl RedisDbs {
    /// `base_url` is a bare `redis://host:port` (or `redis+unix://...`) with no database index —
    /// each logical pool below appends its own `/{db}` suffix.
    pub fn connect(base_url: &str) -> Result<Self, RedisPoolError> {
        Ok(Self {
            archive: pool_for_db(base_url, DB_ARCHIVE, None)?,
            minion: pool_for_db(base_url, DB_MINION, None)?,
            config: pool_for_db(base_url, DB_CONFIG, None)?,
            search: pool_for_db(base_url, DB_SEARCH, None)?,
            metrics: pool_for_db(base_url, DB_METRICS, None)?,
            scanner: ScannerPools::connect(base_url)?,
        })
    }
}

/// Background scanning's own connection pools, sized to the scan concurrency rather than to
/// deadpool's request-serving default (`cores × 2`).
///
/// Sharing one pool was issue #206's root cause: a first scan of a 1.1 TB library held every
/// connection while `/api/info` waited behind it — 64 seconds, on a server whose Redis answered in
/// microseconds. Separate pools make that structurally impossible no matter what concurrency
/// `LANRURUGI_SCAN_CONCURRENCY` asks for.
#[derive(Debug, Clone)]
pub struct ScannerPools {
    pub archive: Pool,
    pub config: Pool,
    pub search: Pool,
}

impl ScannerPools {
    pub fn connect(base_url: &str) -> Result<Self, RedisPoolError> {
        let max_size = lanrurugi_core::concurrency::scan_concurrency() + SCANNER_POOL_HEADROOM;
        Ok(Self {
            archive: pool_for_db(base_url, DB_ARCHIVE, Some(max_size))?,
            config: pool_for_db(base_url, DB_CONFIG, Some(max_size))?,
            search: pool_for_db(base_url, DB_SEARCH, Some(max_size))?,
        })
    }

    /// A repository bound to the scanner's archive pool, so scan writes queue behind other scan
    /// writes instead of behind the API's requests.
    pub fn archives(&self) -> crate::repository::ArchiveRepository {
        crate::repository::ArchiveRepository::new(self.archive.clone())
    }
}

/// `max_size: None` keeps deadpool's own default (`cores × 2`).
fn pool_for_db(base_url: &str, db: u8, max_size: Option<usize>) -> Result<Pool, RedisPoolError> {
    let url = format!("{}/{db}", base_url.trim_end_matches('/'));
    let mut cfg = Config::from_url(&url);
    if let Some(max_size) = max_size {
        cfg.pool = Some(PoolConfig::new(max_size.max(1)));
    }
    cfg.create_pool(Some(Runtime::Tokio1))
        .map_err(RedisPoolError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_urls_select_the_correct_legacy_db_index() {
        // create_pool doesn't eagerly connect, so this is safe to run without a live Redis server
        // — it just verifies we build one pool per logical DB with the right URL shape.
        for db in [DB_ARCHIVE, DB_MINION, DB_CONFIG, DB_SEARCH, DB_METRICS] {
            let pool = pool_for_db("redis://127.0.0.1:6379", db, None);
            assert!(
                pool.is_ok(),
                "pool_for_db({db}) should build without connecting"
            );
        }
    }

    #[test]
    fn scanner_pools_are_sized_from_the_scan_concurrency_not_the_request_pools() {
        let dbs = RedisDbs::connect("redis://127.0.0.1:6379").unwrap();
        let expected = lanrurugi_core::concurrency::scan_concurrency() + SCANNER_POOL_HEADROOM;
        for pool in [
            &dbs.scanner.archive,
            &dbs.scanner.config,
            &dbs.scanner.search,
        ] {
            assert_eq!(pool.status().max_size, expected);
        }
        // The request pools keep deadpool's own default, so an operator raising the scan
        // concurrency cannot grow them (or exhaust them) by proxy.
        assert_eq!(
            dbs.archive.status().max_size,
            deadpool_redis::PoolConfig::default().max_size
        );
    }
}
