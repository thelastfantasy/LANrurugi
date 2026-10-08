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

/// The subset of the seen set that was turned away by a *rule* rather than handled, and may therefore
/// be considered again — see [`SubscriptionRepository::mark_rejected`].
fn rejected_key(subscription_id: &str) -> String {
    format!("LANRURUGI_SUBSCRIPTION_REJECTED_{subscription_id}")
}

/// Which works the user blocked, and when — a hash so the list can be shown newest-first.
///
/// The seen set alone cannot answer "what did I block": a blocked work and a merely-handled one are
/// both simply seen, which is why the block action records itself separately even though the seen
/// mark is what actually stops the download.
fn blocked_key(subscription_id: &str) -> String {
    format!("LANRURUGI_SUBSCRIPTION_BLOCKED_{subscription_id}")
}

/// Epoch milliseconds, for ordering the blocked list. Local helper rather than a dependency on the
/// jobs crate's own clock: this is the only timestamp this file needs.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn reservation_key(id: &str) -> String {
    format!("LANRURUGI_RESERVATION_{id}")
}

const SUBSCRIPTION_INDEX: &str = "LANRURUGI_SUBSCRIPTIONS";
const RESERVATION_INDEX: &str = "LANRURUGI_RESERVATIONS";

/// One hash per subscription, fields keyed by canonical source: a snapshot is read and written whole
/// on every check, and `HGETALL`/`HSET` keep that one round trip each rather than one per work.
fn snapshot_key(subscription_id: &str) -> String {
    format!("LANRURUGI_SUBSCRIPTION_SNAPSHOT_{subscription_id}")
}

fn pending_key(id: &str) -> String {
    format!("LANRURUGI_SUBSCRIPTION_PENDING_{id}")
}

const PENDING_INDEX: &str = "LANRURUGI_SUBSCRIPTION_PENDING";

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

/// What to do when a work already in the snapshot changes at the source (title or tags).
///
/// A user decision rather than fixed behaviour: a subscription following one trusted creator and one
/// scraping a broad tag search warrant different answers, and no single default suits both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceChangePolicy {
    /// Record it and surface it. Nothing else.
    #[default]
    NotifyOnly,
    /// Also queue the held archive for metadata re-enrichment, so the change propagates through the
    /// existing metadata path. Never by copying snapshot values onto the archive — a listing row
    /// truncates its tag list, so writing it would delete real tags.
    RefetchMetadata,
    /// Update the snapshot silently. For a source whose tags churn.
    Ignore,
}

/// What to do when a snapshotted work is no longer in the listing.
///
/// No variant deletes a held archive. Removal has innocent causes — a tag edited off, a temporary
/// takedown, a throttled partial listing — and deletion is irreversible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceRemovalPolicy {
    /// Mark it; withdraw it if still awaiting approval. Held archives untouched.
    #[default]
    MarkOnly,
    /// As `MarkOnly`, plus surface it as needing attention.
    Notify,
    /// Forget it silently.
    Ignore,
}

/// What a subscription's downloads do when a filename collides with an archive the library already
/// has — the one conflict a download cannot resolve on its own.
///
/// A per-subscription answer rather than a global one because a subscription downloads unattended:
/// the default (`Ask`) parks the bytes in `temp_dir` for a decision, and a decision nobody is
/// present to make expires with them 24 hours later (see `PENDING_RENAME_MAX_AGE`). A subscription
/// whose rules are trusted can pre-answer instead.
///
/// Deliberately no "ask only when it matters" middle ground: the collision is either resolved by
/// this policy or handed to the user, and pretending to know which are worth asking about is how a
/// filter silently stops matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    /// Park it and ask (the default, and the only behavior before this existed).
    #[default]
    Ask,
    /// Catalogue it beside the existing archive, under `{name}_{crc}.{ext}` — the same derived name
    /// the rename popover's own default template produces, so an automatic and a manual resolution
    /// of the same collision land on the same filename.
    AutoRename,
    /// Replace the colliding archive. The same mechanism as the download plugin's own
    /// `overwrite_on_duplicate` option (and the global `replacedupe` setting), just chosen per
    /// subscription instead of per plugin.
    Overwrite,
    /// Drop it: no archive, no question. For a subscription whose criteria are broad enough that an
    /// occasional collision is expected noise.
    Discard,
}

