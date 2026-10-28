//! Local error-telemetry store (specs/018-error-telemetry-feedback data-model.md).
//!
//! Everything here is additive: new keys on the `config` logical DB, nothing legacy is read or
//! written. An event is a hash holding its projection as JSON plus the fields the range and level
//! queries need, indexed by a sorted set; an issue is a hash keyed by fingerprint that outlives its
//! events, so the list keeps its history after eviction.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::keys;

#[derive(Debug, Error)]
pub enum DiagnosticsStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, DiagnosticsStorageError>;

pub mod key {
    pub const EVENT: &str = "LANRURUGI_DIAGNOSTIC_EVENT:";
    pub const EVENTS: &str = "LANRURUGI_DIAGNOSTICS_EVENTS";
    pub const EVENT_ISSUE: &str = "LANRURUGI_DIAGNOSTIC_EVENT_ISSUE:";
    pub const ISSUE: &str = "LANRURUGI_DIAGNOSTIC_ISSUE:";
    pub const ISSUES: &str = "LANRURUGI_DIAGNOSTICS_ISSUES";
    pub const BYTES: &str = "LANRURUGI_DIAGNOSTICS_BYTES";
    pub const DROPS: &str = "LANRURUGI_DIAGNOSTICS_DROPS";
}

/// An event's id: the envelope's own `event_id` when it had one, otherwise generated. Keeping the
/// SDK's id is what lets a replayed envelope dedupe on the tracker.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiagnosticEventId(pub String);

impl DiagnosticEventId {
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().simple().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for DiagnosticEventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The grouping key. A hex digest, so it is also usable as a URL path segment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Fingerprint(pub String);

impl Fingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

impl Level {
    pub fn from_wire(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "info" | "debug" => Self::Info,
            "warning" | "warn" => Self::Warning,
            _ => Self::Error,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    /// Higher is more severe, for "the highest level this issue has seen".
    pub fn rank(&self) -> u8 {
        match self {
            Self::Info => 0,
            Self::Warning => 1,
            Self::Error => 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Web,
    Server,
    Plugin,
}

impl Source {
    pub fn from_wire(value: &str) -> Self {
        match value {
            "javascript" | "web" => Self::Web,
            "plugin" => Self::Plugin,
            _ => Self::Server,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Server => "server",
            Self::Plugin => "plugin",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Breadcrumb {
    pub at: i64,
    pub category: String,
    pub level: String,
    pub message: String,
}

/// One captured occurrence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticEvent {
    pub id: DiagnosticEventId,
    pub fingerprint: Fingerprint,
    /// Server clock, unix milliseconds. The ordering key.
    pub at: i64,
    /// The browser's own clock when it disagrees with the server, unix milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_at: Option<i64>,
    pub level: Level,
    pub source: Source,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default)]
    pub tags: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub breadcrumbs: Vec<Breadcrumb>,
    /// The envelope exactly as received, gzipped — what a later replay sends to a tracker.
    #[serde(skip)]
    pub envelope: Vec<u8>,
    /// Stored size of the whole record, for the quota counter.
    #[serde(default)]
    pub bytes: u64,
}

impl DiagnosticEvent {
    /// The bound the spec sets on one event (FR-006), everything included.
    pub const MAX_BYTES: usize = 64 * 1024;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueState {
    Open,
    Resolved,
    Ignored,
}

impl IssueState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Resolved => "resolved",
            Self::Ignored => "ignored",
        }
    }

    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "open" => Some(Self::Open),
            "resolved" => Some(Self::Resolved),
            "ignored" => Some(Self::Ignored),
            _ => None,
        }
    }
}

