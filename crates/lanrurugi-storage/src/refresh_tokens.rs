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

use crate::device_info::DeviceInfo;

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

/// How long after a refresh token is rotated out that presenting it again is still forgiven as
/// benign same-family concurrent reuse (multiple browser tabs racing to refresh off the same
/// stale cookie — see this module's own `rotate` docs) rather than treated as a reuse attack.
/// Anchored to the *first* rotation's `used_at`, never extended by a later forgiven presentation
/// — otherwise a token could be kept alive indefinitely by presenting it every `N < GRACE` seconds.
const REUSE_GRACE_SECS: i64 = 5;

/// Caps how many live sibling tokens one forgiven-reuse token can spawn within its grace window —
/// a real benign case (a handful of tabs) never approaches this; it exists so a retry-loop bug or
/// an attacker racing the grace window can't mint unbounded valid tokens from one presentation.
const MAX_GRACE_REUSES: u32 = 3;

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
    /// Sliding idle expiry, renewed on every successful rotation and capped at `expires_at`.
    /// `None` for records written before this field existed; callers treat that as "idle window
    /// equals absolute expiry" so pre-migration sessions keep their old absolute-only behavior.
    #[serde(default)]
    pub idle_expires_at: Option<i64>,
    /// `true` once this exact token has already been redeemed for a new one — a second
    /// presentation of a `used: true` token is the reuse-detection trigger, unless it falls
    /// within [`REUSE_GRACE_SECS`] of `used_at` (see [`rotate`]'s own docs).
    pub used: bool,
    /// When this token was first redeemed. `None` for a never-used token, and — critically —
    /// also `None` for pre-migration Redis records that predate this field (`serde(default)`),
    /// which deliberately fails safe: no grace period, straight to `ReuseDetected`, same as
    /// this codebase's behavior before the grace period existed.
    #[serde(default)]
    pub used_at: Option<i64>,
    /// How many times a presentation of this token has been forgiven as benign same-family
    /// concurrent reuse within the grace window (see [`rotate`]). Capped at
    /// [`MAX_GRACE_REUSES`] — beyond that, further presentations burn the family same as an
    /// out-of-window replay, so a buggy retry loop (or a real attacker) can't mint unbounded
    /// live sibling tokens from one forgiven presentation.
    #[serde(default)]
    pub grace_reuse_count: u32,
    /// The device that first logged in to start this family — captured once at
    /// [`RefreshTokenRepository::issue_new_family`] and copied forward unchanged on every
    /// [`RefreshTokenRepository::rotate`] (never re-derived from the rotating request's own
    /// User-Agent), so it stays a stable "who this family belongs to" baseline a caller can diff a
    /// rotation's *actual* presenting device against (see
    /// `lanrurugi_storage::activity::action_types::SESSION_DEVICE_CHANGED`). `None` for a
    /// pre-migration record predating this field, or if `User-Agent` parsing failed.
    #[serde(default)]
    pub device_info: Option<DeviceInfo>,
}

/// A newly-issued token pair as returned to a caller — the record persisted server-side, plus the
/// one-time-visible bearer secret needed to construct the `"{token_id}.{secret}"` cookie value.
/// Mirrors [`crate::api_tokens::IssuedApiToken`]'s "raw secret only exists at issuance time"
/// shape.
pub struct IssuedRefreshToken {
    pub record: RefreshTokenRecord,
    pub secret: String,
}

/// Request-derived metadata the repository stores alongside a login family so the Settings
/// "active devices" list can show something useful without re-parsing a User-Agent on every read.
/// Both fields are best-effort: `device_info` may be `None` when the UA couldn't be parsed, and
/// `client_ip` is display/diagnostic only (same posture as the rest of this codebase's
/// `client_ip`, never a security control).
#[derive(Debug, Clone, Default)]
pub struct SessionContext {
    pub device_info: Option<DeviceInfo>,
    pub client_ip: Option<String>,
}

/// One active login family's own metadata record — separate from the rotating token records so a
/// device list needs one `GET` per family rather than scanning every ever-issued token. `expires_at`
/// is the absolute cap anchored to the original login; `idle_expires_at` is the sliding window
/// that each successful rotation renews (capped at `expires_at`). `custom_name` is the
/// administrator-overridable label; when `None`, `device_name()` falls back to the auto-generated
/// device label.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefreshFamilyMeta {
    pub family_id: String,
    pub created_at: i64,
    pub last_seen_at: i64,
    pub idle_expires_at: i64,
    pub expires_at: i64,
    pub device_info: Option<DeviceInfo>,
    pub last_ip: Option<String>,
    pub current_token_id: String,
    #[serde(default)]
    pub custom_name: Option<String>,
}

