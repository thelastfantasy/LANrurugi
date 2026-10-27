//! Shares, recipient claims and recipient tokens (specs/017-archive-sharing data-model.md).
//!
//! A share is a Redis hash. `downloads_used` is only ever changed by the claim script, so the
//! ordinary save path never writes it and an edit cannot race a download into a lost count.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use deadpool_redis::redis::{self, AsyncCommands};
use deadpool_redis::Pool;
use lanrurugi_core::ids::{ArchiveId, ClaimId, ShareId, TankId};
use lanrurugi_share_proto::messages::Permissions;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::keys;

#[derive(Debug, Error)]
pub enum ShareStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed share data in {0:?}: {1}")]
    Malformed(String, String),
}

type Result<T> = std::result::Result<T, ShareStorageError>;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShareItem {
    Archive { id: ArchiveId },
    Tankoubon { id: TankId },
}

impl ShareItem {
    pub fn id_str(&self) -> &str {
        match self {
            Self::Archive { id } => id.as_str(),
            Self::Tankoubon { id } => id.as_str(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareStatus {
    Active,
    Exhausted,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Share {
    pub id: ShareId,
    pub title: Option<String>,
    /// Hex `access_hash` of the code.
    pub code_hash: String,
    pub short_code: bool,
    /// Hex of the code sealed under the node secret, for re-display.
    pub code_enc: String,
    /// Hex pkarr public key derived from the code.
    pub lookup_pubkey: String,
    /// Hex record key derived from the code, for republishing.
    pub record_key: String,
    pub items: Vec<ShareItem>,
    pub permissions: Permissions,
    pub download_limit: Option<u32>,
    pub downloads_used: u32,
    pub ip_allow: Vec<String>,
    pub ip_first_n: Option<u32>,
    pub expires_at: Option<i64>,
    pub embed: bool,
    pub allow_helpers: bool,
    pub helpers_disabled_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub created_at: i64,
    pub created_by: String,
}

/// Short codes are only allowed with an expiry this close (FR-008).
pub const SHORT_CODE_MAX_LIFETIME_SECS: i64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ShareValidationError {
    #[error("select at least one permission")]
    NoPermissions,
    #[error("a share needs at least one item")]
    NoItems,
    #[error("not an IP address or range: {0}")]
    BadIpRule(String),
    #[error("limits must be at least 1")]
    BadLimit,
    #[error("a short code needs an expiry within 7 days")]
    ShortCodeNeedsExpiry,
}

impl Share {
    /// Revoked, then expired, then exhausted, then active.
    pub fn status(&self, now: i64) -> ShareStatus {
        if self.revoked_at.is_some() {
            ShareStatus::Revoked
        } else if self.expires_at.is_some_and(|t| now >= t) {
            ShareStatus::Expired
        } else if self
            .download_limit
            .is_some_and(|limit| self.downloads_used >= limit)
        {
            ShareStatus::Exhausted
        } else {
            ShareStatus::Active
        }
    }

    pub fn has_ip_restriction(&self) -> bool {
        !self.ip_allow.is_empty() || self.ip_first_n.is_some()
    }

    /// Whether `ip` passes the allow-list. The first-N rule needs Redis and is checked by
    /// `ShareRepository::admit_address`.
    pub fn ip_allowed(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        self.ip_allow.is_empty()
            || self.ip_allow.iter().any(|rule| match parse_ip_rule(rule) {
                Some(IpRule::Addr(a)) => canonical_ip(a) == ip,
                Some(IpRule::Net(n)) => n.contains(&ip),
                None => false,
            })
    }

    pub fn validate(&self) -> std::result::Result<(), ShareValidationError> {
        if self.permissions.is_empty() {
            return Err(ShareValidationError::NoPermissions);
        }
        if self.items.is_empty() {
            return Err(ShareValidationError::NoItems);
        }
        if let Some(bad) = self.ip_allow.iter().find(|r| parse_ip_rule(r).is_none()) {
            return Err(ShareValidationError::BadIpRule(bad.clone()));
        }
        if self.download_limit == Some(0) || self.ip_first_n == Some(0) {
            return Err(ShareValidationError::BadLimit);
        }
        if self.short_code
            && !self
                .expires_at
                .is_some_and(|t| t <= self.created_at + SHORT_CODE_MAX_LIFETIME_SECS)
        {
            return Err(ShareValidationError::ShortCodeNeedsExpiry);
        }
        Ok(())
    }

    fn to_fields(&self) -> (Vec<(&'static str, String)>, Vec<&'static str>) {
        let mut set = vec![
            ("id", self.id.to_string()),
            ("code_hash", self.code_hash.clone()),
            ("short_code", bool_field(self.short_code)),
            ("code_enc", self.code_enc.clone()),
            ("lookup_pubkey", self.lookup_pubkey.clone()),
            ("record_key", self.record_key.clone()),
            ("items", json(&self.items)),
            ("permissions", json(&self.permissions)),
            ("ip_allow", json(&self.ip_allow)),
            ("embed", bool_field(self.embed)),
            ("allow_helpers", bool_field(self.allow_helpers)),
            ("created_at", self.created_at.to_string()),
            ("created_by", self.created_by.clone()),
        ];
        let mut unset = Vec::new();
        let mut optional = |name: &'static str, value: Option<String>| match value {
            Some(v) => set.push((name, v)),
            None => unset.push(name),
        };
        optional("title", self.title.clone());
        optional("download_limit", self.download_limit.map(|v| v.to_string()));
        optional("ip_first_n", self.ip_first_n.map(|v| v.to_string()));
        optional("expires_at", self.expires_at.map(|v| v.to_string()));
        optional(
            "helpers_disabled_at",
            self.helpers_disabled_at.map(|v| v.to_string()),
        );
        // `revoked_at` is deliberately absent: only `revoke()` writes it, one-way. Saving a whole
        // record (an edit loaded before a revoke, saved after it) must not be able to clear it.
        (set, unset)
    }

    fn from_fields(key: &str, f: &HashMap<String, String>) -> Result<Self> {
        let bad = |what: &str| ShareStorageError::Malformed(key.to_string(), what.to_string());
        let text = |name: &str| f.get(name).cloned().ok_or_else(|| bad(name));
        let num = |name: &str| -> Result<Option<i64>> {
            f.get(name)
                .map(|v| v.parse().map_err(|_| bad(name)))
                .transpose()
        };
        let small = |name: &str| -> Result<Option<u32>> {
            f.get(name)
                .map(|v| v.parse().map_err(|_| bad(name)))
                .transpose()
        };
        let parse_json = |name: &str| -> Result<serde_json::Value> {
            serde_json::from_str(&text(name)?).map_err(|_| bad(name))
        };
        Ok(Self {
            id: ShareId(text("id")?),
            title: f.get("title").cloned(),
            code_hash: text("code_hash")?,
            short_code: f.get("short_code").is_some_and(|v| v == "1"),
            code_enc: text("code_enc")?,
            lookup_pubkey: text("lookup_pubkey")?,
            record_key: text("record_key")?,
            items: serde_json::from_value(parse_json("items")?).map_err(|_| bad("items"))?,
            permissions: serde_json::from_value(parse_json("permissions")?)
                .map_err(|_| bad("permissions"))?,
            download_limit: small("download_limit")?,
            downloads_used: small("downloads_used")?.unwrap_or(0),
            ip_allow: serde_json::from_value(parse_json("ip_allow")?)
                .map_err(|_| bad("ip_allow"))?,
            ip_first_n: small("ip_first_n")?,
            expires_at: num("expires_at")?,
            embed: f.get("embed").is_some_and(|v| v == "1"),
            allow_helpers: f.get("allow_helpers").is_some_and(|v| v == "1"),
            helpers_disabled_at: num("helpers_disabled_at")?,
            revoked_at: num("revoked_at")?,
            created_at: num("created_at")?.ok_or_else(|| bad("created_at"))?,
            created_by: f.get("created_by").cloned().unwrap_or_default(),
        })
    }
}

enum IpRule {
    Addr(IpAddr),
    Net(ipnet::IpNet),
}

fn parse_ip_rule(rule: &str) -> Option<IpRule> {
    let rule = rule.trim();
    rule.parse::<IpAddr>()
        .map(IpRule::Addr)
        .or_else(|_| rule.parse::<ipnet::IpNet>().map(IpRule::Net))
        .ok()
}

/// IPv4-mapped IPv6 addresses compare as the IPv4 address they carry.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

fn bool_field(v: bool) -> String {
    if v { "1" } else { "0" }.to_string()
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).expect("share fields always serialise")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimKind {
    BrowserDownload,
    Import,
}

impl ClaimKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::BrowserDownload => "browser_download",
            Self::Import => "import",
        }
    }
}

/// Why the claim script refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimRefusal {
    NotFound,
    Revoked,
    Expired,
    Exhausted,
    PermissionDenied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// A new claim was created and counted.
    Created(ClaimId),
    /// The claim already existed; nothing was counted again.
    Existing(ClaimId),
    Refused(ClaimRefusal),
}

/// One entry of a share's claim history (FR-005).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimHistoryEntry {
    pub claim_id: ClaimId,
    pub kind: ClaimKind,
    pub first_address: String,
    pub started_at: i64,
}

