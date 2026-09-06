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

/// Cross-process test serialization for a shared `LRR_CONFIG` field (or any other real Redis
/// state multiple integration-test *files* independently read/write). A plain `tokio::sync::Mutex`
/// (the pattern `auth_flow.rs::redis_state_lock`/`serve_index.rs::theme_field_lock`/
/// `settings_toggles.rs::config_field_lock` each independently reinvented) only serializes tests
/// *within the same test binary* — `cargo test --workspace` compiles every `tests/*.rs` file in a
/// crate into its own separate binary and runs them as separate OS processes, in parallel, all
/// against the exact same real `LANRURUGI_TEST_REDIS_URL` instance. Two such binaries racing to
/// write the same `guestmode` field (one in `auth_flow.rs`, one in `settings_toggles.rs`) is
/// invisible to either one's own in-process `Mutex` — confirmed live via a real CI failure,
/// 2026-08-27: `auth_flow.rs`'s own guest-mode test intermittently read back a `guestmode` value a
/// *different* test binary had just written concurrently, mid-test, and asserted on stale state.
///
/// `key` should name the specific shared resource (e.g. `"guestmode"`), not be a single
/// global lock for everything — two unrelated fields being tested by two different process pairs
/// should still run concurrently.
///
/// **Call [`Self::release`] explicitly before the end of every test** — do not rely on `Drop`
/// alone for the normal-completion path. `Drop::drop` here can only `tokio::spawn` a detached
/// `DEL` task (no `.await` available in a sync `drop`), and `#[tokio::test]`'s default
/// current-thread runtime tears down as soon as the test's `async fn` body returns `Ready` —
/// often *before* that spawned task ever gets polled far enough to run its own `.await` points,
/// so the `DEL` silently never executes. Confirmed live via CI log inspection, 2026-08-28: six
/// consecutive `guestmode`-family tests each completed at *exactly* 30-second intervals despite
/// each one's own HTTP round-trips taking milliseconds — the lock was never actually being
/// released between them, only ever expiring on its own `EX` TTL, forcing every single test to
/// wait out the full TTL before it could even start. `Drop` still exists as a last-resort
/// self-heal for a genuinely panicked/killed process (where nothing downstream of the panic point
/// ever runs, `release()` included), not as the primary release path.
pub struct RedisTestLock {
    pool: Pool,
    key: String,
    released: bool,
}

/// How long an unreleased lock self-heals after — deliberately short. With `release()` now the
/// real release path for every normal test completion, this TTL is purely a dead-process safety
/// net (a panicked/killed test that never reaches its own `release()` call), not something a
/// well-behaved test should ever actually wait out — keeping it short (rather than the 300s an
/// earlier, incorrect fix tried) means a genuinely abandoned lock recovers quickly instead of
/// stalling every other process for 5 minutes.
const LOCK_TTL_SECS: usize = 30;
/// How long a blocked acquirer waits before panicking — independent of `LOCK_TTL_SECS` now that
/// release is explicit: ordinary contention resolves as soon as the holder calls `release()`
/// (typically well under a second), so this is purely the "something is genuinely deadlocked"
/// backstop, generous enough to also cover the TTL self-heal path if a prior process really did
/// die mid-critical-section.
const WAIT_TIMEOUT_SECS: u64 = 60;

impl RedisTestLock {
    /// Blocks (async) until the named lock is acquired, waiting up to `WAIT_TIMEOUT_SECS` total
    /// before panicking — a real deadlock (vs. ordinary contention) should fail the test loudly,
    /// not hang CI until its own job-level timeout.
    pub async fn acquire(pool: &Pool, key: &str) -> Self {
        let redis_key = format!("LANRURUGI_TEST_LOCK_{key}");
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(WAIT_TIMEOUT_SECS);
        loop {
            let mut conn = pool
                .get()
                .await
                .expect("test lock: could not get a Redis connection");
            // NX + an expiry (self-healing if a prior test process panicked/was killed while
            // holding the lock, rather than deadlocking every subsequent CI run against a
            // never-released key) — `SET key 1 NX EX <LOCK_TTL_SECS>`.
            let acquired: bool = deadpool_redis::redis::cmd("SET")
                .arg(&redis_key)
                .arg(1)
                .arg("NX")
                .arg("EX")
                .arg(LOCK_TTL_SECS)
                .query_async::<Option<String>>(&mut conn)
                .await
                .expect("test lock: SET NX failed")
                .is_some();
            if acquired {
                return Self {
                    pool: pool.clone(),
                    key: redis_key,
                    released: false,
                };
            }
            if std::time::Instant::now() >= deadline {
                panic!(
                    "test lock \"{key}\" not acquired within {WAIT_TIMEOUT_SECS}s — real deadlock, not just contention"
                );
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Explicitly, synchronously (from the caller's perspective — this `.await`s a real `DEL`
    /// round-trip) releases the lock. Call this before a test's own `async fn` body returns,
    /// rather than relying on `Drop` — see this struct's own docs for why `Drop` alone is
    /// unreliable under `#[tokio::test]`'s current-thread runtime.
    pub async fn release(mut self) {
        if let Ok(mut conn) = self.pool.get().await {
            let _: Result<i64, _> = deadpool_redis::redis::cmd("DEL")
                .arg(&self.key)
                .query_async(&mut conn)
                .await;
        }
        self.released = true;
    }
}

impl Drop for RedisTestLock {
    fn drop(&mut self) {
        // Last-resort self-heal only — see this struct's own docs. A test that already called
        // `release()` sets `released = true`, so this is a no-op double-release guard, not the
        // primary release mechanism. For a test that panics before reaching `release()`, this
        // fire-and-forget spawn is strictly best-effort (may not run at all, per the same
        // current-thread-runtime-teardown reasoning) — the short `LOCK_TTL_SECS` above is what
        // actually guarantees recovery in that case, not this `Drop` impl.
        if self.released {
            return;
        }
        let pool = self.pool.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            if let Ok(mut conn) = pool.get().await {
                let _: Result<i64, _> = deadpool_redis::redis::cmd("DEL")
                    .arg(&key)
                    .query_async(&mut conn)
                    .await;
            }
        });
    }
}