impl RefreshFamilyMeta {
    /// Custom rename wins; otherwise derive a fresh best-effort label from the last-seen device
    /// info. A missing/legacy `device_info` reads as `"Unknown device"` rather than being hidden.
    pub fn device_name(&self) -> String {
        self.custom_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                self.device_info
                    .as_ref()
                    .map(DeviceInfo::display_name)
                    .unwrap_or_else(|| "Unknown device".to_string())
            })
    }
}

fn token_key(token_id: &str) -> String {
    format!("LANRURUGI_REFRESH_TOKEN_{token_id}")
}

fn family_key(family_id: &str) -> String {
    format!("LANRURUGI_REFRESH_FAMILY_{family_id}")
}

fn family_meta_key(family_id: &str) -> String {
    format!("LANRURUGI_REFRESH_FAMILY_META_{family_id}")
}

/// Global sorted set of active family ids, scored by `last_seen_at`. Kept separately from each
/// family's own membership set so the Settings device list can do an ordered `ZREVRANGE` instead
/// of `SCAN`-ing every `LANRURUGI_REFRESH_FAMILY_*` key. Stale members are pruned lazily whenever
/// a list/limit operation notices the corresponding meta key is gone or expired.
const FAMILY_INDEX_KEY: &str = "LANRURUGI_REFRESH_FAMILY_INDEX";

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

/// Whether presenting an already-`used` token right now should be forgiven as benign
/// same-family concurrent reuse rather than treated as a reuse attack — see [`REUSE_GRACE_SECS`]
/// and [`MAX_GRACE_REUSES`]. A record with `used_at: None` (never used, or a pre-migration record
/// predating this field) is never forgivable — only a token with a recorded first-use timestamp
/// can be within a grace window of it.
fn is_forgivable_reuse(record: &RefreshTokenRecord, now: i64) -> bool {
    let Some(used_at) = record.used_at else {
        return false;
    };
    record.grace_reuse_count < MAX_GRACE_REUSES && now.saturating_sub(used_at) <= REUSE_GRACE_SECS
}