/// Checks status, permission, the first-N set and the limit, then creates the claim and counts it,
/// all in one step so two last-download claims can never both succeed (FR-015). The allow-list is
/// matched in Rust beforehand and passed in as ARGV[4].
const CLAIM_SCRIPT: &str = r#"
local share, ips, claim, history, importers = KEYS[1], KEYS[2], KEYS[3], KEYS[4], KEYS[5]
local now, kind, addr, ip_ok = tonumber(ARGV[1]), ARGV[2], ARGV[3], ARGV[4]
local member, ttl, endpoint = ARGV[5], tonumber(ARGV[6]), ARGV[7]
if redis.call('EXISTS', share) == 0 then return 'not_found' end
if redis.call('HEXISTS', share, 'revoked_at') == 1 then return 'revoked' end
local expires = redis.call('HGET', share, 'expires_at')
if expires and now >= tonumber(expires) then return 'expired' end
local perms = cjson.decode(redis.call('HGET', share, 'permissions'))
if (kind == 'import' and not perms.import) or (kind == 'browser_download' and not perms.download) then
  return 'permission_denied'
end
if ip_ok ~= '1' then return 'not_found' end
local function note_importer()
  if kind == 'import' and endpoint ~= '' then redis.call('SADD', importers, endpoint) end
end
if redis.call('EXISTS', claim) == 1 then
  local addrs = cjson.decode(redis.call('HGET', claim, 'addresses'))
  local seen = false
  for _, a in ipairs(addrs) do if a == addr then seen = true end end
  if not seen then
    table.insert(addrs, addr)
    redis.call('HSET', claim, 'addresses', cjson.encode(addrs))
  end
  redis.call('HSET', claim, 'last_active_at', now)
  redis.call('EXPIRE', claim, ttl)
  note_importer()
  return 'existing'
