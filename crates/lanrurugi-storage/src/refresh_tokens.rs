//! Redis persistence for the SPA login flow's refresh tokens (`lanrurugi_core::session` mints the
//! paired stateless JWT access token; this module owns the stateful, revocable half). Additive,
//! LANrurugi-only namespace (`LANRURUGI_`-prefixed, per this crate's own convention — see
//! `keys.rs`), no legacy equivalent (legacy's session cookie has no refresh concept at all).
//!
//! Structural template: [`crate::download_queue`] (same `thiserror` error enum shape, same `Pool`
//! field, same `LANRURUGI_TEST_REDIS_URL`-gated test convention) — but with two deliberate
//! deviations that template doesn't need: **TTL** (`EXPIRE`, since a refresh token's own
//! `expires_at` should also make Redis forget it automatically once it's genuinely spent, unlike
//! a download-queue item, which sticks around until an explicit delete) and **`WATCH`/`MULTI`
//! optimistic-locking transactions** (via `redis::aio::transaction_async` — see [`rotate`]'s own
//! docs for why a plain read-then-write update, `download_queue::update`'s own style, isn't safe
//! here).
//!
//! Only `sha256(secret)` is ever stored server-side, never the raw bearer secret — the same
//! "can't be recovered from a Redis dump/read, only reissued" posture `lanrurugi_core::password`
//! already takes for the login password itself.

use deadpool_redis::redis::{self, AsyncCommands};
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RefreshTokenStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, RefreshTokenStorageError>;

/// A single refresh token's server-side record. The value the browser actually carries in its
/// `lanrurugi_refresh` cookie is `"{token_id}.{secret}"` — `token_id` is this record's own lookup
/// key, `secret` is a per-token random value never stored raw (see this module's own docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefreshTokenRecord {
    pub token_id: String,
    pub secret_hash: String,
    /// Stable across an entire login's rotation chain — reuse detection burns every `token_id`
    /// ever issued under one `family_id` at once (see [`rotate`]'s docs).
    pub family_id: String,
    pub issued_at: i64,
    /// Absolute expiry, anchored to the *original* login, not extended on each rotation — see
    /// [`rotate`]'s own docs for why an actively-rotating chain must still expire.
    pub expires_at: i64,
    /// `true` once this exact token has already been redeemed for a new one — a second
    /// presentation of a `used: true` token is the reuse-detection trigger.
    pub used: bool,
}

/// A newly-issued token pair as returned to a caller — the record persisted server-side, plus the
/// one-time-visible bearer secret needed to construct the `"{token_id}.{secret}"` cookie value.
/// Mirrors [`crate::api_tokens::IssuedApiToken`]'s "raw secret only exists at issuance time"
/// shape.
pub struct IssuedRefreshToken {
    pub record: RefreshTokenRecord,
    pub secret: String,
}

fn token_key(token_id: &str) -> String {
    format!("LANRURUGI_REFRESH_TOKEN_{token_id}")
}

fn family_key(family_id: &str) -> String {
    format!("LANRURUGI_REFRESH_FAMILY_{family_id}")
}

/// 32 random bytes, hex-encoded — used for both `token_id` and `secret` generation (they're
/// unrelated random values, just the same shape).
fn random_hex() -> String {
    use rand::RngExt;
    let bytes: [u8; 32] = std::array::from_fn(|_| rand::rng().random());
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("writing to a String cannot fail");
    }
    s
}

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex_encode(&hasher.finalize())
}

#[derive(Clone)]
pub struct RefreshTokenRepository {
    pool: Pool,
}

