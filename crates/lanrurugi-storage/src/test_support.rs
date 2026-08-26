//! Shared `LANRURUGI_TEST_REDIS_URL`-gated test helpers, reused by every crate's own
//! `#[cfg(test)]`/integration-test code that needs a real Redis instance to exercise real I/O.
//!
//! Not `#[cfg(test)]`-gated itself: `cfg(test)` only applies within the crate being compiled, so a
//! downstream crate's own test code (e.g. `lanrurugi-search`'s `#[cfg(test)] mod tests`, or
//! `lanrurugi-server/tests/*.rs` integration tests) can't see a `cfg(test)` item in this crate at
//! all — it would need to be a plain `pub` item to be visible cross-crate under `cargo test`.
//!
//! The one thing every prior copy of this helper got wrong: `deadpool_redis::Config::create_pool`
//! is lazy and never touches the network, so `.ok()` only confirms a `Pool` *object* was built, not
//! that the configured Redis is actually reachable. When `LANRURUGI_TEST_REDIS_URL` points at an
//! unreachable host, the old helpers returned `Some(pool)` anyway and the test proceeded straight
//! into a real `Connection refused` panic instead of the intended "skipping: ..." message. These
//! helpers fix that by issuing a real `PING` before returning `Some`.

use deadpool_redis::{Config, Pool, Runtime};

/// Builds a single-DB pool against `LANRURUGI_TEST_REDIS_URL` and verifies it's actually reachable
/// via a real `PING`. Returns `None` (never panics) if the env var is unset, the URL is malformed,
/// or the server doesn't respond — callers should treat `None` as "skip this test".
pub async fn test_pool() -> Option<Pool> {
    let url = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
    test_pool_for_url(&url).await
}

/// Same as [`test_pool`] but for a caller (e.g. `lanrurugi-backup`) that appends its own `/{db}`
/// suffix to the base URL before calling this.
pub async fn test_pool_for_url(url: &str) -> Option<Pool> {
    let pool = Config::from_url(url)
        .create_pool(Some(Runtime::Tokio1))
        .ok()?;
    ping(&pool).await.then_some(pool)
}

/// Connects to all five legacy logical DBs (mirroring [`crate::redis::RedisDbs::connect`]) and
/// verifies reachability via a `PING` against just the archive DB — one round trip is enough to
/// confirm the underlying server is up; the other four pools share the same server/socket.
pub async fn test_redis_dbs() -> Option<crate::redis::RedisDbs> {
    let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
    let dbs = crate::redis::RedisDbs::connect(&base).ok()?;
    ping(&dbs.archive).await.then_some(dbs)
}

async fn ping(pool: &Pool) -> bool {
    let Ok(mut conn) = pool.get().await else {
        return false;
    };
    deadpool_redis::redis::cmd("PING")
        .query_async::<String>(&mut conn)
        .await
        .is_ok()
}