end
local first_n = redis.call('HGET', share, 'ip_first_n')
if first_n and redis.call('SISMEMBER', ips, addr) == 0
   and redis.call('SCARD', ips) >= tonumber(first_n) then
  return 'not_found'
end
local limit = redis.call('HGET', share, 'download_limit')
local used = tonumber(redis.call('HGET', share, 'downloads_used') or '0')
if limit and used >= tonumber(limit) then return 'exhausted' end
if first_n then redis.call('SADD', ips, addr) end
redis.call('HSET', claim, 'share_id', redis.call('HGET', share, 'id'), 'kind', kind,
  'addresses', cjson.encode({addr}), 'endpoint_id', endpoint,
  'started_at', now, 'last_active_at', now)
redis.call('EXPIRE', claim, ttl)
redis.call('ZADD', history, now, member)
redis.call('HINCRBY', share, 'downloads_used', 1)
note_importer()
return 'created'
"#;

/// Admits an address under `ip_first_n`: already admitted, or a free slot left.
const ADMIT_SCRIPT: &str = r#"
local first_n = redis.call('HGET', KEYS[1], 'ip_first_n')
if not first_n then return 1 end
if redis.call('SISMEMBER', KEYS[2], ARGV[1]) == 1 then return 1 end
if redis.call('SCARD', KEYS[2]) >= tonumber(first_n) then return 0 end
redis.call('SADD', KEYS[2], ARGV[1])
return 1
"#;