impl RefreshTokenRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Mints a brand-new `family_id` and its first token — the *only* place a new family is
    /// created; every other issuance in a login's lifetime goes through [`rotate`] instead. Called
    /// once, from `login.rs`'s `login` handler, on successful password verification.
    pub async fn issue_new_family(
        &self,
        now: i64,
        lifetime_secs: i64,
    ) -> Result<IssuedRefreshToken> {
        let family_id = uuid::Uuid::new_v4().to_string();
        self.issue_in_family(&family_id, now, lifetime_secs).await
    }

    async fn issue_in_family(
        &self,
        family_id: &str,
        now: i64,
        lifetime_secs: i64,
    ) -> Result<IssuedRefreshToken> {
        let token_id = uuid::Uuid::new_v4().to_string();
        let secret = random_hex();
        let record = RefreshTokenRecord {
            token_id: token_id.clone(),
            secret_hash: sha256_hex(&secret),
            family_id: family_id.to_string(),
            issued_at: now,
            expires_at: now + lifetime_secs,
            used: false,
        };
        let mut conn = self.pool.get().await?;
        let key = token_key(&token_id);
        let raw = serde_json::to_string(&record)
            .map_err(|e| RefreshTokenStorageError::Json(key.clone(), e))?;
        let ttl_secs: u64 = lifetime_secs.max(1) as u64;
        let _: () = conn.set_ex(&key, raw, ttl_secs).await?;
        let _: () = conn.sadd(family_key(family_id), &token_id).await?;
        Ok(IssuedRefreshToken { record, secret })
    }

    pub async fn get(&self, token_id: &str) -> Result<Option<RefreshTokenRecord>> {
        let mut conn = self.pool.get().await?;
        let key = token_key(token_id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| RefreshTokenStorageError::Json(key, e)),
        }
    }

    /// Outcome of presenting a refresh token — mirrors the three cases `login.rs`'s `refresh`
    /// handler needs to distinguish (see that handler for the HTTP-status mapping).
    ///
    /// Two racing callers presenting the *same* still-valid token must never both "succeed" —
    /// that would silently mint two children from one single-use parent. `WATCH` alone doesn't
    /// give this for free: it only aborts the `EXEC` if the watched key's value changed between
    /// `WATCH` and `EXEC`, it does NOT re-run arbitrary business logic (the `used` check) against
    /// the fresh value on a retry. So the `used` check itself has to live *inside* the
    /// transaction closure, re-reading the record fresh on every attempt (including retries) —
    /// checking it once outside the closure, then writing the same fixed payload on every retry
    /// regardless of what changed, is exactly the bug this whole function exists to avoid (this
    /// was caught by `concurrent_rotation_of_the_same_token_only_lets_one_succeed`'s own test
    /// failing during development — both racers "won" the first time this was written that way).
    pub async fn rotate(&self, token_id: &str, secret: &str, now: i64) -> Result<RotateOutcome> {
        let Some(record) = self.get(token_id).await? else {
            return Ok(RotateOutcome::NotFound);
        };
        if !lanrurugi_core::crypto::constant_time_eq(&sha256_hex(secret), &record.secret_hash) {
            return Ok(RotateOutcome::NotFound); // wrong secret: treat identically to "no such token"
        }
        if now > record.expires_at {
            return Ok(RotateOutcome::NotFound); // expired — TTL should already have reaped this,
                                                // but a request racing the exact expiry instant
                                                // shouldn't get a different answer than "gone"
        }
        if record.used {
            self.burn_family(&record.family_id).await?;
            return Ok(RotateOutcome::ReuseDetected);
        }

        let conn = self.pool.get().await?;
        let old_key = token_key(token_id);
        let new_token_id = uuid::Uuid::new_v4().to_string();
        let new_secret = random_hex();
        let secret_hash = record.secret_hash.clone();
        let family_id = record.family_id.clone();
        let expires_at = record.expires_at;
        let new_record = RefreshTokenRecord {
            token_id: new_token_id.clone(),
            secret_hash: sha256_hex(&new_secret),
            family_id: family_id.clone(),
            issued_at: now,
            // Absolute expiry inherited from the parent, NOT reset to `now + lifetime` — an
            // actively-rotating chain must still expire on schedule from the original login,
            // otherwise "N-day refresh lifetime" is meaningless (see this record's own field docs).
            expires_at,
            used: false,
        };
        let new_key = token_key(&new_token_id);
        let new_raw = serde_json::to_string(&new_record)
            .map_err(|e| RefreshTokenStorageError::Json(new_key.clone(), e))?;
        let remaining_ttl: u64 = (expires_at - now).max(1) as u64;
        let family_key_str = family_key(&family_id);

        // `transaction_async` requires an owned, `Clone` connection (it clones the connection
        // once per retry internally) — `deadpool_redis::Connection` derefs to
        // `MultiplexedConnection`, which is cheaply `Clone` (shares the same underlying
        // multiplexed channel), so this doesn't open a second real connection.
        let owned_conn: deadpool_redis::redis::aio::MultiplexedConnection = (*conn).clone();
        // `transaction_async`'s own return type `T` must implement `FromRedisValue` (it's parsed
        // straight out of the `EXEC` reply), so the "committed vs. lost the race" business
        // outcome can't itself be `T` — it's threaded out through this `Cell` from inside the
        // closure instead, which the closure can freely mutate across retries without fighting
        // the `FnMut` + `Future`-capturing-`self` borrow constraints a `&mut bool` would hit.
        let lost_race = std::sync::atomic::AtomicBool::new(false);
        let commit_result: std::result::Result<(), deadpool_redis::redis::RedisError> =
            redis::aio::transaction_async(owned_conn, &[old_key.as_str()], |mut conn, mut pipe| {
                let old_key = old_key.clone();
                let secret_hash = secret_hash.clone();
                let new_key = new_key.clone();
                let new_raw = new_raw.clone();
                let new_token_id = new_token_id.clone();
                let family_key_str = family_key_str.clone();
                let lost_race = &lost_race;
                async move {
                    // Re-read the watched key's *current* value inside the transaction, after
                    // `WATCH` but before `EXEC` — this is the actual guard, not the check made
                    // before this closure was ever called (see this fn's own doc comment).
                    let current_raw: Option<String> = conn.get(&old_key).await?;
                    let still_valid = current_raw
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<RefreshTokenRecord>(raw).ok())
                        .is_some_and(|current| !current.used && current.secret_hash == secret_hash);
                    if !still_valid {
                        // Someone else already rotated (or burned) this token first. No pipeline
                        // commands queued this attempt — `transaction_async` treats `Ok(None)` as
                        // "nothing to commit, stop here" per its own contract; there's genuinely
                        // nothing left to retry toward, so this exits the loop with a
                        // business-level "lost the race" flag rather than looping forever.
                        lost_race.store(true, std::sync::atomic::Ordering::Relaxed);
                        return Ok(Some(()));
                    }
                    let mut marked_used = serde_json::from_str::<RefreshTokenRecord>(
                        current_raw
                            .as_deref()
                            .expect("checked Some above via still_valid"),
                    )
                    .expect("checked deserializable above via still_valid");
                    marked_used.used = true;
                    let old_raw = serde_json::to_string(&marked_used)
                        .expect("RefreshTokenRecord always serializes");
                    pipe.set(&old_key, old_raw)
                        .ignore()
                        .set_ex(&new_key, &new_raw, remaining_ttl)
                        .ignore()
                        .sadd(&family_key_str, &new_token_id)
                        .ignore();
                    pipe.query_async(&mut conn).await
                }
            })
            .await;

        match commit_result {
            Ok(_) if lost_race.load(std::sync::atomic::Ordering::Relaxed) => {
                Ok(RotateOutcome::NotFound)
            }
            Ok(_) => Ok(RotateOutcome::Rotated {
                record: new_record,
                secret: new_secret,
            }),
            Err(e) => Err(e.into()),
        }
    }

    /// Reuse-detection remediation and explicit logout share this: delete every token this family
    /// has ever issued (used or not) plus the family's own membership set. The strongest available
    /// signal that a refresh token was exfiltrated is exactly this — an already-rotated token being
    /// replayed — so the whole chain is treated as compromised, not just the one presented token.
    pub async fn burn_family(&self, family_id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let fkey = family_key(family_id);
        let token_ids: Vec<String> = conn.smembers(&fkey).await?;
        for id in &token_ids {
            let _: () = conn.del(token_key(id)).await?;
        }
        let _: () = conn.del(&fkey).await?;
        Ok(())
    }
}

