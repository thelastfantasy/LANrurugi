//! Redis persistence for the subscription orchestrator (issue #55,
//! `specs/010-subscription-orchestrator/data-model.md`).
//!
//! Entirely additive: new key namespaces only, no legacy key read or written differently
//! (constitution Principle I). Lives on the `config` logical DB alongside the other non-legacy
//! stores (`plugin_options`, `download_queue`, `archive_split_suggestions`, ...).
//!
//! The one piece of domain logic worth knowing before reading further: a [`CycleOutcome`] can be
//! `Inconclusive`, and that is not a synonym for failure. On real sources a signed-out search
//! *succeeds* and simply returns a smaller result set. A cycle that ran without the expected
//! sign-in therefore learned something real but incomplete — and its candidate list must never be
//! treated as "everything that exists", because doing so would record works it never saw as handled
//! and hide them forever.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SubscriptionsError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
}

type Result<T> = std::result::Result<T, SubscriptionsError>;

fn subscription_key(id: &str) -> String {
    format!("LANRURUGI_SUBSCRIPTION_{id}")
}

fn cycles_key(subscription_id: &str) -> String {
    format!("LANRURUGI_SUBSCRIPTION_CYCLES_{subscription_id}")
}

fn seen_key(subscription_id: &str) -> String {
    format!("LANRURUGI_SUBSCRIPTION_SEEN_{subscription_id}")
}

fn reservation_key(id: &str) -> String {
    format!("LANRURUGI_RESERVATION_{id}")
}

const SUBSCRIPTION_INDEX: &str = "LANRURUGI_SUBSCRIPTIONS";
const RESERVATION_INDEX: &str = "LANRURUGI_RESERVATIONS";

/// How many past cycles to keep per subscription. History is for diagnosing "why didn't this
/// download?", which is a recent-past question — unbounded growth would cost more than it answers.
const CYCLE_HISTORY_LIMIT: isize = 50;

// ── Subscription ────────────────────────────────────────────────────────────────────────────────

/// Whether a subscription is checking, and if not, whose decision that was.
///
/// `Paused` is deliberately distinct from `Disabled`: the system paused it and owes the user a
/// reason and a way back, whereas `Disabled` is the user's own choice and needs neither.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SubscriptionState {
    Enabled,
    Disabled,
    Paused { reason: PauseReason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// A download failed for lack of source-side credit and this subscription is configured to stop
    /// rather than keep spending (FR-019).
    InsufficientCredit,
}

/// What a subscription does when a download fails because the source says there is not enough
/// credit. Reactive, not predictive: no source this project talks to exposes a balance to check
/// beforehand, so the only honest trigger is the failure itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreditPolicy {
    /// Stop checking until the user resumes.
    #[default]
    Pause,
    /// Keep checking; reserve whatever could not be downloaded.
    ContinueAndReserve,
}

/// What a subscription is looking for.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Criteria {
    /// A creator/uploader to follow, if that is how this subscription is scoped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    /// Tags that define the subscription (distinct from `Filters::required_tags`, which narrows an
    /// already-scoped result set).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// A feed or index page supplied by the user, for sources whose extension cannot search on its
    /// own behalf (FR-002c).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing_url: Option<String>,
}

/// Rules applied to candidates a source returned. Every field is a reason a candidate can be
/// rejected, and the matcher reports *which* one fired (FR-011) rather than a bare verdict.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Filters {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_rating: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub id: String,
    pub name: String,
    /// The extension namespace that discovers for this subscription.
    pub source: String,
    pub criteria: Criteria,
    #[serde(default)]
    pub filters: Filters,
    /// Seconds between checks. Enforced against the source's declared minimum at the API boundary —
    /// refused with a reason rather than silently raised (FR-004b), because a user who believes they
    /// set five minutes while getting sixty will misread every later result.
    pub interval_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_category: Option<String>,
    #[serde(default = "default_true")]
    pub enrich_metadata: bool,
    /// Off by default (FR-007a). A new subscription's rules are usually still being tuned, which is
    /// exactly when an over-broad rule would spend real credit unattended.
    #[serde(default)]
    pub auto_download: bool,
    #[serde(default)]
    pub credit_policy: CreditPolicy,
    pub state: SubscriptionState,
    /// Unix seconds. `None` means it has never run, which makes it due immediately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i64>,
    pub created_at: i64,
}

fn default_true() -> bool {
    true
}

impl Subscription {
    /// Whether a check is due. Deliberately a yes/no question rather than "how many intervals have
    /// elapsed" — that phrasing is what gives FR-008a's single catch-up for free: a subscription
    /// idle through ten missed intervals is due exactly once, not ten times.
    pub fn is_due(&self, now: i64) -> bool {
        if self.state != SubscriptionState::Enabled {
            return false;
        }
        match self.last_checked_at {
            None => true,
            Some(last) => now.saturating_sub(last) >= self.interval_secs as i64,
        }
    }
}