#[derive(Clone)]
pub struct ShareRepository {
    pool: Pool,
}

impl ShareRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn create(&self, share: &Share) -> Result<()> {
        let key = keys::share_key(&share.id);
        let (set, _) = share.to_fields();
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.hset_multiple(&key, &set).ignore();
        pipe.hset(&key, "downloads_used", share.downloads_used)
            .ignore();
        pipe.zadd(keys::SHARES_INDEX_KEY, share.id.as_str(), share.created_at)
            .ignore();
        pipe.set(keys::share_by_code_key(&share.code_hash), share.id.as_str())
            .ignore();
        for item in &share.items {
            pipe.sadd(keys::share_item_refs_key(item.id_str()), share.id.as_str())
                .ignore();
        }
        let mut conn = self.pool.get().await?;
        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    pub async fn get(&self, id: &ShareId) -> Result<Option<Share>> {
        let key = keys::share_key(id);
        let mut conn = self.pool.get().await?;
        let fields: HashMap<String, String> = conn.hgetall(&key).await?;
        if fields.is_empty() {
            return Ok(None);
        }
        Share::from_fields(&key, &fields).map(Some)
    }

    pub async fn by_code_hash(&self, code_hash: &str) -> Result<Option<Share>> {
        let mut conn = self.pool.get().await?;
        let id: Option<String> = conn.get(keys::share_by_code_key(code_hash)).await?;
        match id {
            Some(id) => self.get(&ShareId(id)).await,
            None => Ok(None),
        }
    }