/// One deterministic group of events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticIssue {
    pub fingerprint: Fingerprint,
    pub title: String,
    pub level: Level,
    pub count: u64,
    pub first_seen: i64,
    pub last_seen: i64,
    #[serde(default)]
    pub releases: Vec<String>,
    #[serde(default)]
    pub sources: Vec<Source>,
    pub state: IssueState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample_event_id: Option<DiagnosticEventId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl DiagnosticIssue {
    pub fn new(event: &DiagnosticEvent) -> Self {
        Self {
            fingerprint: event.fingerprint.clone(),
            title: event.message.lines().next().unwrap_or("").to_string(),
            level: event.level,
            count: 1,
            first_seen: event.at,
            last_seen: event.at,
            releases: event.release.iter().cloned().collect(),
            sources: vec![event.source],
            state: IssueState::Open,
            state_at: None,
            sample_event_id: Some(event.id.clone()),
            request_id: event.request_id.clone(),
        }
    }

    /// Folds a new occurrence in. A resolved or ignored issue reopens: a regression is not a
    /// decision to hide.
    pub fn record(&mut self, event: &DiagnosticEvent) {
        self.count = self.count.saturating_add(1);
        self.last_seen = self.last_seen.max(event.at);
        self.first_seen = self.first_seen.min(event.at);
        if event.level.rank() > self.level.rank() {
            self.level = event.level;
        }
        if let Some(release) = &event.release {
            if !self.releases.contains(release) {
                self.releases.push(release.clone());
                if self.releases.len() > 20 {
                    self.releases.remove(0);
                }
            }
        }
        if !self.sources.contains(&event.source) {
            self.sources.push(event.source);
        }
        if self.sample_event_id.is_none() {
            self.sample_event_id = Some(event.id.clone());
        }
        if self.request_id.is_none() {
            self.request_id = event.request_id.clone();
        }
        if self.state != IssueState::Open {
            self.state = IssueState::Open;
            self.state_at = Some(event.at);
        }
    }
}

/// What the settings page reports about the store.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub events: u64,
    pub issues: u64,
    pub bytes: u64,
}

/// The retention limits an eviction pass enforces, in the order they are applied.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    pub days: u64,
    pub max_events: u64,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Drops {
    pub retention: u64,
    pub rate_limit: u64,
    pub too_large: u64,
    pub store_error: u64,
}

#[derive(Clone)]
pub struct DiagnosticsRepo {
    pool: Pool,
}

impl DiagnosticsRepo {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    /// Stores one event and folds it into its issue. Returns the resulting issue.
    pub async fn save_event(&self, event: &DiagnosticEvent) -> Result<DiagnosticIssue> {
        let mut conn = self.pool.get().await?;
        let body = serde_json::to_vec(event)
            .map_err(|e| DiagnosticsStorageError::Json(key::EVENT.to_string(), e))?;
        let record_key = format!("{}{}", key::EVENT, event.id);
        let issue_key = format!("{}{}", key::ISSUE, event.fingerprint);
        let link_key = format!("{}{}", key::EVENT_ISSUE, event.id);
        let size = body.len() as u64 + record_key.len() as u64;

        let mut pipe = deadpool_redis::redis::pipe();
        pipe.cmd("HSET")
            .arg(&record_key)
            .arg(("json", &body))
            .arg(("fp", event.fingerprint.as_str()))
            .arg(("ts", event.at))
            .arg(("level", event.level.as_str()))
            .arg(("source", event.source.as_str()))
            .arg(("bytes", size))
            .ignore()
            .cmd("ZADD")
            .arg(key::EVENTS)
            .arg(event.at)
            .arg(event.id.as_str())
            .ignore()
            .cmd("SET")
            .arg(&link_key)
            .arg(event.fingerprint.as_str())
            .ignore()
            .cmd("INCRBY")
            .arg(key::BYTES)
            .arg(size)
            .ignore();
        let _: () = pipe.query_async(&mut conn).await?;

        let existing: Option<Vec<u8>> = conn.hget(&issue_key, "json").await?;
        let issue = match existing {
            Some(raw) => {
                let mut issue: DiagnosticIssue = serde_json::from_slice(&raw)
                    .map_err(|e| DiagnosticsStorageError::Json(issue_key.clone(), e))?;
                issue.record(event);
                issue
            }
            None => DiagnosticIssue::new(event),
        };
        let encoded = serde_json::to_vec(&issue)
            .map_err(|e| DiagnosticsStorageError::Json(issue_key.clone(), e))?;
        let mut pipe = deadpool_redis::redis::pipe();
        pipe.cmd("HSET")
            .arg(&issue_key)
            .arg("json")
            .arg(&encoded)
            .ignore()
            .cmd("ZADD")
            .arg(key::ISSUES)
            .arg(issue.last_seen)
            .arg(issue.fingerprint.as_str())
            .ignore();
        let _: () = pipe.query_async(&mut conn).await?;
        Ok(issue)
    }

