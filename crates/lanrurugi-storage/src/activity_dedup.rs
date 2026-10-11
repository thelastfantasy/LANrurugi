//! A short-TTL Redis "have I already recorded this (actor/caller, device) pair recently"
//! deduplication check, shared by two activity write sites that would otherwise write one
//! [`crate::activity::ActivityEntry`] per request rather than per genuinely-new session:
//!
//! - `guest.access` (`lanrurugi_api::procedure::require_api_key`'s `GuestVisitor` branch) — a
//!   guest's normal browsing (reader page-turns, prefetch, thumbnails) is many requests per
//!   minute; recording one per request would flood the activity log and, since guest mode has no
//!   count-based cap of its own (only the global time-based `retention_secs`), grow Redis
//!   unboundedly under sustained guest traffic.
//! - `session.refresh` (`lanrurugi_api::login::refresh`'s routine, successful rotation branch) —
//!   happens silently every `access_token_lifetime_secs` for as long as a tab stays open, same
//!   "routine background noise" concern `action_types::SESSION_LOGIN`'s own doc comment already
//!   describes for why a *successful* refresh was never recorded at all before this existed.
//!
//! Mechanism: `SET key value NX EX ttl_secs` — the first caller within the window wins the write
//! (`NX` — only set if absent) and gets `true` back ("go ahead and record it"); every other caller
//! within the same `ttl_secs` window gets `false` ("already recorded recently, skip"). Fixed-window,
//! not sliding — a burst that straddles the window boundary can produce two entries close together
//! rather than exactly one per window, which is an acceptable trade for not needing a write on
//! every single request just to slide the expiry (the whole point of this module is avoiding that).

use deadpool_redis::Pool;
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ActivityDedupError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
}

type Result<T> = std::result::Result<T, ActivityDedupError>;

/// A short, fixed-length fingerprint of `(identity, user_agent)` — `identity` is caller-defined
/// (a guest's `client_ip`, or a session/token's own actor key), never the raw values themselves,
/// so a very long real `User-Agent` string doesn't produce an unbounded Redis key. Collisions are
/// harmless here (worst case: two distinct callers share a dedup window a little early), so a
/// truncated SHA-256 is fine — this is not a security boundary.
pub fn fingerprint(identity: &str, user_agent: Option<&str>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identity.as_bytes());
    hasher.update(b"|");
    hasher.update(user_agent.unwrap_or("").as_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(16);
    for b in &digest[..8] {
        use std::fmt::Write;
        write!(s, "{b:02x}").expect("writing to a String cannot fail");
    }
    s
}

fn dedup_key(namespace: &str, fingerprint: &str) -> String {
    format!("LANRURUGI_ACTIVITY_DEDUP_{namespace}_{fingerprint}")
}

#[derive(Clone)]
pub struct ActivityDedupGate {
    pool: Pool,
}

impl ActivityDedupGate {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Returns `true` the first time this `(namespace, fingerprint)` pair is seen within
    /// `ttl_secs`, `false` for every subsequent call within that same window. `namespace`
    /// separates independent dedup windows sharing this one mechanism (`"guest"` /
    /// `"session_refresh"`) so a fingerprint colliding across the two call sites can't suppress an
    /// unrelated one.
    pub async fn should_record(
        &self,
        namespace: &str,
        fingerprint: &str,
        ttl_secs: u64,
    ) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        let key = dedup_key(namespace, fingerprint);
        let set: bool = deadpool_redis::redis::cmd("SET")
            .arg(&key)
            .arg(1)
            .arg("NX")
            .arg("EX")
            .arg(ttl_secs)
            .query_async::<Option<String>>(&mut conn)
            .await
            .map(|reply| reply.is_some())?;
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        crate::test_support::test_pool().await
    }

    #[tokio::test]
    async fn first_call_records_subsequent_calls_within_ttl_do_not() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let gate = ActivityDedupGate::new(pool);
        let fp = fingerprint("192.168.1.1", Some("Mozilla/5.0 Test"));

        assert!(gate.should_record("guest", &fp, 60).await.unwrap());
        assert!(!gate.should_record("guest", &fp, 60).await.unwrap());
        assert!(!gate.should_record("guest", &fp, 60).await.unwrap());
    }

    #[tokio::test]
    async fn different_namespaces_do_not_share_a_dedup_window() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let gate = ActivityDedupGate::new(pool);
        let fp = fingerprint("token:abc", Some("Mozilla/5.0 Test"));

        assert!(gate.should_record("guest", &fp, 60).await.unwrap());
        assert!(gate
            .should_record("session_refresh", &fp, 60)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn different_fingerprints_do_not_collide() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let gate = ActivityDedupGate::new(pool);
        let fp_a = fingerprint("192.168.1.1", Some("Chrome"));
        let fp_b = fingerprint("192.168.1.2", Some("Chrome"));

        assert!(gate.should_record("guest", &fp_a, 60).await.unwrap());
        assert!(gate.should_record("guest", &fp_b, 60).await.unwrap());
    }
}