    /// Every share, newest first.
    pub async fn list(&self) -> Result<Vec<Share>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.zrevrange(keys::SHARES_INDEX_KEY, 0, -1).await?;
        drop(conn);
        let mut shares = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(share) = self.get(&ShareId(id)).await? {
                shares.push(share);
            }
        }
        Ok(shares)
    }

    /// Ids of shares that reference an archive or tankoubon.
    pub async fn shares_referencing(&self, item_id: &str) -> Result<Vec<ShareId>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.smembers(keys::share_item_refs_key(item_id)).await?;
        Ok(ids.into_iter().map(ShareId).collect())
    }

    /// Writes every field except `downloads_used`, and keeps the item reverse index in step.
    pub async fn save(&self, share: &Share, previous_items: &[ShareItem]) -> Result<()> {
        let key = keys::share_key(&share.id);
        let (set, unset) = share.to_fields();
        let now_items: HashSet<&ShareItem> = share.items.iter().collect();
        let before: HashSet<&ShareItem> = previous_items.iter().collect();
        let mut pipe = redis::pipe();
        pipe.atomic();
        pipe.hset_multiple(&key, &set).ignore();
        if !unset.is_empty() {
            pipe.hdel(&key, unset).ignore();
        }
        for gone in before.difference(&now_items) {
            pipe.srem(keys::share_item_refs_key(gone.id_str()), share.id.as_str())
                .ignore();
        }
        for added in now_items.difference(&before) {
            pipe.sadd(keys::share_item_refs_key(added.id_str()), share.id.as_str())
                .ignore();
        }
        let mut conn = self.pool.get().await?;
        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    /// Lowering a limit never touches what was already used.
    pub async fn revoke(&self, id: &ShareId, now: i64) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.hset(keys::share_key(id), "revoked_at", now).await?;
        Ok(())
    }

    /// Runs the first-N admission for browsing and token checks. `true` when the address may use
    /// the share.
    pub async fn admit_address(&self, share: &Share, address: IpAddr) -> Result<bool> {
        if share.ip_first_n.is_none() {
            return Ok(true);
        }
        let mut conn = self.pool.get().await?;
        let admitted: i64 = redis::Script::new(ADMIT_SCRIPT)
            .key(keys::share_key(&share.id))
            .key(keys::share_ips_key(&share.id))
            .arg(canonical_ip(address).to_string())
            .invoke_async(&mut conn)
            .await?;
        Ok(admitted == 1)
    }

    /// Creates or refreshes a claim (research §9). `claim_id` is reused when the caller already
    /// holds one, so a resumed or retried download is never counted twice.
    pub async fn claim(
        &self,
        share: &Share,
        kind: ClaimKind,
        address: IpAddr,
        claim_id: Option<ClaimId>,
        endpoint_id: Option<&str>,
        now: i64,
    ) -> Result<ClaimOutcome> {
        let address = canonical_ip(address);
        let claim_id = claim_id.unwrap_or_else(ClaimId::generate);
        let history = ClaimHistoryEntry {
            claim_id: claim_id.clone(),
            kind,
            first_address: address.to_string(),
            started_at: now,
        };
        let mut conn = self.pool.get().await?;
        let result: String = redis::Script::new(CLAIM_SCRIPT)
            .key(keys::share_key(&share.id))
            .key(keys::share_ips_key(&share.id))
            .key(keys::share_claim_key(&claim_id))
            .key(keys::share_claims_index_key(&share.id))
            .key(keys::share_importers_key(&share.id))
            .arg(now)
            .arg(kind.as_str())
            .arg(address.to_string())
            .arg(if share.ip_allowed(address) { "1" } else { "0" })
            .arg(json(&history))
            .arg(keys::SHARE_CLAIM_TTL_SECS)
            .arg(endpoint_id.unwrap_or(""))
            .invoke_async(&mut conn)
            .await?;
        Ok(match result.as_str() {
            "created" => ClaimOutcome::Created(claim_id),
            "existing" => ClaimOutcome::Existing(claim_id),
            "revoked" => ClaimOutcome::Refused(ClaimRefusal::Revoked),
            "expired" => ClaimOutcome::Refused(ClaimRefusal::Expired),
            "exhausted" => ClaimOutcome::Refused(ClaimRefusal::Exhausted),
            "permission_denied" => ClaimOutcome::Refused(ClaimRefusal::PermissionDenied),
            _ => ClaimOutcome::Refused(ClaimRefusal::NotFound),
        })
    }

    /// Whether a claim is still alive (it expires 24 h after its last activity).
    pub async fn claim_exists(&self, claim_id: &ClaimId) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        Ok(conn.exists(keys::share_claim_key(claim_id)).await?)
    }

    /// Claim history, newest first, at most `limit` entries.
    pub async fn claim_history(
        &self,
        id: &ShareId,
        limit: isize,
    ) -> Result<Vec<ClaimHistoryEntry>> {
        let mut conn = self.pool.get().await?;
        let members: Vec<String> = conn
            .zrevrange(keys::share_claims_index_key(id), 0, limit - 1)
            .await?;
        Ok(members
            .iter()
            .filter_map(|m| serde_json::from_str(m).ok())
            .collect())
    }
}

/// A registered helper of one share (sharer side).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperInfo {
    pub endpoint_id: String,
    pub relay_urls: Vec<String>,
    pub addrs: Vec<String>,
    /// Sharer archive ids the helper holds.
    pub archives: Vec<String>,
    #[serde(default)]
    pub last_seen: i64,
}

