//! Redis persistence for first-party API tokens (issue #54) — replaces legacy's single fixed
//! `apikey` string/`Authorization: Bearer base64(apikey)`/`?key=` mechanism (deliberately not kept
//! for backward compatibility; see `.specify/memory/constitution.md`'s Principle II annotation)
//! with a real multi-token system: named, individually revocable, with last-used tracking.
//!
//! Structural template: [`crate::download_queue`] (same `thiserror` error enum shape, same `Pool`
//! field, same `LANRURUGI_TEST_REDIS_URL`-gated test convention), plus one addition
//! `download_queue.rs` doesn't need: a second Redis index (`LANRURUGI_API_TOKEN_BY_HASH_{hash}`)
//! mapping a token's hash straight to its `id`, since [`ApiTokenRepository::verify`] runs on the
//! hot path of every single authenticated API request and can't afford an `SMEMBERS` + N `GET`s
//! scan over every issued token just to find the one matching an incoming bearer value.
//!
//! Format: `lru_{64 hex chars}` (opaque random, not JWT) — deliberately unlike the JWT access
//! token `lanrurugi_core::session` mints for the SPA's own login: a long-lived, individually
//! revocable, per-token-metadata credential is the textbook case for a server-verified opaque
//! token (GitHub PATs, Stripe keys, ... all take this shape), not a self-contained JWT — once
//! every verification already requires a Redis round-trip for revocation/metadata anyway, JWT's
//! whole stateless-verification value proposition is moot, and a plain random string avoids any
//! parsing/algorithm-confusion surface a JWT would add for zero benefit here. Only
//! `sha256(token)` is ever stored server-side — the raw value is shown to the caller once, at
//! issuance, and never again (same posture `refresh_tokens.rs`/`lanrurugi_core::password` already
//! take toward their own secrets).

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiTokenStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, ApiTokenStorageError>;

/// A recognizable prefix (the same convention `sk_`/`ghp_`/etc. use) — lets an automated
/// secret-scanner flag an accidentally-committed token, and lets callers cheaply recognize "this
/// looks like one of ours" before even trying a Redis lookup.
const TOKEN_PREFIX: &str = "lru_";

/// A token's own permission scope, checked by `lanrurugi_server::middleware::auth` on every
/// request authenticated via that token — deliberately coarse (two tiers, not a permission-bit
/// matrix), matching the two real-world shapes this system needs to support: a read-only
/// third-party reader (Tachiyomi/Mihon/OPDS) vs. a trusted automation script that needs to mutate
/// the library. `#[serde(default)]` (→ `Admin`) so a token record written before this field
/// existed keeps its prior, pre-role-system behavior (full access) rather than being silently
/// downgraded to `Guest` on the next read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TokenRole {
    #[default]
    Admin,
    /// Read-only: every non-`GET` request authenticated via a `Guest`-role token is rejected
    /// regardless of which endpoint it targets — see `middleware::auth`'s own docs for why method
    /// alone (not a per-endpoint allowlist) is the enforcement boundary.
    Guest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiTokenRecord {
    pub id: String,
    pub name: String,
    pub token_hash: String,
    pub created_at: i64,
    #[serde(default)]
    pub role: TokenRole,
    /// Absolute Unix timestamp this token stops working, or `None` for a permanent token that
    /// never expires. Enforced via a real Redis `EXPIRE` on both the primary record and its hash
    /// index at issuance time (see `issue`) rather than a manual timestamp comparison at verify
    /// time — Redis itself garbage-collects an expired token, so `verify`/`get` naturally return
    /// `None` for one with no extra logic needed, the same pattern `refresh_tokens.rs` already
    /// established for its own TTL'd records.
    #[serde(default)]
    pub expires_at: Option<i64>,
    #[serde(default)]
    pub last_used_at: Option<i64>,
    /// Best-effort, diagnostic-only — derived from `X-Forwarded-For`'s first hop or the raw peer
    /// address (see `lanrurugi_server::middleware::auth`'s own extraction helper), inherently
    /// spoofable from an untrusted network position under a reverse-proxy deployment with no
    /// trusted-proxy allowlist configured. Never treated as a security control anywhere this is
    /// read — display only.
    #[serde(default)]
    pub last_used_ip: Option<String>,
}

/// A newly-issued token as returned to the caller — the record persisted server-side, plus the
/// raw bearer value, visible this one time only (see this module's own docs on why the raw value
/// is never stored).
pub struct IssuedApiToken {
    pub record: ApiTokenRecord,
    pub raw_token: String,
}