/// Effective idle expiry: `now + idle_lifetime_secs`, capped at the family's absolute expiry.
/// A non-positive `idle_lifetime_secs` means "no separate idle limit" (legacy absolute-only
/// behavior), so the idle deadline simply equals the absolute one.
fn idle_expiry(now: i64, idle_lifetime_secs: i64, absolute_expires_at: i64) -> i64 {
    if idle_lifetime_secs <= 0 {
        absolute_expires_at
    } else {
        now.saturating_add(idle_lifetime_secs)
            .min(absolute_expires_at)
    }
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
    /// once, from `login.rs`'s `login` handler, on successful password verification. `device_info`
    /// is this family's baseline device, parsed from the login request's own `User-Agent`.
    ///
    /// Kept as a backwards-compatible convenience wrapper for tests and pre-dual-window callers:
    /// it makes the idle window equal to the absolute window (the old absolute-only behavior).
    pub async fn issue_new_family(
        &self,
        now: i64,
        lifetime_secs: i64,
        device_info: Option<DeviceInfo>,
    ) -> Result<IssuedRefreshToken> {
        self.issue_new_family_with_idle(
            now,
            lifetime_secs,
            lifetime_secs,
            SessionContext {
                device_info,
                client_ip: None,
            },
        )
        .await
    }

    /// Dual-window variant used by the login handler: `absolute_lifetime_secs` anchors the hard
    /// expiry to this login, while `idle_lifetime_secs` is the sliding window each successful
    /// rotation renews. The effective idle expiry is always capped at the absolute one. A
    /// non-positive idle value means "no separate idle limit" (same as the old absolute-only
    /// behavior), which keeps pre-existing tests/older callers working unchanged.
    pub async fn issue_new_family_with_idle(
        &self,
        now: i64,
        absolute_lifetime_secs: i64,
        idle_lifetime_secs: i64,
        context: SessionContext,
    ) -> Result<IssuedRefreshToken> {
        let family_id = uuid::Uuid::new_v4().to_string();
        self.issue_in_family(
            &family_id,
            now,
            absolute_lifetime_secs,
            idle_lifetime_secs,
            context,
        )
        .await
    }

    async fn issue_in_family(
        &self,
        family_id: &str,
        now: i64,
        absolute_lifetime_secs: i64,
        idle_lifetime_secs: i64,
        context: SessionContext,
    ) -> Result<IssuedRefreshToken> {
        let token_id = uuid::Uuid::new_v4().to_string();
        let secret = random_hex();
        let expires_at = now + absolute_lifetime_secs.max(1);
        let idle_expires_at = idle_expiry(now, idle_lifetime_secs, expires_at);
        let record = RefreshTokenRecord {
            token_id: token_id.clone(),
            secret_hash: sha256_hex(&secret),
            family_id: family_id.to_string(),
            issued_at: now,
            expires_at,
            idle_expires_at: Some(idle_expires_at),
            used: false,
            used_at: None,
            grace_reuse_count: 0,
            device_info: context.device_info.clone(),
        };
        let meta = RefreshFamilyMeta {
            family_id: family_id.to_string(),
            created_at: now,
            last_seen_at: now,
            idle_expires_at,
            expires_at,
            device_info: context.device_info,
            last_ip: context.client_ip,
            current_token_id: token_id.clone(),
            custom_name: None,
        };
        let ttl_secs: u64 = (idle_expires_at - now).max(1) as u64;
        let key = token_key(&token_id);
        let raw = serde_json::to_string(&record)
            .map_err(|e| RefreshTokenStorageError::Json(key.clone(), e))?;
        let meta_key = family_meta_key(family_id);
        let meta_raw = serde_json::to_string(&meta)
            .map_err(|e| RefreshTokenStorageError::Json(meta_key.clone(), e))?;

        let mut conn = self.pool.get().await?;
        let _: () = conn.set_ex(&key, raw, ttl_secs).await?;
        let _: () = conn.sadd(family_key(family_id), &token_id).await?;
        // The meta/index are what make this session visible to the Settings device list. Writes
        // are non-transactional for simplicity, but if either fails we clean up rather than leave
        // a half-created family: the login can safely be retried with a fresh password submission.
        if let Err(e) = conn.set_ex::<_, _, ()>(&meta_key, meta_raw, ttl_secs).await {
            self.burn_family(family_id).await.ok();
            return Err(e.into());
        }
        if let Err(e) = conn
            .zadd::<_, _, _, ()>(FAMILY_INDEX_KEY, family_id, now)
            .await
        {
            self.burn_family(family_id).await.ok();
            return Err(e.into());
        }
        Ok(IssuedRefreshToken { record, secret })
    }

    /// Active-family listing for the Settings device UI, newest `last_seen_at` first. Expired
    /// family metas/indices are pruned lazily here (and their token membership sets burned) so a
    /// long-lived instance never leaks index entries forever.
    pub async fn list_active_families(&self, now: i64) -> Result<Vec<RefreshFamilyMeta>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.zrevrange(FAMILY_INDEX_KEY, 0, -1).await?;
        let mut active = Vec::new();
        for family_id in ids {
            let meta_key = family_meta_key(&family_id);
            let raw: Option<String> = conn.get(&meta_key).await?;
            let Some(raw) = raw else {
                let _: () = conn.zrem(FAMILY_INDEX_KEY, &family_id).await?;
                continue;
            };
            let meta = match serde_json::from_str::<RefreshFamilyMeta>(&raw) {
                Ok(meta) => meta,
                Err(e) => {
                    tracing::warn!(%family_id, error = %e, "pruning malformed refresh family meta");
                    let _: () = conn.del(&meta_key).await?;
                    let _: () = conn.zrem(FAMILY_INDEX_KEY, &family_id).await?;
                    continue;
                }
            };
            if now > meta.expires_at || now > meta.idle_expires_at {
                let _: () = conn.del(&meta_key).await?;
                let _: () = conn.zrem(FAMILY_INDEX_KEY, &family_id).await?;
                self.burn_family(&family_id).await?;
                continue;
            }
            active.push(meta);
        }
        Ok(active)
    }

    /// Evicts oldest families until at most `max_devices` remain, returning the evicted metadata
    /// (for optional activity/logging). `max_devices <= 0` means unlimited.
    pub async fn enforce_device_limit(
        &self,
        now: i64,
        max_devices: i64,
    ) -> Result<Vec<RefreshFamilyMeta>> {
        if max_devices <= 0 {
            return Ok(Vec::new());
        }
        let mut active = self.list_active_families(now).await?;
        let max = max_devices as usize;
        if active.len() <= max {
            return Ok(Vec::new());
        }
        // New lists are newest-first; evict from the tail (oldest last_seen_at).
        let evicted = active.split_off(max);
        for meta in &evicted {
            self.burn_family(&meta.family_id).await?;
        }
        Ok(evicted)
    }

    /// Fetches one family's meta without the filtering/pruning `list_active_families` performs —
    /// used by the rename/revoke handlers to distinguish "unknown device" from "expired device".
    pub async fn get_family_meta(&self, family_id: &str) -> Result<Option<RefreshFamilyMeta>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.get(family_meta_key(family_id)).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| RefreshTokenStorageError::Json(family_meta_key(family_id), e)),
        }
    }

    /// Renames one active family. Returns `None` when the family is unknown. The custom name is
    /// stored separately so the auto-generated label remains available if we ever expose a reset.
    pub async fn rename_family(
        &self,
        family_id: &str,
        name: &str,
        now: i64,
    ) -> Result<Option<RefreshFamilyMeta>> {
        let Some(mut meta) = self.get_family_meta(family_id).await? else {
            return Ok(None);
        };
        let trimmed = name.trim();
        meta.custom_name = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        };
        let key = family_meta_key(family_id);
        let raw = serde_json::to_string(&meta)
            .map_err(|e| RefreshTokenStorageError::Json(key.clone(), e))?;
        let ttl_secs = (meta.idle_expires_at - now).max(1) as u64;
        let mut conn = self.pool.get().await?;
        let _: () = conn.set_ex(key, raw, ttl_secs).await?;
        Ok(Some(meta))
    }

    /// Deletes one family if it exists (including its meta/index entry), returning whether
    /// anything was actually revoked. This is the `DELETE /sessions/{id}` primitive; `burn_family`
    /// remains the unconditional internal cleanup used by reuse detection and expiration pruning.
    pub async fn revoke_family(&self, family_id: &str) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        let exists_meta: bool = conn.exists(family_meta_key(family_id)).await?;
        let exists_family: bool = conn.exists(family_key(family_id)).await?;
        if !exists_meta && !exists_family {
            return Ok(false);
        }
        drop(conn);
        self.burn_family(family_id).await?;
        Ok(true)
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
    /// Two racing callers presenting the *same* still-valid (never-used) token must never both
    /// "succeed" from that single presentation — that would silently mint two children from one
    /// single-use parent. `WATCH` alone doesn't give this for free: it only aborts the `EXEC` if
    /// the watched key's value changed between `WATCH` and `EXEC`, it does NOT re-run arbitrary
    /// business logic (the `used` check) against the fresh value on a retry. So the `used` check
    /// itself has to live *inside* the transaction closure, re-reading the record fresh on every
    /// attempt (including retries) — checking it once outside the closure, then writing the same
    /// fixed payload on every retry regardless of what changed, is exactly the bug this whole
    /// function exists to avoid (this was caught by
    /// `concurrent_rotation_of_the_same_token_only_lets_one_succeed`'s own test failing during
    /// development — both racers "won" the first time this was written that way).
    ///
    /// A *separate* presentation of an already-`used` token, within [`REUSE_GRACE_SECS`] of that
    /// token's `used_at` and under [`MAX_GRACE_REUSES`] prior forgiven presentations, is treated
    /// as benign same-family concurrent reuse (multiple browser tabs racing off one stale cookie)
    /// rather than a reuse attack: it still mints a fresh rotated child (so the caller gets a
    /// working session back), but does NOT touch `used_at` or reset the grace clock — only the
    /// grace-reuse counter advances. Outside that window/count, or on a token that was never used
    /// at all before this presentation raced with another, `burn_family` still fires exactly as
    /// before.
    pub async fn rotate(&self, token_id: &str, secret: &str, now: i64) -> Result<RotateOutcome> {
        self.rotate_with_context(token_id, secret, now, 0, SessionContext::default())
            .await
    }

    /// Same rotation/reuse-detection semantics as before, plus the dual-window idle refresh and
    /// the family-meta update that drives the active-device list. `idle_lifetime_secs <= 0` keeps
    /// the caller's pre-dual-window absolute-only behavior (idle expiry follows the parent's
    /// absolute expiry). `context` updates the family's last-seen device/IP; the original
    /// login's `device_info` remains the fallback when a particular refresh request couldn't
    /// provide one.
    pub async fn rotate_with_context(
        &self,
        token_id: &str,
        secret: &str,
        now: i64,
        idle_lifetime_secs: i64,
        context: SessionContext,
    ) -> Result<RotateOutcome> {
        let Some(record) = self.get(token_id).await? else {
            return Ok(RotateOutcome::NotFound);
        };
        if !lanrurugi_core::crypto::constant_time_eq(&sha256_hex(secret), &record.secret_hash) {
            return Ok(RotateOutcome::NotFound); // wrong secret: treat identically to "no such token"
        }
        let current_idle_expires_at = record.idle_expires_at.unwrap_or(record.expires_at);
        if now > record.expires_at || now > current_idle_expires_at {
            return Ok(RotateOutcome::NotFound); // absolute or idle expiry; TTL normally reaps it
                                                // first, but a request racing the boundary must
                                                // still get "gone" rather than a late refresh
        }
        if record.used && !is_forgivable_reuse(&record, now) {
            self.burn_family(&record.family_id).await?;
            return Ok(RotateOutcome::ReuseDetected);
        }

        let existing_meta = self.get_family_meta(&record.family_id).await.ok().flatten();
        let created_at = existing_meta
            .as_ref()
            .map(|meta| meta.created_at)
            .unwrap_or(record.issued_at);
        let custom_name = existing_meta
            .as_ref()
            .and_then(|meta| meta.custom_name.clone());
        let meta_last_ip = context
            .client_ip
            .clone()
            .or_else(|| existing_meta.as_ref().and_then(|meta| meta.last_ip.clone()));

        let mut conn = self.pool.get().await?;
        let old_key = token_key(token_id);
        let new_token_id = uuid::Uuid::new_v4().to_string();
        let new_secret = random_hex();
        let secret_hash = record.secret_hash.clone();
        let family_id = record.family_id.clone();
        let expires_at = record.expires_at;
        let new_idle_expires_at = idle_expiry(now, idle_lifetime_secs, expires_at);
        let new_record = RefreshTokenRecord {
            token_id: new_token_id.clone(),
            secret_hash: sha256_hex(&new_secret),
            family_id: family_id.clone(),
            issued_at: now,
            // Absolute expiry inherited from the parent, NOT reset to `now + lifetime` — an
            // actively-rotating chain must still expire on schedule from the original login,
            // otherwise "N-day refresh lifetime" is meaningless (see this record's own field docs).
            expires_at,
            idle_expires_at: Some(new_idle_expires_at),
            used: false,
            used_at: None,
            grace_reuse_count: 0,
            // Baseline device is copied forward from the parent unless this rotation's own
            // request supplied fresh device info (the latter is what the device list shows).
            device_info: context
                .device_info
                .clone()
                .or_else(|| record.device_info.clone()),
        };
        let new_key = token_key(&new_token_id);
        let new_raw = serde_json::to_string(&new_record)
            .map_err(|e| RefreshTokenStorageError::Json(new_key.clone(), e))?;
        let remaining_ttl: u64 = (new_idle_expires_at - now).max(1) as u64;
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
                    let current = current_raw
                        .as_deref()
                        .and_then(|raw| serde_json::from_str::<RefreshTokenRecord>(raw).ok())
                        .filter(|current| current.secret_hash == secret_hash);
                    let still_valid = current.as_ref().is_some_and(|current| {
                        let current_idle = current.idle_expires_at.unwrap_or(current.expires_at);
                        now <= current.expires_at
                            && now <= current_idle
                            && (!current.used || is_forgivable_reuse(current, now))
                    });
                    if !still_valid {
                        // Someone else already rotated (or burned) this token first, or the grace
                        // window/count for a forgivable reuse has since been exhausted by another
                        // racer's own forgiven presentation. No pipeline commands queued this
                        // attempt — `transaction_async` treats `Ok(None)` as "nothing to commit,
                        // stop here" per its own contract; there's genuinely nothing left to retry
                        // toward, so this exits the loop with a business-level "lost the race" flag
                        // rather than looping forever.
                        lost_race.store(true, std::sync::atomic::Ordering::Relaxed);
                        return Ok(Some(()));
                    }
                    let mut marked_used = current.expect("checked Some above via still_valid");
                    if marked_used.used {
                        // Forgivable reuse of an already-rotated token: advance only the
                        // grace-reuse counter, never touch `used_at` — extending it would let a
                        // token presented every `N < REUSE_GRACE_SECS` seconds stay alive forever.
                        marked_used.grace_reuse_count += 1;
                    } else {
                        marked_used.used = true;
                        marked_used.used_at = Some(now);
                    }
                    let old_raw = serde_json::to_string(&marked_used)
                        .expect("RefreshTokenRecord always serializes");
                    // `set_ex`, not `set` — a plain `SET` clears the key's existing TTL, which
                    // would leave every rotated-out token permanently resident in Redis instead of
                    // expiring alongside the session it belonged to.
                    pipe.set_ex(&old_key, old_raw, remaining_ttl)
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
            Ok(_) => {
                // Best-effort family metadata update. A failure here must never fail the
                // already-committed rotation — the caller would retry with the now-used parent
                // token and trip reuse detection, needlessly burning the whole family. The only
                // consequence of a missed update is a stale device-list row until the next rotate.
                let meta = RefreshFamilyMeta {
                    family_id: family_id.clone(),
                    created_at,
                    last_seen_at: now,
                    idle_expires_at: new_idle_expires_at,
                    expires_at,
                    device_info: new_record.device_info.clone(),
                    last_ip: meta_last_ip,
                    current_token_id: new_token_id.clone(),
                    custom_name: custom_name.clone(),
                };
                if let Ok(raw) = serde_json::to_string(&meta) {
                    let meta_key = family_meta_key(&family_id);
                    if let Err(e) = conn.set_ex::<_, _, ()>(&meta_key, raw, remaining_ttl).await {
                        tracing::warn!(%family_id, error = %e, "failed to update refresh family meta");
                    }
                    if let Err(e) = conn
                        .zadd::<_, _, _, ()>(FAMILY_INDEX_KEY, &family_id, now)
                        .await
                    {
                        tracing::warn!(%family_id, error = %e, "failed to update refresh family index");
                    }
                }
                Ok(RotateOutcome::Rotated {
                    record: Box::new(new_record),
                    secret: new_secret,
                })
            }
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
        // Also remove the device-list row/index entry, not just the credential chain.
        let _: () = conn.del(family_meta_key(family_id)).await?;
        let _: () = conn.zrem(FAMILY_INDEX_KEY, family_id).await?;
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
        // Boxed — `RefreshTokenRecord` grew past clippy's `large_enum_variant` threshold once
        // `device_info` (three `String`s) was added, and `NotFound`/`ReuseDetected` are
        // zero-sized, so leaving it unboxed would pad every `RotateOutcome` (including those two
        // common variants) out to this one variant's much larger size.
        record: Box<RefreshTokenRecord>,
        secret: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        crate::test_support::test_pool().await
    }

    #[tokio::test]
    async fn issues_and_verifies_a_new_family() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);

        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();
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
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

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
    async fn rotation_preserves_the_old_tokens_ttl_instead_of_making_it_permanent() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool.clone());
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

        repo.rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();

        // A plain `SET` (no `EX`) clears a key's existing TTL, leaving it resident in Redis
        // forever — the bug this test guards against. The rotated-out old key must still carry a
        // finite, positive TTL, not -1 ("no expiry").
        let mut conn = pool.get().await.unwrap();
        let ttl: i64 = conn.ttl(token_key(&issued.record.token_id)).await.unwrap();
        assert!(
            ttl > 0,
            "expected a finite TTL on the rotated-out old token, got {ttl}"
        );

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn reusing_an_already_rotated_token_burns_the_whole_family() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

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
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

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
    async fn concurrent_rotation_of_the_same_token_never_burns_the_family() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

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
        // The winner commits a normal rotation; the loser re-reads inside its own transaction
        // attempt, sees `used: true` with `used_at` equal to `now` (0 seconds elapsed, well
        // within the grace window) and forgives it as benign same-family concurrent reuse —
        // this is exactly the multi-tab scenario the grace window exists for. Neither racer
        // burns the family, and both walk away with a working rotated token.
        assert_eq!(
            rotated_count, 2,
            "both racers should succeed — the loser's reuse is within the grace window"
        );
        assert!(
            repo.get(&issued.record.family_id).await.is_ok(),
            "family must not have been burned"
        );

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn reuse_within_the_grace_window_is_forgiven_and_mints_a_working_token() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

        let first = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();
        assert!(matches!(first, RotateOutcome::Rotated { .. }));

        // A second tab, still holding the same now-rotated-out cookie, presents it again 3
        // seconds later — within `REUSE_GRACE_SECS` (5). It must NOT burn the family, and must
        // get back its own working rotated token (not the first tab's child).
        let second = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_103)
            .await
            .unwrap();
        let RotateOutcome::Rotated {
            record: second_child,
            ..
        } = second
        else {
            panic!("expected the grace-window reuse to still rotate, got a different outcome");
        };

        let parent_after = repo.get(&issued.record.token_id).await.unwrap().unwrap();
        assert_eq!(
            parent_after.grace_reuse_count, 1,
            "the grace-reuse counter must advance"
        );
        assert_eq!(
            parent_after.used_at,
            Some(1_100),
            "used_at must stay pinned to the first rotation, not the forgiven reuse"
        );

        // Both children remain independently usable.
        assert!(repo.get(&second_child.token_id).await.unwrap().is_some());

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn reuse_outside_the_grace_window_still_burns_the_family() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

        repo.rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();

        // 6 seconds later — just past `REUSE_GRACE_SECS` (5) — must burn, same as before this
        // feature existed. Guards against a token being kept alive indefinitely by presenting it
        // every `N < REUSE_GRACE_SECS` seconds forever.
        let replay = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_106)
            .await
            .unwrap();
        assert!(matches!(replay, RotateOutcome::ReuseDetected));
        assert_eq!(repo.get(&issued.record.token_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn grace_reuse_count_is_capped_then_burns() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

        repo.rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();

        // MAX_GRACE_REUSES (3) forgiven presentations, all well within the grace window.
        for _ in 0..3 {
            let outcome = repo
                .rotate(&issued.record.token_id, &issued.secret, 1_101)
                .await
                .unwrap();
            assert!(matches!(outcome, RotateOutcome::Rotated { .. }));
        }

        // The 4th presentation, still within the time window, must now burn — the count cap was
        // exhausted by the 3 prior forgiven presentations.
        let fourth = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_101)
            .await
            .unwrap();
        assert!(matches!(fourth, RotateOutcome::ReuseDetected));
        assert_eq!(repo.get(&issued.record.token_id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_used_token_missing_used_at_is_never_forgiven() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool.clone());
        let issued = repo.issue_new_family(1_000, 604_800, None).await.unwrap();

        repo.rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();

        // Simulate a pre-migration Redis record: `used: true` but no `used_at` (as if written by
        // the old binary before this field existed). `serde(default)` is what makes this JSON
        // shape parse at all.
        let mut conn = pool.get().await.unwrap();
        let mut stale = repo.get(&issued.record.token_id).await.unwrap().unwrap();
        stale.used_at = None;
        let raw = serde_json::to_string(&stale).unwrap();
        let _: () = conn
            .set_ex(token_key(&issued.record.token_id), raw, 604_800)
            .await
            .unwrap();

        // Immediately replaying it (0 seconds elapsed) must still burn — a missing `used_at`
        // fails safe to "not forgivable", matching this codebase's pre-grace-window behavior.
        let replay = repo
            .rotate(&issued.record.token_id, &issued.secret, 1_100)
            .await
            .unwrap();
        assert!(matches!(replay, RotateOutcome::ReuseDetected));
    }
}