impl ShareRepository {
    /// Whether `endpoint` ever made an import claim on the share.
    pub async fn is_importer(&self, id: &ShareId, endpoint: &str) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        Ok(conn
            .sismember(keys::share_importers_key(id), endpoint)
            .await?)
    }

    pub async fn register_helper(&self, id: &ShareId, info: &HelperInfo) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let mut pipe = redis::pipe();
        pipe.atomic()
            .zadd(
                keys::share_helpers_key(id),
                &info.endpoint_id,
                info.last_seen,
            )
            .ignore()
            .hset(
                keys::share_helper_info_key(id),
                &info.endpoint_id,
                json(info),
            )
            .ignore();
        pipe.query_async::<()>(&mut conn).await?;
        Ok(())
    }

    /// Records a heartbeat; `None` when the endpoint never registered.
    pub async fn helper_heartbeat(
        &self,
        id: &ShareId,
        endpoint: &str,
        now: i64,
    ) -> Result<Option<HelperInfo>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.hget(keys::share_helper_info_key(id), endpoint).await?;
        let Some(mut info) = raw.and_then(|r| serde_json::from_str::<HelperInfo>(&r).ok()) else {
            return Ok(None);
        };
        info.last_seen = now;
        let _: () = conn
            .zadd(keys::share_helpers_key(id), endpoint, now)
            .await?;
        Ok(Some(info))
    }

    /// Helpers heard from at or after `since`, most recent first.
    pub async fn helpers(&self, id: &ShareId, since: i64) -> Result<Vec<HelperInfo>> {
        let mut conn = self.pool.get().await?;
        let seen: Vec<(String, i64)> = conn
            .zrevrangebyscore_withscores(keys::share_helpers_key(id), "+inf", since)
            .await?;
        let mut out = Vec::with_capacity(seen.len());
        for (endpoint, last_seen) in seen {
            let raw: Option<String> = conn
                .hget(keys::share_helper_info_key(id), &endpoint)
                .await?;
            if let Some(mut info) = raw.and_then(|r| serde_json::from_str::<HelperInfo>(&r).ok()) {
                info.last_seen = last_seen;
                out.push(info);
            }
        }
        Ok(out)
    }
}

/// What a browser recipient's token resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecipientToken {
    pub share_id: ShareId,
    pub claim_id: Option<ClaimId>,
    pub address: String,
}

/// Opaque bearer tokens for browser recipients, stored by SHA-256 with a 12 h sliding TTL. A token
/// of a revoked share is useless without being deleted: every request re-checks the share.
#[derive(Clone)]
pub struct RecipientTokenRepository {
    pool: Pool,
}