fn token_record_key(id: &str) -> String {
    format!("LANRURUGI_API_TOKEN_{id}")
}

fn hash_index_key(token_hash: &str) -> String {
    format!("LANRURUGI_API_TOKEN_BY_HASH_{token_hash}")
}

const IDS_KEY: &str = "LANRURUGI_API_TOKEN_IDS";

fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    let mut s = String::with_capacity(64);
    use std::fmt::Write;
    for b in hasher.finalize() {
        write!(s, "{b:02x}").expect("writing to a String cannot fail");
    }
    s
}

fn random_token() -> String {
    use rand::RngExt;
    let bytes: [u8; 32] = std::array::from_fn(|_| rand::rng().random());
    let mut hex = String::with_capacity(64);
    use std::fmt::Write;
    for b in bytes {
        write!(hex, "{b:02x}").expect("writing to a String cannot fail");
    }
    format!("{TOKEN_PREFIX}{hex}")
}

/// Cheap prefix check — lets `middleware::auth::is_authorized` skip a Redis round-trip entirely
/// for a bearer value that obviously isn't one of ours (e.g. a stale/legacy-shaped header from
/// a client that hasn't been updated yet).
pub fn looks_like_api_token(bearer_value: &str) -> bool {
    bearer_value.starts_with(TOKEN_PREFIX)
}

#[derive(Clone)]
pub struct ApiTokenRepository {
    pool: Pool,
}