pub enum RotateOutcome {
    /// No such token, wrong secret, or expired — these are deliberately indistinguishable to the
    /// caller (all map to a plain 401), since none of them indicate token theft the way
    /// `ReuseDetected` does.
    NotFound,
    /// A `used: true` token was presented again — the whole family has now been burned by this
    /// call; the caller should respond 401 and clear both cookies.
    ReuseDetected,
    Rotated {
        record: RefreshTokenRecord,
        secret: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        let url = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let cfg = deadpool_redis::Config::from_url(url);
        cfg.create_pool(Some(deadpool_redis::Runtime::Tokio1)).ok()
    }

    #[tokio::test]
    async fn issues_and_verifies_a_new_family() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);

        let issued = repo.issue_new_family(1_000, 604_800).await.unwrap();
        let fetched = repo.get(&issued.record.token_id).await.unwrap().unwrap();
        assert_eq!(fetched, issued.record);
        assert!(!fetched.used);

        repo.burn_family(&issued.record.family_id).await.unwrap();
        assert_eq!(repo.get(&issued.record.token_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn rotation_produces_a_new_token_and_invalidates_the_old_one() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800).await.unwrap();

        let outcome = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();
        let RotateOutcome::Rotated {
            record: new_record,
            secret: new_secret,
        } = outcome
        else {
            panic!("expected Rotated");
        };
        assert_ne!(new_record.token_id, issued.record.token_id);
        assert_eq!(new_record.family_id, issued.record.family_id);
        // Absolute expiry inherited from the original login, not extended.
        assert_eq!(new_record.expires_at, issued.record.expires_at);

        // The old token is now marked used, not deleted.
        let old_after = repo.get(&issued.record.token_id).await.unwrap().unwrap();
        assert!(old_after.used);

        // The new token verifies (via a second rotation) and is itself usable.
        let second = repo
            .rotate(&new_record.token_id, &new_secret, 1_200)
            .await
            .unwrap();
        assert!(matches!(second, RotateOutcome::Rotated { .. }));

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn reusing_an_already_rotated_token_burns_the_whole_family() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800).await.unwrap();

        let first = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();
        let RotateOutcome::Rotated { record: child, .. } = first else {
            panic!("expected Rotated");
        };

        // Replaying the now-`used` original token must burn the entire family — including the
        // legitimate child that was just minted from it.
        let replay = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_200)
            .await
            .unwrap();
        assert!(matches!(replay, RotateOutcome::ReuseDetected));

        assert_eq!(repo.get(&issued.record.token_id).await.unwrap(), None);
        assert_eq!(
            repo.get(&child.token_id).await.unwrap(),
            None,
            "the legitimate child token must also be burned — reuse is treated as the whole chain being compromised"
        );
    }

    #[tokio::test]
    async fn wrong_secret_is_rejected_without_burning_anything() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800).await.unwrap();

        let outcome = repo
            .rotate(&issued.record.token_id, "wrong-secret", 1_100)
            .await
            .unwrap();
        assert!(matches!(outcome, RotateOutcome::NotFound));

        // The token must still be usable with its real secret — a bad guess isn't evidence of
        // theft, so it must not have triggered any burn.
        let still_valid = repo.get(&issued.record.token_id).await.unwrap().unwrap();
        assert!(!still_valid.used);

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_rotation_of_the_same_token_only_lets_one_succeed() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800).await.unwrap();

        let repo_a = repo.clone();
        let repo_b = repo.clone();
        let token_id = issued.record.token_id.clone();
        let token_id_b = token_id.clone();
        let secret = issued.secret.clone();
        let secret_b = secret.clone();

        let (a, b) = tokio::join!(
            tokio::spawn(async move { repo_a.rotate(&token_id, &secret, 1_100).await }),
            tokio::spawn(async move { repo_b.rotate(&token_id_b, &secret_b, 1_100).await }),
        );
        let a = a.unwrap().unwrap();
        let b = b.unwrap().unwrap();

        let rotated_count = [&a, &b]
            .iter()
            .filter(|o| matches!(o, RotateOutcome::Rotated { .. }))
            .count();
        // Exactly one of the two racing requests may successfully rotate; the loser must see
        // either `ReuseDetected` (if it reads the now-`used` record after the winner's commit) or
        // could itself win in the rare timing where both start their transaction before either
        // commits — but never both "succeeding" with two different child tokens from one parent.
        assert_eq!(rotated_count, 1, "exactly one racer must win the rotation");

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }
}