fn token_digest(raw: &str) -> String {
    Sha256::digest(raw.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl RecipientTokenRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Returns the raw token, shown to the recipient only.
    pub async fn issue(&self, share_id: &ShareId, address: IpAddr) -> Result<String> {
        let bytes: [u8; 32] = rand::random();
        let raw: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let record = RecipientToken {
            share_id: share_id.clone(),
            claim_id: None,
            address: canonical_ip(address).to_string(),
        };
        let mut conn = self.pool.get().await?;
        let _: () = conn
            .set_ex(
                keys::share_rtoken_key(&token_digest(&raw)),
                json(&record),
                keys::SHARE_RTOKEN_TTL_SECS as u64,
            )
            .await?;
        Ok(raw)
    }

    /// Resolves and slides the TTL.
    pub async fn resolve(&self, raw: &str) -> Result<Option<RecipientToken>> {
        if raw.len() != 64 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let key = keys::share_rtoken_key(&token_digest(raw));
        let mut conn = self.pool.get().await?;
        let value: Option<String> = conn.get(&key).await?;
        let Some(value) = value else {
            return Ok(None);
        };
        let _: () = conn.expire(&key, keys::SHARE_RTOKEN_TTL_SECS).await?;
        serde_json::from_str(&value)
            .map(Some)
            .map_err(|e| ShareStorageError::Malformed(key, e.to_string()))
    }

    pub async fn attach_claim(&self, raw: &str, claim_id: &ClaimId) -> Result<()> {
        let Some(mut record) = self.resolve(raw).await? else {
            return Ok(());
        };
        record.claim_id = Some(claim_id.clone());
        let mut conn = self.pool.get().await?;
        let _: () = conn
            .set_ex(
                keys::share_rtoken_key(&token_digest(raw)),
                json(&record),
                keys::SHARE_RTOKEN_TTL_SECS as u64,
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample(now: i64) -> Share {
        Share {
            id: ShareId::generate(),
            title: Some("t".into()),
            code_hash: format!("{:064x}", rand::random::<u64>()),
            short_code: false,
            code_enc: "00".into(),
            lookup_pubkey: "00".into(),
            record_key: "00".into(),
            items: vec![ShareItem::Archive {
                id: ArchiveId::from("a1"),
            }],
            permissions: Permissions::ALL,
            download_limit: None,
            downloads_used: 0,
            ip_allow: vec![],
            ip_first_n: None,
            expires_at: None,
            embed: false,
            allow_helpers: false,
            helpers_disabled_at: None,
            revoked_at: None,
            created_at: now,
            created_by: "test".into(),
        }
    }

    #[test]
    fn status_follows_revoked_expired_exhausted_active() {
        let mut share = sample(100);
        assert_eq!(share.status(100), ShareStatus::Active);
        share.download_limit = Some(1);
        share.downloads_used = 1;
        assert_eq!(share.status(100), ShareStatus::Exhausted);
        share.expires_at = Some(150);
        assert_eq!(share.status(150), ShareStatus::Expired);
        share.revoked_at = Some(120);
        assert_eq!(share.status(110), ShareStatus::Revoked);
    }

    #[test]
    fn allow_list_matches_addresses_and_ranges() {
        let mut share = sample(0);
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(share.ip_allowed(ip("8.8.8.8")));
        share.ip_allow = vec!["192.168.1.0/24".into(), "2001:db8::1".into()];
        assert!(share.ip_allowed(ip("192.168.1.77")));
        assert!(share.ip_allowed(ip("::ffff:192.168.1.5")));
        assert!(share.ip_allowed(ip("2001:db8::1")));
        assert!(!share.ip_allowed(ip("192.168.2.1")));
    }

    #[test]
    fn validation_rejects_bad_shares() {
        let mut share = sample(0);
        assert!(share.validate().is_ok());
        share.permissions = Permissions::default();
        assert_eq!(share.validate(), Err(ShareValidationError::NoPermissions));
        share.permissions = Permissions::ALL;
        share.ip_allow = vec!["nope".into()];
        assert!(matches!(
            share.validate(),
            Err(ShareValidationError::BadIpRule(_))
        ));
        share.ip_allow.clear();
        share.short_code = true;
        assert_eq!(
            share.validate(),
            Err(ShareValidationError::ShortCodeNeedsExpiry)
        );
        share.expires_at = Some(SHORT_CODE_MAX_LIFETIME_SECS + 1);
        assert_eq!(
            share.validate(),
            Err(ShareValidationError::ShortCodeNeedsExpiry)
        );
        share.expires_at = Some(SHORT_CODE_MAX_LIFETIME_SECS);
        assert!(share.validate().is_ok());
        share.download_limit = Some(0);
        assert_eq!(share.validate(), Err(ShareValidationError::BadLimit));
    }

    #[test]
    fn fields_round_trip() {
        let mut share = sample(5);
        share.download_limit = Some(3);
        share.expires_at = Some(99);
        let (set, _) = share.to_fields();
        let mut map: HashMap<String, String> =
            set.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        map.insert("downloads_used".into(), "0".into());
        assert_eq!(Share::from_fields("k", &map).unwrap(), share);
    }
}

// ---------------------------------------------------------------------------------------------
// Receiving side: one import of another instance's share (data-model.md "ShareImport").

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportArchiveState {
    Pending,
    Transferring,
    Ingesting,
    Done,
    Reused,
    Failed,
}

impl ImportArchiveState {
    pub fn is_settled(self) -> bool {
        matches!(self, Self::Done | Self::Reused | Self::Failed)
    }
}

/// One archive of an import, keyed by the sharer's archive id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportArchive {
    pub sharer_archive_id: String,
    pub title: String,
    #[serde(default)]
    pub blake3: Option<String>,
    pub size: u64,
    pub state: ImportArchiveState,
    #[serde(default)]
    pub bytes_done: u64,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub local_archive_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportState {
    #[default]
    Transferring,
    Rebuilding,
    Done,
    Failed,
}

/// The helper side's view of one helped share (data-model.md "Help state transitions").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelpState {
    #[default]
    Inactive,
    Active,
    Paused,
    Ended,
}

/// After this long without a heartbeat answer, a helper shows the sharer as offline.
pub const HELP_OFFLINE_AFTER_SECS: i64 = 15 * 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShareImport {
    pub id: lanrurugi_core::ids::ShareImportId,
    /// The `lrrg:` text form: reconnects after a restart, and stays so the import can show it.
    #[serde(default)]
    pub code: Option<String>,
    pub sharer_endpoint_id: String,
    pub share_id: String,
    pub claim_id: String,
    pub title: String,
    /// The manifest as fetched at confirmation (`ShareManifest`, claim form).
    pub manifest: serde_json::Value,
    #[serde(default)]
    pub category: Option<String>,
    /// Put each archive into categories named like the sharer's, creating missing ones.
    #[serde(default)]
    pub inherit_categories: bool,
    pub queue_item_id: String,
    pub archives: Vec<ImportArchive>,
    pub concurrency: u32,
    #[serde(default)]
    pub help_deliver: bool,
    #[serde(default)]
    pub help_hidden: bool,
    #[serde(default)]
    pub help_state: HelpState,
    /// `exhausted`, `revoked`, `expired` or `helping_disabled`, while paused or ended.
    #[serde(default)]
    pub help_reason: Option<String>,
    /// The sharer-reported time of the last state change.
    #[serde(default)]
    pub help_state_at: Option<i64>,
    #[serde(default)]
    pub help_last_contact_at: Option<i64>,
    /// Sharer archive ids no longer in the share; never served.
    #[serde(default)]
    pub help_removed_archives: Vec<String>,
    #[serde(default)]
    pub help_bytes_served: u64,
    #[serde(default)]
    pub state: ImportState,
    #[serde(default)]
    pub error: Option<String>,
    /// Set while the sharer has paused uploads: unix seconds it said to come back, or 0 when it
    /// gave no time (FR-044).
    #[serde(default)]
    pub paused_until: Option<i64>,
    pub created_at: i64,
}

impl ShareImport {
    /// The help state shown to the administrator: `sharer_offline` is derived from the last
    /// contact rather than stored.
    pub fn shown_help_state(&self, now: i64, offline_after: i64) -> &'static str {
        match self.help_state {
            HelpState::Inactive => "inactive",
            HelpState::Ended => "ended",
            _ if self
                .help_last_contact_at
                .is_some_and(|t| now - t > offline_after) =>
            {
                "sharer_offline"
            }
            HelpState::Active => "active",
            HelpState::Paused => "paused",
        }
    }

    /// Whether this instance serves ranges of the share right now.
    pub fn serves_help(&self) -> bool {
        self.help_deliver
            && !self.help_hidden
            && self.state == ImportState::Done
            && self.help_state == HelpState::Active
    }
}

/// Imports are written as whole JSON documents: one task owns each import, so there is no
/// concurrent field-level writer to race.
#[derive(Clone)]
pub struct ShareImportRepository {
    pool: Pool,
}

impl ShareImportRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn save(&self, import: &ShareImport) -> Result<()> {
        let key = keys::share_import_key(&import.id);
        let mut conn = self.pool.get().await?;
        let _: () = conn.set(&key, json(import)).await?;
        let _: () = conn
            .zadd(
                keys::SHARE_IMPORTS_INDEX_KEY,
                import.id.as_str(),
                import.created_at,
            )
            .await?;
        Ok(())
    }

    pub async fn get(
        &self,
        id: &lanrurugi_core::ids::ShareImportId,
    ) -> Result<Option<ShareImport>> {
        let key = keys::share_import_key(id);
        let mut conn = self.pool.get().await?;
        let raw: Option<String> = conn.get(&key).await?;
        raw.map(|r| {
            serde_json::from_str(&r).map_err(|e| ShareStorageError::Malformed(key, e.to_string()))
        })
        .transpose()
    }

    /// Newest first.
    pub async fn list(&self) -> Result<Vec<ShareImport>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.zrevrange(keys::SHARE_IMPORTS_INDEX_KEY, 0, -1).await?;
        drop(conn);
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(import) = self.get(&lanrurugi_core::ids::ShareImportId(id)).await? {
                out.push(import);
            }
        }
        Ok(out)
    }
}