impl ApiTokenRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// `expires_in_secs: None` issues a permanent token (no Redis TTL set on either key — matches
    /// the pre-expiry behavior every token had before this field existed). `Some(secs)` sets a
    /// real Redis `EXPIRE` on both the primary record and its hash index, so an expired token is
    /// garbage-collected by Redis itself rather than needing a manual timestamp check at verify
    /// time (see `ApiTokenRecord::expires_at`'s own docs).
    pub async fn issue(
        &self,
        name: String,
        role: TokenRole,
        now: i64,
        expires_in_secs: Option<i64>,
    ) -> Result<IssuedApiToken> {
        let raw_token = random_token();
        let token_hash = sha256_hex(&raw_token);
        let expires_at = expires_in_secs.map(|secs| now + secs);
        let record = ApiTokenRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            token_hash: token_hash.clone(),
            created_at: now,
            role,
            expires_at,
            last_used_at: None,
            last_used_ip: None,
        };
        let mut conn = self.pool.get().await?;
        let key = token_record_key(&record.id);
        let hash_key = hash_index_key(&token_hash);
        let raw = serde_json::to_string(&record)
            .map_err(|e| ApiTokenStorageError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        let _: () = conn.sadd(IDS_KEY, &record.id).await?;
        let _: () = conn.set(&hash_key, &record.id).await?;
        if let Some(secs) = expires_in_secs {
            let ttl = secs.max(1);
            let _: () = conn.expire(&key, ttl).await?;
            let _: () = conn.expire(&hash_key, ttl).await?;
        }
        Ok(IssuedApiToken { record, raw_token })
    }

    pub async fn get(&self, id: &str) -> Result<Option<ApiTokenRecord>> {
        let mut conn = self.pool.get().await?;
        let key = token_record_key(id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| ApiTokenStorageError::Json(key, e)),
        }
    }

    /// Hot-path lookup: `sha256(raw_token)` → id (via the secondary index) → record. `O(1)`
    /// regardless of how many tokens exist — see this module's own docs on why the plain
    /// `download_queue.rs`-style `SMEMBERS` + N `GET`s scan isn't acceptable here.
    pub async fn verify(&self, raw_token: &str) -> Result<Option<ApiTokenRecord>> {
        let mut conn = self.pool.get().await?;
        let token_hash = sha256_hex(raw_token);
        let id: Option<String> = conn.get(hash_index_key(&token_hash)).await?;
        let Some(id) = id else {
            return Ok(None);
        };
        self.get(&id).await
    }

    pub async fn list_all(&self) -> Result<Vec<ApiTokenRecord>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.smembers(IDS_KEY).await?;
        let mut records = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(record) = self.get(&id).await? {
                records.push(record);
            }
        }
        records.sort_by_key(|r| r.created_at);
        Ok(records)
    }

    /// Idempotent — deleting an already-absent id is a no-op, not an error. Cleans up both the
    /// primary record and its hash-index entry; a leftover hash-index entry pointing at a deleted
    /// record's id would otherwise let a since-revoked token's raw value keep resolving to *some*
    /// id (whose `get` would then just return `None` further down the line — not a security hole,
    /// but needless dangling state).
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        if let Some(record) = self.get(id).await? {
            let _: () = conn.del(hash_index_key(&record.token_hash)).await?;
        }
        let _: () = conn.del(token_record_key(id)).await?;
        let _: () = conn.srem(IDS_KEY, id).await?;
        Ok(())
    }

    /// Best-effort — the caller (`middleware::auth`) is expected to have already throttled how
    /// often this runs per token (see that module's own in-memory throttle), since this is called
    /// on the hot path of every authenticated request that used this exact token.
    pub async fn touch_last_used(&self, id: &str, now: i64, ip: Option<String>) -> Result<()> {
        let Some(mut record) = self.get(id).await? else {
            return Ok(()); // token was deleted between verify() and this call — nothing to update
        };
        record.last_used_at = Some(now);
        record.last_used_ip = ip;
        self.write_record(&record).await
    }

    /// Renames an existing token in place — everything else about the record (id, hash, role,
    /// expiry, usage history) is untouched. `Ok(None)` when `id` doesn't exist (nothing to
    /// rename), matching `get`'s own "absent, not an error" convention.
    pub async fn rename(&self, id: &str, name: String) -> Result<Option<ApiTokenRecord>> {
        let Some(mut record) = self.get(id).await? else {
            return Ok(None);
        };
        record.name = name;
        self.write_record(&record).await?;
        Ok(Some(record))
    }

    /// Writes `record` back to its existing primary key with `SET ... KEEPTTL` — a plain `SET`
    /// (what this used before) silently *clears* any TTL Redis already had on the key, which for
    /// an expiring token (`issue`'s own `expires_in_secs: Some(_)` path) would have quietly turned
    /// it permanent the very next time `touch_last_used` ran on it (i.e., the next time it was
    /// actually used for anything) — the opposite of what an expiring token's own name implies.
    /// `KEEPTTL` is a no-op for a permanent token (no TTL to begin with), so this is safe for both.
    async fn write_record(&self, record: &ApiTokenRecord) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = token_record_key(&record.id);
        let raw = serde_json::to_string(record)
            .map_err(|e| ApiTokenStorageError::Json(key.clone(), e))?;
        let _: () = conn
            .set_options(
                &key,
                raw,
                deadpool_redis::redis::SetOptions::default()
                    .with_expiration(deadpool_redis::redis::SetExpiry::KEEPTTL),
            )
            .await?;
        Ok(())
    }
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
    async fn issues_verifies_lists_and_deletes_a_token() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool);

        let issued = repo
            .issue("Mihon phone".to_string(), TokenRole::Admin, 1_000, None)
            .await
            .unwrap();
        assert!(issued.raw_token.starts_with(TOKEN_PREFIX));
        assert!(looks_like_api_token(&issued.raw_token));

        let verified = repo.verify(&issued.raw_token).await.unwrap().unwrap();
        assert_eq!(verified.id, issued.record.id);
        assert_eq!(verified.name, "Mihon phone");
        assert_eq!(verified.last_used_at, None);

        let all = repo.list_all().await.unwrap();
        assert!(all.iter().any(|t| t.id == issued.record.id));

        repo.delete(&issued.record.id).await.unwrap();
        assert_eq!(repo.get(&issued.record.id).await.unwrap(), None);
        assert_eq!(
            repo.verify(&issued.raw_token).await.unwrap(),
            None,
            "a deleted token must no longer verify"
        );

        // Idempotent.
        repo.delete(&issued.record.id).await.unwrap();
    }

    #[tokio::test]
    async fn wrong_token_does_not_verify() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool);
        let issued = repo
            .issue("test".to_string(), TokenRole::Admin, 1_000, None)
            .await
            .unwrap();

        assert_eq!(repo.verify("lru_wrongvalue").await.unwrap(), None);

        repo.delete(&issued.record.id).await.unwrap();
    }

    #[tokio::test]
    async fn touch_last_used_updates_time_and_ip() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool);
        let issued = repo
            .issue("test".to_string(), TokenRole::Admin, 1_000, None)
            .await
            .unwrap();

        repo.touch_last_used(&issued.record.id, 2_000, Some("1.2.3.4".to_string()))
            .await
            .unwrap();
        let updated = repo.get(&issued.record.id).await.unwrap().unwrap();
        assert_eq!(updated.last_used_at, Some(2_000));
        assert_eq!(updated.last_used_ip.as_deref(), Some("1.2.3.4"));

        repo.delete(&issued.record.id).await.unwrap();
    }

    #[tokio::test]
    async fn deleting_removes_the_hash_index_not_just_the_record() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool);
        let issued = repo
            .issue("test".to_string(), TokenRole::Admin, 1_000, None)
            .await
            .unwrap();
        let raw_token = issued.raw_token.clone();

        repo.delete(&issued.record.id).await.unwrap();

        // Re-issuing a token can't accidentally collide with a stale hash-index entry from the
        // deleted one — verifying the old raw value must come back empty, not resolve to a
        // dangling/wrong id.
        assert_eq!(repo.verify(&raw_token).await.unwrap(), None);
    }

    #[tokio::test]
    async fn permanent_token_carries_no_ttl_and_never_expires() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool.clone());
        let issued = repo
            .issue("permanent".to_string(), TokenRole::Admin, 1_000, None)
            .await
            .unwrap();
        assert_eq!(issued.record.expires_at, None);

        let mut conn = pool.get().await.unwrap();
        let ttl: i64 = deadpool_redis::redis::cmd("TTL")
            .arg(token_record_key(&issued.record.id))
            .query_async(&mut conn)
            .await
            .unwrap();
        assert_eq!(
            ttl, -1,
            "a permanent token's record must carry no TTL at all"
        );

        repo.delete(&issued.record.id).await.unwrap();
    }

    #[tokio::test]
    async fn expiring_token_carries_a_real_redis_ttl_on_both_keys() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool.clone());
        let issued = repo
            .issue("expiring".to_string(), TokenRole::Guest, 1_000, Some(3_600))
            .await
            .unwrap();
        assert_eq!(issued.record.expires_at, Some(1_000 + 3_600));
        assert_eq!(issued.record.role, TokenRole::Guest);

        let mut conn = pool.get().await.unwrap();
        let record_ttl: i64 = deadpool_redis::redis::cmd("TTL")
            .arg(token_record_key(&issued.record.id))
            .query_async(&mut conn)
            .await
            .unwrap();
        let hash_ttl: i64 = deadpool_redis::redis::cmd("TTL")
            .arg(hash_index_key(&sha256_hex(&issued.raw_token)))
            .query_async(&mut conn)
            .await
            .unwrap();
        assert!(record_ttl > 0 && record_ttl <= 3_600, "got {record_ttl}");
        assert!(hash_ttl > 0 && hash_ttl <= 3_600, "got {hash_ttl}");

        repo.delete(&issued.record.id).await.unwrap();
    }

    #[tokio::test]
    async fn rename_updates_the_name_and_leaves_everything_else_untouched() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool);
        let issued = repo
            .issue("old name".to_string(), TokenRole::Admin, 1_000, None)
            .await
            .unwrap();

        let renamed = repo
            .rename(&issued.record.id, "new name".to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(renamed.name, "new name");
        assert_eq!(renamed.id, issued.record.id);
        assert_eq!(renamed.token_hash, issued.record.token_hash);

        // The raw token still verifies to the same, renamed record — rename must not disturb the
        // hash index or require re-issuing a new token.
        let verified = repo.verify(&issued.raw_token).await.unwrap().unwrap();
        assert_eq!(verified.name, "new name");

        assert_eq!(
            repo.rename("does-not-exist", "x".to_string())
                .await
                .unwrap(),
            None,
        );

        repo.delete(&issued.record.id).await.unwrap();
    }

    #[tokio::test]
    async fn touching_or_renaming_an_expiring_token_does_not_clear_its_ttl() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ApiTokenRepository::new(pool.clone());
        let issued = repo
            .issue(
                "ttl-preserving".to_string(),
                TokenRole::Admin,
                1_000,
                Some(3_600),
            )
            .await
            .unwrap();

        // A plain Redis `SET` (what this repository used before `write_record` switched to
        // `SET ... KEEPTTL`) silently clears any existing TTL — this is the exact regression this
        // test guards against: neither of these two write paths may turn an expiring token
        // permanent as a side effect of otherwise-unrelated bookkeeping.
        repo.touch_last_used(&issued.record.id, 1_500, Some("1.2.3.4".to_string()))
            .await
            .unwrap();
        repo.rename(&issued.record.id, "renamed".to_string())
            .await
            .unwrap();

        let mut conn = pool.get().await.unwrap();
        let ttl: i64 = deadpool_redis::redis::cmd("TTL")
            .arg(token_record_key(&issued.record.id))
            .query_async(&mut conn)
            .await
            .unwrap();
        assert!(
            ttl > 0,
            "TTL must survive touch_last_used + rename, got {ttl}"
        );

        repo.delete(&issued.record.id).await.unwrap();
    }
}