    pub async fn event(&self, id: &DiagnosticEventId) -> Result<Option<DiagnosticEvent>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<Vec<u8>> = conn.hget(format!("{}{}", key::EVENT, id), "json").await?;
        match raw {
            Some(raw) => Ok(Some(serde_json::from_slice(&raw).map_err(|e| {
                DiagnosticsStorageError::Json(format!("{}{}", key::EVENT, id), e)
            })?)),
            None => Ok(None),
        }
    }

    /// Events whose timestamp falls in `from..=to`, oldest first, at most `limit`.
    pub async fn events_between(
        &self,
        from: i64,
        to: i64,
        limit: usize,
    ) -> Result<Vec<DiagnosticEvent>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn
            .zrangebyscore_limit(key::EVENTS, from, to, 0, limit as isize)
            .await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            let raw: Option<Vec<u8>> = conn.hget(format!("{}{}", key::EVENT, id), "json").await?;
            if let Some(raw) = raw {
                out.push(serde_json::from_slice(&raw).map_err(|e| {
                    DiagnosticsStorageError::Json(format!("{}{}", key::EVENT, id), e)
                })?);
            }
        }
        Ok(out)
    }

    /// The most recent events belonging to one issue, newest first.
    pub async fn events_for_issue(
        &self,
        fingerprint: &Fingerprint,
        limit: usize,
    ) -> Result<Vec<DiagnosticEvent>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.zrevrange(key::EVENTS, 0, -1).await?;
        let mut out = Vec::new();
        for id in ids {
            if out.len() >= limit {
                break;
            }
            let raw: Option<Vec<u8>> = conn.hget(format!("{}{}", key::EVENT, id), "json").await?;
            let Some(raw) = raw else { continue };
            let event: DiagnosticEvent = serde_json::from_slice(&raw)
                .map_err(|e| DiagnosticsStorageError::Json(format!("{}{}", key::EVENT, id), e))?;
            if &event.fingerprint == fingerprint {
                out.push(event);
            }
        }
        Ok(out)
    }

    pub async fn issue(&self, fingerprint: &Fingerprint) -> Result<Option<DiagnosticIssue>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<Vec<u8>> = conn
            .hget(format!("{}{}", key::ISSUE, fingerprint), "json")
            .await?;
        match raw {
            Some(raw) => Ok(Some(serde_json::from_slice(&raw).map_err(|e| {
                DiagnosticsStorageError::Json(format!("{}{}", key::ISSUE, fingerprint), e)
            })?)),
            None => Ok(None),
        }
    }

    /// Issues by last-seen, newest first. Filtering and paging happen in the API layer over this
    /// window; the number of distinct issues is small next to the number of events.
    pub async fn issues(&self, limit: usize) -> Result<Vec<DiagnosticIssue>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.zrevrange(key::ISSUES, 0, limit as isize - 1).await?;
        let mut out = Vec::with_capacity(ids.len());
        for fp in ids {
            let raw: Option<Vec<u8>> = conn.hget(format!("{}{}", key::ISSUE, fp), "json").await?;
            if let Some(raw) = raw {
                out.push(serde_json::from_slice(&raw).map_err(|e| {
                    DiagnosticsStorageError::Json(format!("{}{}", key::ISSUE, fp), e)
                })?);
            }
        }
        Ok(out)
    }

    pub async fn set_issue_state(
        &self,
        fingerprint: &Fingerprint,
        state: IssueState,
        at: i64,
    ) -> Result<Option<DiagnosticIssue>> {
        let Some(mut issue) = self.issue(fingerprint).await? else {
            return Ok(None);
        };
        issue.state = state;
        issue.state_at = Some(at);
        let encoded = serde_json::to_vec(&issue).map_err(|e| {
            DiagnosticsStorageError::Json(format!("{}{}", key::ISSUE, fingerprint), e)
        })?;
        let mut conn = self.pool.get().await?;
        let _: () = conn
            .hset(format!("{}{}", key::ISSUE, fingerprint), "json", encoded)
            .await?;
        Ok(Some(issue))
    }

    pub async fn usage(&self) -> Result<Usage> {
        let mut conn = self.pool.get().await?;
        let events: u64 = conn.zcard(key::EVENTS).await?;
        let issues: u64 = conn.zcard(key::ISSUES).await?;
        let bytes: u64 = conn.get(key::BYTES).await?;
        Ok(Usage {
            events,
            issues,
            bytes,
        })
    }

    pub async fn drops(&self) -> Drops {
        let Ok(mut conn) = self.pool.get().await else {
            return Drops::default();
        };
        let values: std::collections::HashMap<String, u64> =
            conn.hgetall(key::DROPS).await.unwrap_or_default();
        Drops {
            retention: *values.get("retention").unwrap_or(&0),
            rate_limit: *values.get("rate_limit").unwrap_or(&0),
            too_large: *values.get("too_large").unwrap_or(&0),
            store_error: *values.get("store_error").unwrap_or(&0),
        }
    }

    pub async fn bump_drop(&self, kind: &str) {
        let Ok(mut conn) = self.pool.get().await else {
            return;
        };
        let _: Result<i64> = conn.hincr(key::DROPS, kind, 1).await.map_err(Into::into);
    }

    /// Deletes one event and its index entries, decrementing the byte counter. The issue survives.
    pub async fn delete_event(&self, id: &DiagnosticEventId) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let record_key = format!("{}{}", key::EVENT, id);
        let bytes: u64 = conn.hget(&record_key, "bytes").await.unwrap_or(0);
        let mut pipe = deadpool_redis::redis::pipe();
        pipe.cmd("DEL")
            .arg(&record_key)
            .ignore()
            .cmd("DEL")
            .arg(format!("{}{}", key::EVENT_ISSUE, id))
            .ignore()
            .cmd("ZREM")
            .arg(key::EVENTS)
            .arg(id.as_str())
            .ignore();
        let _: () = pipe.query_async(&mut conn).await?;
        if bytes > 0 {
            let _: Result<i64> = conn.decr(key::BYTES, bytes).await.map_err(Into::into);
        }
        Ok(())
    }

    /// The three-step eviction from the data model: age, then count, then bytes. Returns how many
    /// events went away. Issues are never removed here.
    pub async fn evict(&self, retention: Retention, now: i64) -> Result<u64> {
        let mut conn = self.pool.get().await?;
        let mut removed = 0_u64;

        if retention.days > 0 {
            let cutoff = now - (retention.days as i64).saturating_mul(86_400_000);
            let expired: Vec<String> = conn.zrangebyscore(key::EVENTS, "-inf", cutoff).await?;
            for id in expired {
                self.delete_event(&DiagnosticEventId(id)).await?;
                removed += 1;
            }
        }

        loop {
            let count: u64 = conn.zcard(key::EVENTS).await?;
            if retention.max_events == 0 || count <= retention.max_events {
                break;
            }
            let oldest: Vec<String> = conn.zrange(key::EVENTS, 0, 0).await?;
            let Some(id) = oldest.into_iter().next() else {
                break;
            };
            self.delete_event(&DiagnosticEventId(id)).await?;
            removed += 1;
        }

        loop {
            let bytes: u64 = conn.get(key::BYTES).await?;
            if retention.max_bytes == 0 || bytes <= retention.max_bytes {
                break;
            }
            let oldest: Vec<String> = conn.zrange(key::EVENTS, 0, 0).await?;
            let Some(id) = oldest.into_iter().next() else {
                break;
            };
            self.delete_event(&DiagnosticEventId(id)).await?;
            removed += 1;
        }

        if removed > 0 {
            let _: Result<i64> = conn
                .hincr(key::DROPS, "retention", removed)
                .await
                .map_err(Into::into);
        }
        Ok(removed)
    }

    /// Which issue an event belonged to.
    pub async fn event_issue(&self, id: &str) -> Result<Option<Fingerprint>> {
        let mut conn = self.pool.get().await?;
        let fp: Option<String> = conn.get(format!("{}{}", key::EVENT_ISSUE, id)).await?;
        Ok(fp.map(Fingerprint))
    }

    /// The configuration keys this module owns, for `settings.rs`-style enumeration.
    pub fn config_key() -> &'static str {
        keys::CONFIG_KEY
    }
}