/// Cookies and headers supplied by hand, for a source no login plugin covers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Credentials {
    /// `name=value` pairs, as a browser would send them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cookies: String,
    /// `Name: value` per line — some sources authenticate with a header rather than a cookie.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub headers: String,
}

impl Credentials {
    pub fn is_empty(&self) -> bool {
        self.cookies.trim().is_empty() && self.headers.trim().is_empty()
    }
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
    /// The condition a candidate must satisfy, as a tree.
    ///
    /// A tree rather than a flat list because real intent nests: "by this artist, AND (rated 4+ OR
    /// over 100 pages), AND NOT already-translated" cannot be written as one list of conditions joined
    /// by a single connective.
    ///
    /// `None` matches everything — a subscription with no conditions takes whatever its source lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Condition>,

    // ── Superseded by `condition`, kept so existing subscriptions keep working ──────────────────
    //
    // These were the original fixed filters, before conditions could be expressed over any field the
    // source provides. Each has an exact equivalent as a `FieldRule`, so they are migrated on read
    // (see `Filters::effective_condition`) rather than evaluated by a second code path: two
    // implementations of "does this candidate match" would eventually disagree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_rating: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_categories: Vec<String>,
}

/// One node of a subscription's condition tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Condition {
    /// Every child must hold.
    All { children: Vec<Condition> },
    /// At least one child must hold.
    Any { children: Vec<Condition> },
    /// The child must not hold. An exclusion, kept as its own node rather than as a per-rule flag so
    /// that a whole group can be negated, not just one comparison.
    Not { child: Box<Condition> },
    /// A comparison against one candidate field.
    Rule(FieldRule),
}

impl Filters {
    /// The condition to actually evaluate, with the superseded fixed filters folded in.
    ///
    /// Returns `None` only when there is genuinely nothing to check. The legacy filters become `Rule`
    /// nodes joined to `condition` by `All`, which is what they always meant: every one of them had to
    /// hold, and they had to hold alongside everything else.
    ///
    /// `excluded_categories` is deliberately **not** one of them: it compares against the categories
    /// the *library* already holds this work in, which is not a candidate field at all (see
    /// `lanrurugi_api::subscriptions::matcher::evaluate_outcome`, which keeps its own loop for it).
    /// Folding it in as a `category` rule would ask a different question — the *source's* own
    /// category — and, worse, a candidate whose source reports no category would come back
    /// "not yet answerable" instead of ever reaching that loop.
    pub fn effective_condition(&self) -> Option<Condition> {
        let mut parts: Vec<Condition> = Vec::new();

        if !self.required_tags.is_empty() {
            parts.push(Condition::Rule(FieldRule {
                field: "tags".into(),
                operator: FieldOperator::IncludesAll,
                value: RuleValue::List(self.required_tags.clone()),
            }));
        }
        if !self.excluded_tags.is_empty() {
            parts.push(Condition::Rule(FieldRule {
                field: "tags".into(),
                operator: FieldOperator::IncludesNone,
                value: RuleValue::List(self.excluded_tags.clone()),
            }));
        }
        if let Some(min) = self.minimum_rating {
            parts.push(Condition::Rule(FieldRule {
                field: "rating".into(),
                operator: FieldOperator::Gte,
                value: RuleValue::Number(min as f64),
            }));
        }
        if let Some(c) = &self.condition {
            parts.push(c.clone());
        }

        match parts.len() {
            0 => None,
            1 => parts.pop(),
            _ => Some(Condition::All { children: parts }),
        }
    }
}

/// Which candidate field a rule compares, with what operator.
///
/// The set of filterable fields is deliberately not stored alongside: it is derived, and a stored copy
/// would be a second separately-derived answer to the same question that would eventually disagree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldRule {
    /// The candidate key this compares, e.g. `posted_at`.
    pub field: String,
    pub operator: FieldOperator,
    pub value: RuleValue,
}

/// The operators the host implements. A rule cannot name one the host has no implementation for, and
/// each has a single implementation shared by every source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldOperator {
    Gte,
    Lte,
    Eq,
    /// Published at least `value` seconds ago. A duration rather than a timestamp: a subscription is a
    /// standing instruction, and "newer than a fixed date" would mean something different every day it
    /// ran and eventually match everything.
    OlderThan,
    /// Published within the last `value` seconds.
    NewerThan,
    Equals,
    Contains,
    In,
    NotIn,
    IncludesAll,
    IncludesNone,
    /// The field carries no values at all — a work with no language tag, say.
    ///
    /// Not expressible with the operators above: excluding specific values cannot say "none of this
    /// kind", and a work tagged in no language carries nothing to exclude.
    IsEmpty,
    /// It carries at least one value, whatever it is.
    IsNotEmpty,
    Is,
}

