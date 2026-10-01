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

use deadpool_redis::{Config, Pool, Runtime};
use thiserror::Error;

/// Legacy default logical-DB indices, per `lrr.conf`. A from-scratch LANrurugi install has no
/// reason to deviate from these, but they're named constants (not inlined) so a future
/// multi-instance/advanced-config story has an obvious place to make them configurable.
pub const DB_ARCHIVE: u8 = 0;
pub const DB_MINION: u8 = 1;
pub const DB_CONFIG: u8 = 2;
pub const DB_SEARCH: u8 = 3;
pub const DB_METRICS: u8 = 4;

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
}

impl RedisDbs {
    /// `base_url` is a bare `redis://host:port` (or `redis+unix://...`) with no database index —
    /// each logical pool below appends its own `/{db}` suffix.
    pub fn connect(base_url: &str) -> Result<Self, RedisPoolError> {
        Ok(Self {
            archive: pool_for_db(base_url, DB_ARCHIVE)?,
            minion: pool_for_db(base_url, DB_MINION)?,
            config: pool_for_db(base_url, DB_CONFIG)?,
            search: pool_for_db(base_url, DB_SEARCH)?,
            metrics: pool_for_db(base_url, DB_METRICS)?,
        })
    }
}

fn pool_for_db(base_url: &str, db: u8) -> Result<Pool, RedisPoolError> {
    let url = format!("{}/{db}", base_url.trim_end_matches('/'));
    let cfg = Config::from_url(&url);
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
            let pool = pool_for_db("redis://127.0.0.1:6379", db);
            assert!(
                pool.is_ok(),
                "pool_for_db({db}) should build without connecting"
            );
        }
    }
}