// ── Check cycle ─────────────────────────────────────────────────────────────────────────────────

/// How a check ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleOutcome {
    /// Ran with everything it needed; its candidate list is authoritative.
    Completed,
    /// Could not look at all (source unreachable, extension error).
    Failed { reason: String },
    /// Ran, but without the expected sign-in — so it saw a smaller world than the one asked about.
    ///
    /// The distinction from `Completed` is the whole reason this variant exists. Treating this as a
    /// completed check would let it record works as seen, permanently hiding everything it had no
    /// access to, with no error anywhere to reveal it (FR-002g/h).
    Inconclusive { reason: String },
}

impl CycleOutcome {
    /// Whether this cycle's findings may be recorded as "seen". Only a `Completed` cycle may.
    pub fn is_authoritative(&self) -> bool {
        matches!(self, CycleOutcome::Completed)
    }
}

/// What happened to one candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum CandidateVerdict {
    Queued,
    AwaitingApproval,
    Reserved {
        reason: String,
    },
    /// Carries which rule rejected it, so a too-strict subscription is diagnosable (FR-011).
    Rejected {
        rule: String,
    },
    AlreadyHeld,
    AlreadySeen,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateRecord {
    /// Normalised, so the same work under different URL forms is one work.
    pub source_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub verdict: CandidateVerdict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckCycle {
    pub id: String,
    pub subscription_id: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub outcome: CycleOutcome,
    pub candidates_seen: u32,
    #[serde(default)]
    pub candidates: Vec<CandidateRecord>,
}

// ── Reservation ─────────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservationStatus {
    Waiting,
    /// Kept rather than deleted: later cycles consult discarded entries so the same work is not
    /// re-reserved (FR-016). Deleting would make the user's decision evaporate.
    Discarded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationEntry {
    pub id: String,
    pub subscription_id: String,
    pub source_url: String,
    /// Why it could not be downloaded, in the user's terms.
    pub reason: String,
    pub status: ReservationStatus,
    pub created_at: i64,
}

// ── Repository ──────────────────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SubscriptionRepository {
    pool: Pool,
}