/// A rule's comparison value. Shaped by the operator rather than by the field, so one variant serves
/// every field of a given kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RuleValue {
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<String>),
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
    /// Tags to merge onto every archive this subscription downloads, in addition to whatever the
    /// metadata plugin supplies. Deduplicated against the archive's existing tags when applied.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub metadata_tags: Vec<String>,
    #[serde(default = "default_true")]
    pub enrich_metadata: bool,
    /// Off by default (FR-007a). A new subscription's rules are usually still being tuned, which is
    /// exactly when an over-broad rule would spend real credit unattended.
    #[serde(default)]
    pub auto_download: bool,
    #[serde(default)]
    pub credit_policy: CreditPolicy,
    /// What this subscription's downloads do about a filename collision — see [`ConflictPolicy`].
    #[serde(default)]
    pub conflict_policy: ConflictPolicy,
    /// Cookies and headers to send with this subscription's own checks, for a source whose login
    /// plugin is missing or that authenticates in a way no plugin covers.
    ///
    /// Folded in the same place a login plugin's own result goes, so the extension cannot tell the
    /// two apart. A per-subscription escape hatch, not a replacement: a login plugin refreshes
    /// itself, whereas a pasted cookie goes stale and the check silently sees less once it does.
    #[serde(default, skip_serializing_if = "Credentials::is_empty")]
    pub credentials: Credentials,
    #[serde(default)]
    pub on_source_changed: SourceChangePolicy,
    #[serde(default)]
    pub on_source_removed: SourceRemovalPolicy,
    pub state: SubscriptionState,
    /// Unix seconds. `None` means it has never run, which makes it due immediately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i64>,
    pub created_at: i64,
    /// When its settings were last written — creation counts as a write.
    ///
    /// Drives a short grace period before the first check (see [`SETTLE_SECS`]). Separate from
    /// `last_checked_at`, which says when it last *ran*: a rule saved a moment ago has not run at all,
    /// and that is precisely when running it is most likely to be unwanted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings_changed_at: Option<i64>,
}