#[cfg(test)]
mod dual_window_tests {
    use super::*;

    /// These tests exercise the global device index/limit, so each must not share a Redis
    /// logical DB with another concurrently-running test in this same test binary. Each gets its
    /// own DB from a small rotating pool (Redis's default 16 DBs: 0 is left for the older tests,
    /// 5..=14 are ours) and flushes it first; this keeps the tests deterministic under cargo's
    /// default parallel test-thread execution.
    const FIRST_TEST_DB: u8 = 5;
    const TEST_DB_COUNT: u8 = 10;
    static NEXT_TEST_DB: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

    async fn test_pool() -> Option<Pool> {
        let base = std::env::var("LANRURUGI_TEST_REDIS_URL").ok()?;
        let offset =
            NEXT_TEST_DB.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % TEST_DB_COUNT;
        let url = format!("{}/{}", base.trim_end_matches('/'), FIRST_TEST_DB + offset);
        let pool = crate::test_support::test_pool_for_url(&url).await?;
        let mut conn = pool.get().await.ok()?;
        let _: () = deadpool_redis::redis::cmd("FLUSHDB")
            .query_async(&mut conn)
            .await
            .ok()?;
        Some(pool)
    }

    #[tokio::test]
    async fn idle_window_renews_on_each_rotation_but_never_passes_absolute_expiry() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);

        // Login at t=1000: absolute 1000s (-> 2000), idle 400s (-> 1400).
        let issued = repo
            .issue_new_family_with_idle(1_000, 1_000, 400, SessionContext::default())
            .await
            .unwrap();
        assert_eq!(issued.record.idle_expires_at, Some(1_400));

        // Rotation at t=1050 renews idle to 1450, still below absolute.
        let first = repo
            .rotate_with_context(
                &issued.record.token_id,
                &issued.secret,
                1_050,
                400,
                SessionContext::default(),
            )
            .await
            .unwrap();
        let RotateOutcome::Rotated {
            record: first_child,
            secret: first_secret,
        } = first
        else {
            panic!("expected Rotated");
        };
        assert_eq!(first_child.idle_expires_at, Some(1_450));
        assert_eq!(first_child.expires_at, 2_000);

        // Rotation at t=1400 would renew idle to 1800, still below absolute.
        let second = repo
            .rotate_with_context(
                &first_child.token_id,
                &first_secret,
                1_400,
                400,
                SessionContext::default(),
            )
            .await
            .unwrap();
        let RotateOutcome::Rotated {
            record: second_child,
            secret: second_secret,
        } = second
        else {
            panic!("expected Rotated");
        };
        assert_eq!(second_child.idle_expires_at, Some(1_800));

        // Rotation at t=1750 (still inside the 1800 idle deadline) with a 400s idle window
        // would be 2150, capped to absolute 2000.
        let third = repo
            .rotate_with_context(
                &second_child.token_id,
                &second_secret,
                1_750,
                400,
                SessionContext::default(),
            )
            .await
            .unwrap();
        let RotateOutcome::Rotated {
            record: third_child,
            secret: third_secret,
        } = third
        else {
            panic!("expected Rotated");
        };
        assert_eq!(third_child.idle_expires_at, Some(2_000));
        assert_eq!(third_child.expires_at, 2_000);

        // One second past absolute expiry must fail.
        let expired = repo
            .rotate_with_context(
                &third_child.token_id,
                &third_secret,
                2_001,
                400,
                SessionContext::default(),
            )
            .await
            .unwrap();
        assert!(matches!(expired, RotateOutcome::NotFound));

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn idle_expiry_alone_rejects_rotation_even_before_absolute_expiry() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo
            .issue_new_family_with_idle(1_000, 10_000, 100, SessionContext::default())
            .await
            .unwrap();

        let outcome = repo
            .rotate_with_context(
                &issued.record.token_id,
                &issued.secret,
                1_101,
                100,
                SessionContext::default(),
            )
            .await
            .unwrap();
        assert!(matches!(outcome, RotateOutcome::NotFound));

        repo.burn_family(&issued.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn device_limit_evicts_the_oldest_seen_family() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let oldest = repo
            .issue_new_family_with_idle(1_000, 10_000, 10_000, SessionContext::default())
            .await
            .unwrap();
        let middle = repo
            .issue_new_family_with_idle(1_001, 10_000, 10_000, SessionContext::default())
            .await
            .unwrap();
        let newest = repo
            .issue_new_family_with_idle(1_002, 10_000, 10_000, SessionContext::default())
            .await
            .unwrap();

        let evicted = repo.enforce_device_limit(1_003, 2).await.unwrap();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].family_id, oldest.record.family_id);
        assert!(repo.get(&oldest.record.token_id).await.unwrap().is_none());
        assert!(repo.get(&middle.record.token_id).await.unwrap().is_some());
        assert!(repo.get(&newest.record.token_id).await.unwrap().is_some());

        repo.burn_family(&middle.record.family_id).await.unwrap();
        repo.burn_family(&newest.record.family_id).await.unwrap();
    }

    #[tokio::test]
    async fn family_rename_and_revoke_round_trip_through_the_device_list() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = RefreshTokenRepository::new(pool);
        let issued = repo
            .issue_new_family_with_idle(1_000, 10_000, 10_000, SessionContext::default())
            .await
            .unwrap();

        let renamed = repo
            .rename_family(&issued.record.family_id, "My workstation", 1_100)
            .await
            .unwrap()
            .expect("family meta must exist");
        assert_eq!(renamed.device_name(), "My workstation");

        let listed = repo.list_active_families(1_200).await.unwrap();
        assert!(listed
            .iter()
            .any(|meta| meta.family_id == issued.record.family_id
                && meta.device_name() == "My workstation"));

        assert!(repo.revoke_family(&issued.record.family_id).await.unwrap());
        assert!(repo
            .get_family_meta(&issued.record.family_id)
            .await
            .unwrap()
            .is_none());
        assert!(!repo.revoke_family(&issued.record.family_id).await.unwrap());
    }
}