impl SubscriptionRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    // Subscriptions

    pub async fn save(&self, subscription: &Subscription) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = subscription_key(&subscription.id);
        let raw = serde_json::to_string(subscription)
            .map_err(|e| SubscriptionsError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        let _: () = conn.sadd(SUBSCRIPTION_INDEX, &subscription.id).await?;
        Ok(())
    }

    pub async fn get(&self, id: &str) -> Result<Option<Subscription>> {
        let mut conn = self.pool.get().await?;
        let key = subscription_key(id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| SubscriptionsError::Json(key, e)),
        }
    }

    pub async fn list_all(&self) -> Result<Vec<Subscription>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.smembers(SUBSCRIPTION_INDEX).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(s) = self.get(&id).await? {
                out.push(s);
            }
        }
        out.sort_by_key(|s| s.created_at);
        Ok(out)
    }

    /// Removes the subscription and everything that belonged only to it. Leaving its cycle history
    /// or seen-set behind would be the same class of orphan this project fixed for archives in
    /// issue #102.
    pub async fn delete(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(subscription_key(id)).await?;
        let _: () = conn.del(cycles_key(id)).await?;
        let _: () = conn.del(seen_key(id)).await?;
        let _: () = conn.srem(SUBSCRIPTION_INDEX, id).await?;
        Ok(())
    }

    // Cycle history

    pub async fn record_cycle(&self, cycle: &CheckCycle) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = cycles_key(&cycle.subscription_id);
        let raw =
            serde_json::to_string(cycle).map_err(|e| SubscriptionsError::Json(key.clone(), e))?;
        let _: () = conn.lpush(&key, raw).await?;
        let _: () = conn.ltrim(&key, 0, CYCLE_HISTORY_LIMIT - 1).await?;
        Ok(())
    }

    pub async fn recent_cycles(
        &self,
        subscription_id: &str,
        limit: isize,
    ) -> Result<Vec<CheckCycle>> {
        let mut conn = self.pool.get().await?;
        let key = cycles_key(subscription_id);
        let raws: Vec<String> = conn.lrange(&key, 0, limit - 1).await?;
        Ok(raws
            .iter()
            .filter_map(|r| serde_json::from_str(r).ok())
            .collect())
    }

    // Seen works

    /// Records works as seen. Callers MUST only reach here from an authoritative cycle — see
    /// [`CycleOutcome::is_authoritative`] and this module's own header for why.
    pub async fn mark_seen(&self, subscription_id: &str, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;
        let _: () = conn.sadd(seen_key(subscription_id), sources).await?;
        Ok(())
    }

    pub async fn is_seen(&self, subscription_id: &str, source: &str) -> Result<bool> {
        let mut conn = self.pool.get().await?;
        Ok(conn.sismember(seen_key(subscription_id), source).await?)
    }

    pub async fn seen_all(&self, subscription_id: &str) -> Result<Vec<String>> {
        let mut conn = self.pool.get().await?;
        Ok(conn.smembers(seen_key(subscription_id)).await?)
    }

    // Reservations

    pub async fn save_reservation(&self, entry: &ReservationEntry) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = reservation_key(&entry.id);
        let raw =
            serde_json::to_string(entry).map_err(|e| SubscriptionsError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        let _: () = conn.sadd(RESERVATION_INDEX, &entry.id).await?;
        Ok(())
    }

    pub async fn get_reservation(&self, id: &str) -> Result<Option<ReservationEntry>> {
        let mut conn = self.pool.get().await?;
        let key = reservation_key(id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| SubscriptionsError::Json(key, e)),
        }
    }

    pub async fn list_reservations(&self) -> Result<Vec<ReservationEntry>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.smembers(RESERVATION_INDEX).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(e) = self.get_reservation(&id).await? {
                out.push(e);
            }
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        Ok(out)
    }

    pub async fn delete_reservation(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(reservation_key(id)).await?;
        let _: () = conn.srem(RESERVATION_INDEX, id).await?;
        Ok(())
    }

    /// Drops the oldest entries past `max_entries`, returning how many went. Bounded growth is a
    /// requirement (FR-018); the caller is responsible for telling the user it happened, since an
    /// entry vanishing unexplained is indistinguishable from one that was never created.
    pub async fn prune_reservations(&self, max_entries: usize) -> Result<u32> {
        let all = self.list_reservations().await?;
        if all.len() <= max_entries {
            return Ok(0);
        }
        let mut pruned = 0u32;
        for entry in all.into_iter().skip(max_entries) {
            self.delete_reservation(&entry.id).await?;
            pruned += 1;
        }
        Ok(pruned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: &str, interval: u64, state: SubscriptionState, last: Option<i64>) -> Subscription {
        Subscription {
            id: id.to_string(),
            name: "test".to_string(),
            source: "discovery/example".to_string(),
            criteria: Criteria::default(),
            filters: Filters::default(),
            interval_secs: interval,
            target_category: None,
            enrich_metadata: true,
            auto_download: false,
            credit_policy: CreditPolicy::default(),
            state,
            last_checked_at: last,
            created_at: 0,
        }
    }

    #[test]
    fn a_subscription_that_has_never_run_is_due() {
        assert!(sub("a", 3600, SubscriptionState::Enabled, None).is_due(1_000));
    }

    #[test]
    fn a_subscription_is_due_once_its_interval_has_elapsed() {
        let s = sub("a", 3600, SubscriptionState::Enabled, Some(1_000));
        assert!(!s.is_due(1_000 + 3599));
        assert!(s.is_due(1_000 + 3600));
    }

    /// FR-008a: downtime collapses into one catch-up, not a backlog. Expressed here as "due is a
    /// yes/no question" — ten missed intervals and one missed interval are the same answer.
    #[test]
    fn missing_many_intervals_still_reports_due_once() {
        let s = sub("a", 3600, SubscriptionState::Enabled, Some(1_000));
        assert!(s.is_due(1_000 + 3600 * 10));
        assert!(s.is_due(1_000 + 3600));
    }

    #[test]
    fn a_disabled_or_paused_subscription_is_never_due() {
        assert!(!sub("a", 1, SubscriptionState::Disabled, None).is_due(i64::MAX));
        assert!(!sub(
            "a",
            1,
            SubscriptionState::Paused {
                reason: PauseReason::InsufficientCredit
            },
            None
        )
        .is_due(i64::MAX));
    }

    /// The guard this whole module's header is about: only a completed cycle may record works as
    /// seen. A degraded or failed cycle saw an incomplete world.
    #[test]
    fn only_a_completed_cycle_is_authoritative() {
        assert!(CycleOutcome::Completed.is_authoritative());
        assert!(!CycleOutcome::Inconclusive {
            reason: "not signed in".to_string()
        }
        .is_authoritative());
        assert!(!CycleOutcome::Failed {
            reason: "unreachable".to_string()
        }
        .is_authoritative());
    }

    #[test]
    fn auto_download_defaults_to_off_so_nothing_spends_credit_unattended() {
        let s = sub("a", 3600, SubscriptionState::Enabled, None);
        assert!(!s.auto_download);
    }
}