/// How long a subscription waits after its settings are written before its first check.
///
/// Saving is not the same as being sure. An over-broad rule is usually noticed moments after it is
/// saved — by which time, without this, a check would already have queued downloads against it. A
/// minute is long enough to re-open the form and fix it, and short enough that nobody waits on it.
pub const SETTLE_SECS: i64 = 60;

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
        // Nothing runs while the settings are still settling, however overdue it otherwise is: the
        // window exists precisely for the case where what was just saved is not what was wanted.
        if let Some(changed) = self.settings_changed_at {
            if now.saturating_sub(changed) < SETTLE_SECS {
                return false;
            }
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
    /// The work itself is in the library. Carries that archive's id, so a reader can be sent to the
    /// copy that exists instead of back out to the site it came from.
    AlreadyHeld {
        archive_id: String,
    },
    /// An older revision of a work the library already holds, decided from the revision chain stored
    /// with that work — no download, and no E-Hentai round trip, which is the whole point of storing
    /// the chain. Names the newer member (source and archive id) for the same reason as `AlreadyHeld`.
    Superseded {
        newer_source: String,
        newer_archive_id: String,
    },
    AlreadySeen,
    /// Inside its `minimum_age_secs` waiting period. Deliberately not a rejection: it will be
    /// reconsidered on a later check, so it must not be recorded as seen.
    TooSoon,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateRecord {
    /// Normalised, so the same work under different URL forms is one work.
    pub source_url: String,
    /// By language, as the source supplied them — see `lanrurugi_plugin::protocol::Titles`.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub title: std::collections::HashMap<String, String>,
    /// Carried so a preview can show who posted a work and what it is tagged, which is most of what
    /// anyone judges a listing by. Absent for a source whose listing does not reveal them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploader: Option<String>,
    /// Source-side rating, if the listing carried one. Optional for the same reason as `uploader`:
    /// older records predate it, and not every source exposes a rating at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    /// Publication time as Unix seconds, already parsed from whatever timezone/spelling the source
    /// supplied. Optional for old records and sources that do not expose a date.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    pub verdict: CandidateVerdict,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

/// One work as a listing last showed it.
///
/// The cache that makes change detection possible at all. It cannot be folded into `Seen Work`:
/// that set is written with `SADD` and accumulates everything ever seen, whereas a diff needs *what
/// the last check showed* — `previous - current` is what disappeared, `current - previous` is new.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapshotEntry {
    /// Canonical source URL; the key within a subscription's snapshot.
    pub source: String,
    /// By language, as the source supplied them.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub title: std::collections::HashMap<String, String>,
    /// When the *source* says it was published — not when we first saw it. A work published years ago
    /// can enter a tag search today by being edited, which is why this is never used to decide what
    /// is new (that is `Seen Work` membership).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posted_at: Option<String>,
    /// **Truncated by the listing** (verified live: 19 of 25 e-hentai rows carried exactly 12 tags).
    /// Usable only to judge "does this still match the subscription" — never to write onto an
    /// archive, which would delete real tags.
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rating: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploader: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    /// When *we* first saw it, as distinct from `posted_at`.
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    /// Set only once the removal gating passes: an authoritative cycle plus several consecutive
    /// absences.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disappeared_at: Option<i64>,
    /// Consecutive checks without it, so one partial response cannot mark a whole page at once.
    #[serde(default)]
    pub missing_count: u32,
    /// Still listed, but its current tags no longer satisfy the subscription.
    #[serde(default)]
    pub no_longer_matching: bool,
    /// The previous title, kept so a change can be shown as old to new rather than silently applied.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub title_changed_from: std::collections::HashMap<String, String>,
}

/// A matched work waiting for the user's go-ahead.
///
/// Exists as its own record rather than being read back out of cycle history: history is an
/// append-only log bounded by age, so a pending item would silently expire out of it while still
/// being, from the user's point of view, an unanswered question. It also needs to be addressable on
/// its own — "approve this one" has to name something.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingApproval {
    pub id: String,
    pub subscription_id: String,
    /// Normalised, so approving it enqueues exactly the work that was matched.
    pub source_url: String,
    /// By language, as the source supplied them.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub title: std::collections::HashMap<String, String>,
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
        let _: () = conn.del(rejected_key(id)).await?;
        let _: () = conn.del(blocked_key(id)).await?;
        let _: () = conn.del(snapshot_key(id)).await?;
        let _: () = conn.srem(SUBSCRIPTION_INDEX, id).await?;
        drop(conn);
        // Its pending approvals go too — a question about a subscription that no longer exists is
        // unanswerable, and leaving them would be the same orphan class issue #102 dealt with.
        for entry in self
            .list_pending()
            .await?
            .into_iter()
            .filter(|e| e.subscription_id == id)
        {
            self.delete_pending(&entry.id).await?;
        }
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

    /// Remembers that these works were turned away by a rule, not handled.
    ///
    /// A rule reads values the source keeps changing: a rating rises as people vote, a work is edited
    /// to add a tag, a page count is filled in. Being in the seen set is permanent, so without this
    /// second set a work rejected for a rating of 3 could never be downloaded after it reached 4 —
    /// the one outcome the user cannot observe, since nothing reports a work that was never offered.
    /// A work that was instead *handled* (queued, approved, discarded) is deliberately absent here:
    /// that decision was the user's, and reconsidering it would re-offer what they already answered.
    pub async fn mark_rejected(&self, subscription_id: &str, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;
        let _: () = conn.sadd(rejected_key(subscription_id), sources).await?;
        Ok(())
    }

    /// Drops works from the reconsiderable set — because they were reconsidered and handled, or
    /// because the user asked for a fresh look at everything.
    pub async fn forget_rejected(&self, subscription_id: &str, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;
        let _: () = conn.srem(rejected_key(subscription_id), sources).await?;
        Ok(())
    }

    /// Every work this subscription may consider again. Read once per cycle, alongside the seen set.
    pub async fn rejected_all(&self, subscription_id: &str) -> Result<Vec<String>> {
        let mut conn = self.pool.get().await?;
        let members: Vec<String> = conn.smembers(rejected_key(subscription_id)).await?;
        Ok(members)
    }

    /// Settles works by the user's own decision: never downloaded again, and never reconsidered.
    ///
    /// This is the one rejection that is not the host's guess about a value that may change — it is an
    /// answer. Used by the preview's own "block" action. Deleting a downloaded archive has the same
    /// effect for the same reason: the work was taken and then turned down, so the seen mark it already
    /// carries stays and nothing needs recording here.
    ///
    /// `forget_seen` undoes it, deliberately (a block made in error, or a change of mind, are the same
    /// request).
    pub async fn block(&self, subscription_id: &str, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;
        let _: () = conn.sadd(seen_key(subscription_id), sources).await?;
        // Also drops any provisional rejection it carried: an answered "no" outranks a guessed one.
        let _: () = conn.srem(rejected_key(subscription_id), sources).await?;
        // Recorded so the block can be *listed* — and therefore undone — rather than being
        // indistinguishable from every other seen work.
        let at = now_ms().to_string();
        let fields: Vec<(&str, &str)> = sources.iter().map(|s| (s.as_str(), at.as_str())).collect();
        let _: () = conn
            .hset_multiple(blocked_key(subscription_id), &fields)
            .await?;
        Ok(())
    }

    /// Every work this subscription has blocked, newest first.
    pub async fn blocked_all(&self, subscription_id: &str) -> Result<Vec<(String, i64)>> {
        let mut conn = self.pool.get().await?;
        let raw: std::collections::HashMap<String, i64> =
            conn.hgetall(blocked_key(subscription_id)).await?;
        let mut out: Vec<(String, i64)> = raw.into_iter().collect();
        out.sort_by_key(|(_, at)| std::cmp::Reverse(*at));
        Ok(out)
    }

    /// Makes a subscription reconsider specific works on its next check.
    ///
    /// Needed because being seen is otherwise permanent, and two ordinary situations depend on undoing
    /// it: a work deleted from the library by mistake would never be fetched again, and widening a
    /// rule would not cause anything it previously turned away to be looked at afresh — those works
    /// were recorded as seen at the moment they were rejected.
    pub async fn forget_seen(&self, subscription_id: &str, sources: &[String]) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;
        let _: () = conn.srem(seen_key(subscription_id), sources).await?;
        let _: () = conn.srem(rejected_key(subscription_id), sources).await?;
        // A blocked work that is being reconsidered stops being blocked, by definition.
        let _: () = conn.hdel(blocked_key(subscription_id), sources).await?;
        Ok(())
    }

    /// Clears the whole seen set, so the next check treats the source as entirely new.
    ///
    /// Deliberately separate from `forget_seen`: on a long-running subscription this means the next
    /// check may match its entire back catalogue at once, which is a different order of consequence
    /// from reconsidering a few works and should be asked for explicitly.
    pub async fn forget_all_seen(&self, subscription_id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(seen_key(subscription_id)).await?;
        let _: () = conn.del(rejected_key(subscription_id)).await?;
        let _: () = conn.del(blocked_key(subscription_id)).await?;
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

    // Listing snapshots

    /// The whole snapshot for a subscription, keyed by canonical source.
    pub async fn snapshot(
        &self,
        subscription_id: &str,
    ) -> Result<std::collections::HashMap<String, SnapshotEntry>> {
        let mut conn = self.pool.get().await?;
        let key = snapshot_key(subscription_id);
        let raw: std::collections::HashMap<String, String> = conn.hgetall(&key).await?;
        let mut out = std::collections::HashMap::with_capacity(raw.len());
        for (source, value) in raw {
            // A single unparseable field must not make the whole snapshot unreadable — that would
            // turn a stale write into a total loss of change-detection for this subscription.
            match serde_json::from_str::<SnapshotEntry>(&value) {
                Ok(entry) => {
                    out.insert(source, entry);
                }
                Err(e) => {
                    tracing::warn!(%key, %source, error = %e, "dropping unparseable snapshot entry");
                }
            }
        }
        Ok(out)
    }

    /// Replaces the listed entries, leaving the rest of the snapshot alone. Entries are upserted
    /// rather than the hash being rewritten wholesale, so a work that this check did not see keeps
    /// its `missing_count` and `disappeared_at` instead of being resurrected as brand new.
    pub async fn save_snapshot_entries(
        &self,
        subscription_id: &str,
        entries: &[SnapshotEntry],
    ) -> Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;
        let key = snapshot_key(subscription_id);
        let mut pairs: Vec<(String, String)> = Vec::with_capacity(entries.len());
        for entry in entries {
            let raw = serde_json::to_string(entry)
                .map_err(|e| SubscriptionsError::Json(key.clone(), e))?;
            pairs.push((entry.source.clone(), raw));
        }
        let _: () = conn.hset_multiple(&key, &pairs).await?;
        Ok(())
    }

    pub async fn delete_snapshot_entries(
        &self,
        subscription_id: &str,
        sources: &[String],
    ) -> Result<()> {
        if sources.is_empty() {
            return Ok(());
        }
        let mut conn = self.pool.get().await?;
        let _: () = conn.hdel(snapshot_key(subscription_id), sources).await?;
        Ok(())
    }

    // Pending approvals

    pub async fn save_pending(&self, entry: &PendingApproval) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = pending_key(&entry.id);
        let raw =
            serde_json::to_string(entry).map_err(|e| SubscriptionsError::Json(key.clone(), e))?;
        let _: () = conn.set(&key, raw).await?;
        let _: () = conn.sadd(PENDING_INDEX, &entry.id).await?;
        Ok(())
    }

    pub async fn get_pending(&self, id: &str) -> Result<Option<PendingApproval>> {
        let mut conn = self.pool.get().await?;
        let key = pending_key(id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| SubscriptionsError::Json(key, e)),
        }
    }

    pub async fn list_pending(&self) -> Result<Vec<PendingApproval>> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.smembers(PENDING_INDEX).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            match self.get_pending(&id).await? {
                Some(e) => out.push(e),
                // The index outlived its record. Dropping it from the index here keeps the set from
                // growing without bound; silently skipping would leave the id to be re-read on
                // every later listing.
                None => {
                    let _: () = conn.srem(PENDING_INDEX, &id).await?;
                }
            }
        }
        out.sort_by_key(|e| std::cmp::Reverse(e.created_at));
        Ok(out)
    }

    pub async fn delete_pending(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.del(pending_key(id)).await?;
        let _: () = conn.srem(PENDING_INDEX, id).await?;
        Ok(())
    }

    /// Source URLs already awaiting approval for this subscription — consulted by a later cycle so
    /// the same work is not queued for approval twice, which would make the list grow every check
    /// until the user answers.
    pub async fn pending_sources(&self, subscription_id: &str) -> Result<Vec<String>> {
        Ok(self
            .list_pending()
            .await?
            .into_iter()
            .filter(|e| e.subscription_id == subscription_id)
            .map(|e| e.source_url)
            .collect())
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

    async fn test_pool() -> Option<Pool> {
        crate::test_support::test_pool().await
    }

    fn pending(id: &str, subscription_id: &str, url: &str) -> PendingApproval {
        PendingApproval {
            id: id.to_string(),
            subscription_id: subscription_id.to_string(),
            source_url: url.to_string(),
            title: std::collections::HashMap::new(),
            created_at: 0,
        }
    }

    /// The behaviour a user depends on: deleting a downloaded archive must NOT cause the subscription
    /// to fetch it again. Deletion deliberately does not touch the seen set, so this asserts the set
    /// keeps the record — the archive being gone is not evidence the user wants it back.
    #[tokio::test]
    async fn a_work_stays_seen_so_deleting_its_archive_does_not_refetch_it() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = SubscriptionRepository::new(pool);
        let source = "e-hentai.org/g/9001/deadbeef01".to_string();
        repo.mark_seen("sub-seen-1", std::slice::from_ref(&source))
            .await
            .unwrap();

        assert!(repo.is_seen("sub-seen-1", &source).await.unwrap());

        // Forgetting is the one way back, and it has to be asked for explicitly.
        repo.forget_seen("sub-seen-1", std::slice::from_ref(&source))
            .await
            .unwrap();
        assert!(!repo.is_seen("sub-seen-1", &source).await.unwrap());
    }

    /// The distinction the reconsideration feature rests on: a rejected work stays eligible to be
    /// looked at again, while a handled one does not.
    #[tokio::test]
    async fn a_rejected_work_can_be_reconsidered_but_a_handled_one_cannot() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = SubscriptionRepository::new(pool);
        let rejected = "e-hentai.org/g/9101/aaaaaaaaaa".to_string();
        let handled = "e-hentai.org/g/9102/bbbbbbbbbb".to_string();
        repo.mark_seen("sub-rejected-1", &[rejected.clone(), handled.clone()])
            .await
            .unwrap();
        repo.mark_rejected("sub-rejected-1", std::slice::from_ref(&rejected))
            .await
            .unwrap();

        let reconsiderable = repo.rejected_all("sub-rejected-1").await.unwrap();
        assert_eq!(reconsiderable, vec![rejected.clone()]);

        // Once it is reconsidered and taken, it is no longer a question to revisit.
        repo.forget_rejected("sub-rejected-1", std::slice::from_ref(&rejected))
            .await
            .unwrap();
        assert!(repo
            .rejected_all("sub-rejected-1")
            .await
            .unwrap()
            .is_empty());

        // And the explicit "reconsider these" request clears both marks.
        repo.mark_rejected("sub-rejected-1", std::slice::from_ref(&rejected))
            .await
            .unwrap();
        repo.forget_seen("sub-rejected-1", std::slice::from_ref(&rejected))
            .await
            .unwrap();
        assert!(repo
            .rejected_all("sub-rejected-1")
            .await
            .unwrap()
            .is_empty());
    }

    /// Clearing everything is a separate request from forgetting named works: on a long-running
    /// subscription it can match the whole back catalogue at once.
    #[tokio::test]
    async fn forgetting_everything_empties_the_set() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = SubscriptionRepository::new(pool);
        repo.mark_seen("sub-seen-2", &["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(repo.seen_all("sub-seen-2").await.unwrap().len(), 2);

        repo.forget_all_seen("sub-seen-2").await.unwrap();
        assert!(repo.seen_all("sub-seen-2").await.unwrap().is_empty());
    }

    /// The wire shape the frontend's own `Condition` union is written against. Asserted rather than
    /// assumed: a tagged enum's JSON is easy to change by accident (renaming a variant, adding
    /// `rename_all`), and the two would then disagree silently — the host would deserialize nothing
    /// and the subscription would match everything.
    #[test]
    fn condition_serialises_in_the_shape_the_frontend_expects() {
        let c = Condition::All {
            children: vec![
                Condition::Rule(FieldRule {
                    field: "rating".into(),
                    operator: FieldOperator::Gte,
                    value: RuleValue::Number(4.0),
                }),
                Condition::Not {
                    child: Box::new(Condition::Any { children: vec![] }),
                },
            ],
        };
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["kind"], "all");
        assert_eq!(json["children"][0]["kind"], "rule");
        assert_eq!(json["children"][0]["field"], "rating");
        assert_eq!(json["children"][0]["operator"], "gte");
        assert_eq!(json["children"][0]["value"], 4.0);
        assert_eq!(json["children"][1]["kind"], "not");
        assert_eq!(json["children"][1]["child"]["kind"], "any");

        // And back, so the host can read what the frontend sends.
        let back: Condition = serde_json::from_value(json).unwrap();
        assert_eq!(back, c);
    }

    /// The superseded fixed filters must fold into the tree, not be evaluated by a second path.
    #[test]
    fn legacy_filters_fold_into_one_all_node() {
        let f = Filters {
            required_tags: vec!["artist:foo".into()],
            minimum_rating: Some(4.0),
            ..Filters::default()
        };
        let Some(Condition::All { children }) = f.effective_condition() else {
            panic!("expected an All node");
        };
        assert_eq!(children.len(), 2);

        // Nothing set at all means nothing to check, not an empty node that matches nothing.
        assert_eq!(Filters::default().effective_condition(), None);
    }

    #[tokio::test]
    async fn round_trips_a_pending_approval_and_deletes_it() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = SubscriptionRepository::new(pool);
        let entry = pending("pa-rt-1", "sub-rt-1", "https://example.test/g/1");
        repo.save_pending(&entry).await.unwrap();

        assert_eq!(repo.get_pending("pa-rt-1").await.unwrap(), Some(entry));
        assert!(repo
            .list_pending()
            .await
            .unwrap()
            .iter()
            .any(|e| e.id == "pa-rt-1"));

        repo.delete_pending("pa-rt-1").await.unwrap();
        assert_eq!(repo.get_pending("pa-rt-1").await.unwrap(), None);
        assert!(!repo
            .list_pending()
            .await
            .unwrap()
            .iter()
            .any(|e| e.id == "pa-rt-1"));
    }

    /// The guard that stops the approval list growing by one copy of the same work every cycle.
    #[tokio::test]
    async fn pending_sources_reports_what_is_already_awaiting_approval() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = SubscriptionRepository::new(pool);
        let entry = pending("pa-ps-1", "sub-ps-1", "https://example.test/g/2");
        repo.save_pending(&entry).await.unwrap();

        let sources = repo.pending_sources("sub-ps-1").await.unwrap();
        assert!(sources.iter().any(|s| s == "https://example.test/g/2"));
        // Scoped to its own subscription: another subscription may legitimately still want the work.
        assert!(repo
            .pending_sources("sub-ps-other")
            .await
            .unwrap()
            .is_empty());

        repo.delete_pending("pa-ps-1").await.unwrap();
    }

    /// An index entry whose record is gone must not survive the listing that observed it, or the set
    /// grows without bound and every later listing re-reads the same dead id.
    #[tokio::test]
    async fn listing_drops_an_index_entry_whose_record_vanished() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = SubscriptionRepository::new(pool.clone());
        let mut conn = pool.get().await.unwrap();
        let _: () = conn.sadd(PENDING_INDEX, "pa-ghost").await.unwrap();

        let listed = repo.list_pending().await.unwrap();
        assert!(!listed.iter().any(|e| e.id == "pa-ghost"));

        let ids: Vec<String> = conn.smembers(PENDING_INDEX).await.unwrap();
        assert!(!ids.iter().any(|i| i == "pa-ghost"));
    }

    /// Deleting a subscription must take its unanswered questions with it — otherwise the approval
    /// list keeps offering works whose rule no longer exists.
    #[tokio::test]
    async fn deleting_a_subscription_clears_its_pending_approvals() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = SubscriptionRepository::new(pool);
        let s = sub("sub-del-1", 3600, SubscriptionState::Enabled, None);
        repo.save(&s).await.unwrap();
        repo.save_pending(&pending(
            "pa-del-1",
            "sub-del-1",
            "https://example.test/g/3",
        ))
        .await
        .unwrap();

        repo.delete("sub-del-1").await.unwrap();
        assert_eq!(repo.get_pending("pa-del-1").await.unwrap(), None);
    }

    fn sub(id: &str, interval: u64, state: SubscriptionState, last: Option<i64>) -> Subscription {
        Subscription {
            id: id.to_string(),
            name: "test".to_string(),
            source: "discovery/example".to_string(),
            criteria: Criteria::default(),
            filters: Filters::default(),
            interval_secs: interval,
            target_category: None,
            metadata_tags: Vec::new(),
            enrich_metadata: true,
            auto_download: false,
            credit_policy: CreditPolicy::default(),
            conflict_policy: ConflictPolicy::default(),
            credentials: Credentials::default(),
            settings_changed_at: None,
            on_source_changed: SourceChangePolicy::default(),
            on_source_removed: SourceRemovalPolicy::default(),
            state,
            last_checked_at: last,
            created_at: 0,
        }
    }

    /// Saving is not the same as being sure. A rule written moments ago is the one most likely to be
    /// wrong, and without this the first check would already have queued downloads against it.
    #[test]
    fn a_freshly_saved_subscription_waits_before_its_first_check() {
        let mut s = sub("a", 3600, SubscriptionState::Enabled, None);
        s.settings_changed_at = Some(1_000);

        assert!(!s.is_due(1_000), "not the instant it was saved");
        assert!(
            !s.is_due(1_000 + SETTLE_SECS - 1),
            "nor just before the window closes"
        );
        assert!(s.is_due(1_000 + SETTLE_SECS), "but once it has settled");
    }

    /// Editing restarts the window too: changing a rule is as likely to be a mistake as writing one,
    /// and the check that follows is just as unattended.
    #[test]
    fn editing_restarts_the_settle_window_even_when_long_overdue() {
        let mut s = sub("a", 3600, SubscriptionState::Enabled, Some(0));
        s.settings_changed_at = Some(100_000);

        // Overdue by a wide margin on the interval alone, yet held back by the recent edit.
        assert!(!s.is_due(100_030));
        assert!(s.is_due(100_000 + SETTLE_SECS));
    }

    /// A subscription stored before this field existed has no window to observe, and must not be
    /// held back forever by its absence.
    #[test]
    fn a_subscription_without_a_recorded_edit_is_unaffected() {
        let s = sub("a", 3600, SubscriptionState::Enabled, None);
        assert_eq!(s.settings_changed_at, None);
        assert!(s.is_due(1_000));
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
