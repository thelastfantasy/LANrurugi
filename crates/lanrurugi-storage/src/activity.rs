//! Redis persistence for the operator activity log (issue #87) — a real, structured, persisted
//! record of *who did what*, distinct from the existing `tracing`-based request log
//! (`lanrurugi_api::procedure::trace_request`), which only captures method/path/operator/allowed
//! at the auth-middleware layer and can't say what was actually changed, doesn't cover every
//! mutating endpoint, and has no query/filter/retention story of its own.
//!
//! Structural template: [`crate::api_tokens`] (same `thiserror` error enum shape, same `Pool`
//! field, same `LANRURUGI_TEST_REDIS_URL`-gated test convention, same real-Redis-`EXPIRE`
//! retention model instead of an application-level sweep) plus [`crate::compare_cache`]'s
//! insertion-order sorted set for the time-range query this module additionally needs.
//!
//! On the **`config`** logical DB, same placement as `api_tokens`/`download_queue`/
//! `compare_cache` — a new, purely additive namespace, not part of the legacy key surface.

use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::Pool;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ActivityStorageError {
    #[error("Redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    #[error("failed to get a pooled Redis connection: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("malformed JSON in Redis key {0:?}: {1}")]
    Json(String, #[source] serde_json::Error),
    #[error("malformed cursor {0:?}")]
    Cursor(String),
}

type Result<T> = std::result::Result<T, ActivityStorageError>;

/// A namespaced string ("archive.delete", "settings.update", "scanner.ingest", ...) rather than a
/// closed `enum` — the activity log is an append-only, long-retained record; a new action type
/// added later must never fail to deserialize an already-stored older entry the way an `enum`
/// with an unrecognized variant would. The facets endpoint also hands these strings straight back
/// to the frontend as filter values, so there's no separate enum-to-display-name table to keep in
/// sync either.
pub type ActionType = String;

/// One constant per write site — keeps every call to [`ActivityRepository::append`] (via
/// `lanrurugi_api::activity::record_manual`/`record_automatic`) using an identical string instead
/// of a hand-typed literal that could drift by a typo between two call sites for "the same"
/// action.
pub mod action_types {
    pub const ARCHIVE_DELETE: &str = "archive.delete";
    pub const ARCHIVE_RENAME: &str = "archive.rename";
    pub const ARCHIVE_METADATA_UPDATE: &str = "archive.metadata_update";
    /// A `PUT /archives/{id}/metadata` call whose only real effect was adding/changing/removing
    /// the `rating:` tag (title/summary unchanged, no other tag touched) — the Library page's
    /// right-click star widget's own write path (`useLibrary.ts::updateRating`). Split out from
    /// `ARCHIVE_METADATA_UPDATE` per direct feedback that a rating change reading as a generic
    /// "updated metadata"/tag-diff entry buried the fact that it was *just* a rating, not a real
    /// edit — see `archives.rs::update_archive_metadata`'s own detection logic.
    pub const ARCHIVE_RATING_UPDATE: &str = "archive.rating_update";
    pub const ARCHIVE_PATCH_DELETE: &str = "archive.patch_delete";
    pub const ARCHIVE_THUMB_REGEN: &str = "archive.thumb_regen";
    pub const ARCHIVE_UPLOAD: &str = "archive.upload";
    pub const SETTINGS_UPDATE: &str = "settings.update";
    pub const SETTINGS_PASSWORD_CHANGE: &str = "settings.password_change";
    pub const CATEGORY_CREATE: &str = "category.create";
    pub const CATEGORY_DELETE: &str = "category.delete";
    pub const TOKEN_CREATE: &str = "token.create";
    pub const TOKEN_REVOKE: &str = "token.revoke";
    pub const TOKEN_RENAME: &str = "token.rename";
    pub const DOWNLOAD_QUEUE_ADD: &str = "download_queue.add";
    pub const DOWNLOAD_QUEUE_START: &str = "download_queue.start";
    pub const DOWNLOAD_QUEUE_STOP: &str = "download_queue.stop";
    pub const DOWNLOAD_QUEUE_OVERWRITE: &str = "download_queue.overwrite";
    pub const DOWNLOAD_QUEUE_RENAME: &str = "download_queue.rename";
    pub const DOWNLOAD_QUEUE_COMPARE: &str = "download_queue.compare";
    pub const DOWNLOAD_QUEUE_BULK_START: &str = "download_queue.bulk_start";
    pub const DOWNLOAD_QUEUE_DELETE: &str = "download_queue.delete";
    pub const DOWNLOAD_QUEUE_CLEAR_COMPLETED: &str = "download_queue.clear_completed";
    pub const TANKOUBON_CREATE: &str = "tankoubon.create";
    pub const TANKOUBON_RENAME: &str = "tankoubon.rename";
    pub const TANKOUBON_DELETE: &str = "tankoubon.delete";
    pub const TANKOUBON_MEMBER_ADD: &str = "tankoubon.member_add";
    pub const TANKOUBON_MEMBER_REMOVE: &str = "tankoubon.member_remove";
    /// `PUT /tankoubons/{id}` (`tankoubons.rs::update_tankoubon`) — name/summary/tags/chapter
    /// names/member-list edits, the Tankoubon equivalent of `ARCHIVE_METADATA_UPDATE`. This
    /// endpoint recorded no activity at all until this constant existed — a real, pre-existing
    /// gap (not a design choice) confirmed while adding rating-only detection for it, matching
    /// `ARCHIVE_RATING_UPDATE`'s own split from `ARCHIVE_METADATA_UPDATE` below.
    pub const TANKOUBON_METADATA_UPDATE: &str = "tankoubon.metadata_update";
    /// A `PUT /tankoubons/{id}` call whose only real effect was the `rating:` tag — same detection
    /// shape as `ARCHIVE_RATING_UPDATE` (title/summary/chapter_names/archives all unchanged, at
    /// most one `rating:` tag added and one removed) — see
    /// `tankoubons.rs::update_tankoubon`'s own `non_rating_change` tracking, which already existed
    /// for an unrelated purpose (suppressing the `updated_at` sort-order bump for a rating-only
    /// change) before this activity split reused the same distinction.
    pub const TANKOUBON_RATING_UPDATE: &str = "tankoubon.rating_update";
    pub const DATABASE_DROP: &str = "database.drop";
    pub const DATABASE_RESTORE: &str = "database.restore";
    pub const DATABASE_CLEAR_NEW_FLAGS: &str = "database.clear_new_flags";
    pub const DATABASE_CLEAN: &str = "database.clean";
    pub const DATABASE_REBUILD_INDEX: &str = "database.rebuild_index";
    pub const DATABASE_IMPORT_LEGACY: &str = "database.import_legacy";
    pub const PLUGIN_EXECUTE: &str = "plugin.execute";
    pub const PLUGIN_UPLOAD: &str = "plugin.upload";
    pub const PLUGIN_PRIORITY_UPDATE: &str = "plugin.priority_update";
    pub const PLUGIN_URL_DOWNLOAD_TRIGGER: &str = "plugin.url_download_trigger";
    /// `POST /plugin-wizard/save` (`plugin_wizard::save`) — the AI plugin wizard's own terminal
    /// confirm-save, split out from `PLUGIN_UPLOAD` so it reads distinctly in the activity list
    /// (real user feedback, 2026-08-26: asked whether AI-generated/installed/edited plugins leave
    /// any activity trail at all — they did, but indistinguishably from a plain manual `.ts`
    /// upload). Covers both a fresh generate-and-save and an edit-an-existing-AI-plugin overwrite
    /// — `after.overwrite` (bool) tells the two apart; intermediate generate/regenerate/trial-run
    /// rounds inside the wizard are deliberately NOT recorded here, only the final save (same
    /// "only the real mutating action, not every intermediate step" convention as everywhere else
    /// in this module).
    pub const PLUGIN_WIZARD_SAVE: &str = "plugin_wizard.save";
    /// Automatic: the file watcher/scanner catalogued a new archive on its own.
    pub const SCANNER_INGEST: &str = "scanner.ingest";
    /// Automatic: metadata plugins ran without being tied to a manual upload/download that
    /// already has its own record — see `lanrurugi_api::activity`'s own docs on why this is only
    /// written for the scanner-triggered case, never for upload/download (those merge the
    /// summary into their own manual record's `after` field instead).
    pub const METADATA_PLUGIN_AUTORUN: &str = "metadata_plugin.autorun";
    /// Reserved for issue #55 (subscription-style auto-download), not implemented yet — the
    /// constant exists now so that feature's own write site can reuse this module's structure
    /// unchanged.
    pub const AUTO_DOWNLOAD: &str = "auto_download.fetch";
    pub const BOOKMARK_ADD: &str = "bookmark.add";
    pub const BOOKMARK_REMOVE: &str = "bookmark.remove";
    /// issue #97: a stamp add/delete that also added/removed a page's bookmark — one merged
    /// record for both halves of the linked action (not a separate stamp record plus a separate
    /// bookmark record), direction distinguished by `after.bookmark.action` (`"add"`/`"remove"`).
    /// Never written when the linkage didn't actually fire (setting off, page already in the
    /// target state, or other stamps remain on the page).
    pub const STAMP_BOOKMARK_SYNC: &str = "stamp.bookmark_sync";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorKind {
    Session,
    Token,
    /// scanner ingest / metadata-plugin autorun / (future) auto-download — the system itself,
    /// not any human-initiated request.
    System,
    /// No persistent identity behind this action — either `require_api_key` never inserted an
    /// `AuthContext` at all, or it inserted `AuthMethod::GuestVisitor`
    /// (007-guest-restricted-access), which carries no identity of its own either. Distinct from
    /// `Session`/`Token` so the activity feed doesn't misrepresent a no-identity action as an
    /// authenticated one — in practice this action still shouldn't be reachable at all (every
    /// route either of those two callers can reach is GET-only/read-only), but this variant exists
    /// so recording one is a defensive no-op, not a panic.
    Anonymous,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Actor {
    pub kind: ActorKind,
    /// `Token`: the token's own record id. `System`: a fixed subsystem tag ("scanner" /
    /// "metadata_plugin"). `Session`/`Anonymous`: always `None` (this project has no
    /// multi-user session identity to distinguish further).
    #[serde(default)]
    pub id: Option<String>,
    /// Snapshotted at write time, not resolved live on read — a `Token`'s human-readable name is
    /// copied in here so a since-revoked token's history stays legible (looking it up by `id`
    /// again later would just come back empty). `System` actors get a fixed display label
    /// ("Scanner (File Watcher)", "Metadata Plugin (auto)").
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityTarget {
    /// The affected object's id (archive id, token id, category id, tankoubon id, plugin
    /// namespace, download-queue item id, ...). `None` for database-wide operations
    /// (drop/clean/rebuild-index) that have no single target.
    #[serde(default)]
    pub id: Option<String>,
    /// Human-readable label, snapshotted at write time for the same reason `Actor::display_name`
    /// is — the target itself may be renamed or deleted later without invalidating this record's
    /// own readability.
    #[serde(default)]
    pub label: Option<String>,
    /// "archive" | "token" | "category" | "tankoubon" | "plugin" | "download_queue_item" |
    /// "database" | "settings" — drives which icon/link the frontend renders.
    #[serde(default)]
    pub kind: Option<String>,
}

/// The causal chain for an automatic entry that was itself triggered by another event —
/// currently only `metadata_plugin.autorun`, pointing back at the `scanner.ingest` entry that
/// triggered it. Stores a short description alongside the (best-effort) `source_entry_id` rather
/// than embedding the whole parent record: the parent may have already aged out under retention,
/// and the description alone still reads sensibly on its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CausedBy {
    /// "scanner_ingest" — the only value currently produced; more may be added (e.g.
    /// "auto_download" for issue #55) without needing a schema migration since this is a plain
    /// string, not an enum.
    pub reason: String,
    #[serde(default)]
    pub source_entry_id: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutoOrManual {
    Manual,
    Automatic,
}

/// Whether the recorded action actually completed — every write site historically only ever
/// called `record_manual`/`record_automatic` from its own success path, so a failed action (a 500,
/// a validation rejection, a partial-failure branch) left no trace in the activity log at all
/// despite being exactly the kind of event an operator audit trail exists to surface. Each write
/// site was individually reviewed to add a `Failure` record on its own real failure branches (not
/// mechanically bolted onto every `Err` in the file — several early-return validation failures,
/// e.g. "unknown settings field", intentionally still don't get their own entry, since those are
/// rejected before anything resembling the named action was attempted at all).
///
/// `#[serde(tag = "status")]` (an internally-tagged enum, not the default externally-tagged
/// `{"failure": {"reason": "..."}}` shape) so the JSON stays flat and easy for the frontend to
/// branch on with a single `outcome.status` string, matching how `AutoOrManual`/`ActorKind` above
/// are already flat string enums the frontend switches on directly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Success,
    /// `reason` is a short, human-readable failure summary (an error's `Display` output, or a
    /// validation-rejection message) — same "safe to show, not necessarily exhaustive" contract as
    /// `JobStatus::error` (`lanrurugi_core::jobs`), not a full error chain/backtrace.
    Failure {
        reason: String,
    },
}

impl Default for Outcome {
    /// Every `ActivityEntry` persisted before this field existed was, definitionally, a recorded
    /// success (see this enum's own docs on why failures were never recorded at all until now) —
    /// `#[serde(default)]` on `ActivityEntry::outcome` deserializes an old stored record's missing
    /// field to this, which is the historically accurate value, not an arbitrary placeholder.
    fn default() -> Self {
        Self::Success
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityEntry {
    pub id: String,
    /// Unix seconds — always identical to this entry's own score in every sorted-set index it's
    /// a member of (see this module's own key-design docs); the two are derived from the same
    /// `now` value at write time and never diverge.
    pub timestamp: i64,
    pub actor: Actor,
    pub auto_or_manual: AutoOrManual,
    pub action_type: ActionType,
    pub target: ActivityTarget,
    #[serde(default)]
    pub outcome: Outcome,
    /// Display/diagnostic only — same disclaimer as `ApiTokenRecord::last_used_ip`: derived from
    /// `X-Forwarded-For`'s first hop or the raw peer address, spoofable from an untrusted network
    /// position with no trusted-proxy allowlist configured. `None` for `System`-actor entries
    /// (there's no HTTP request to derive an IP from).
    #[serde(default)]
    pub client_ip: Option<String>,
    /// Populated only for "update"-shaped actions (settings.update, archive.metadata_update,
    /// token.rename, ...) — a `serde_json::Value` rather than a per-action-type Rust type because
    /// the shape genuinely differs per `action_type` and nothing ever filters a query by the
    /// *contents* of this field, only the frontend's own display code branches on `action_type`
    /// to interpret it.
    #[serde(default)]
    pub before: Option<serde_json::Value>,
    #[serde(default)]
    pub after: Option<serde_json::Value>,
    /// Only present for `auto_or_manual = Automatic` entries with a real upstream trigger (i.e.
    /// `metadata_plugin.autorun`, not `scanner.ingest` itself — nothing triggered the scanner
    /// ingesting a file besides the file appearing on disk).
    #[serde(default)]
    pub caused_by: Option<CausedBy>,
}

fn entry_key(id: &str) -> String {
    format!("LANRURUGI_ACTIVITY_{id}")
}

/// Global time index — every entry is a member, scored by `timestamp`. The base scan source for
/// an unfiltered (or time-range-only) query.
const ORDER_KEY: &str = "LANRURUGI_ACTIVITY_ORDER";

fn by_actor_key(actor_key: &str) -> String {
    format!("LANRURUGI_ACTIVITY_BY_ACTOR_{actor_key}")
}

fn by_action_type_key(action_type: &str) -> String {
    format!("LANRURUGI_ACTIVITY_BY_ACTION_{action_type}")
}

/// `outcome_key` is the fixed string `"success"`/`"failure"` (`Outcome`'s own `#[serde(tag =
/// "status")]` discriminant) — unlike actor/action_type, there are only ever exactly these two
/// values, so this index needs no `KNOWN_*` candidate-discovery set the way `KNOWN_ACTORS_KEY`/
/// `KNOWN_ACTION_TYPES_KEY` provide for those (the frontend's outcome filter can hardcode both
/// options rather than asking `facets()` what values exist).
fn by_outcome_key(outcome_key: &str) -> String {
    format!("LANRURUGI_ACTIVITY_BY_OUTCOME_{outcome_key}")
}

fn outcome_index_key(outcome: &Outcome) -> &'static str {
    match outcome {
        Outcome::Success => "success",
        Outcome::Failure { .. } => "failure",
    }
}

/// Every actor key that has ever appeared — the facets endpoint's candidate source (further
/// filtered down to only non-empty indexes at read time, see [`ActivityRepository::facets`]).
/// Never itself pruned; a stale entry here costs one cheap `ZCARD` per facets call, which is
/// negligible next to maintaining a second parallel cleanup path.
const KNOWN_ACTORS_KEY: &str = "LANRURUGI_ACTIVITY_KNOWN_ACTORS";
const KNOWN_ACTION_TYPES_KEY: &str = "LANRURUGI_ACTIVITY_KNOWN_ACTION_TYPES";

/// Single Redis value (a plain integer string, not JSON) holding the configured retention window
/// in seconds — `None`/absent means "keep forever", mirroring `ApiTokenRepository::issue`'s own
/// `expires_in_secs: Option<i64>` convention.
const RETENTION_SECS_KEY: &str = "LANRURUGI_ACTIVITY_RETENTION_SECS";

/// `actor_key` mirrors `AuthContext::trace_label()`'s own format ("session" / "token:<id>"), with
/// a "system:<subsystem>" prefix added for the two kinds that format doesn't otherwise produce.
fn actor_index_key(actor: &Actor) -> String {
    match actor.kind {
        ActorKind::Session => "session".to_string(),
        ActorKind::Token => format!("token:{}", actor.id.as_deref().unwrap_or("")),
        ActorKind::System => format!("system:{}", actor.id.as_deref().unwrap_or("")),
        ActorKind::Anonymous => "anonymous".to_string(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct ActivityFilter {
    pub start_ts: Option<i64>,
    pub end_ts: Option<i64>,
    /// Same format as [`actor_index_key`] — "session" | "token:<id>" | "system:<subsystem>" |
    /// "anonymous". Multiple values are OR'd together (any matching actor), same as
    /// `action_types` below — mirrors the frontend's own multi-select chip filter (issue #87's
    /// `Combobox` in `multiple` mode), not an AND-of-actors query (which would be vacuous: a
    /// single entry only ever has exactly one actor, so "actor A AND actor B" could never match
    /// anything).
    pub actor_keys: Vec<String>,
    /// OR'd together — same reasoning as `actor_keys` above (a single entry has exactly one
    /// `action_type`, so this can only ever be a "match any of" filter, never "match all of").
    pub action_types: Vec<String>,
    /// "success" | "failure" ([`outcome_index_key`]'s own format) — same OR-multiple/AND-across-
    /// dimensions treatment as `actor_keys`/`action_types`. In practice the frontend only ever
    /// offers exactly these two checkboxes, so this is almost always `[]` (no filter), `["success"]`,
    /// `["failure"]`, or both (equivalent to no filter, but a legal query all the same).
    pub outcome_keys: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ActivityPage {
    pub entries: Vec<ActivityEntry>,
    pub next_cursor: Option<String>,
    pub total_estimate: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActivityFacetValue {
    pub value: String,
    pub count: usize,
}

#[derive(Debug, Clone)]
pub struct ActivityFacets {
    pub action_types: Vec<ActivityFacetValue>,
    /// `value` here is the raw `actor_key` string ("session" / "token:<id>" / "system:<x>" /
    /// "anonymous") — the API layer resolves it into a structured `{kind, id, display_name}` by
    /// looking at one representative entry per key (see `lanrurugi_api::activity`'s facets
    /// handler), since the repository layer itself has no reason to know how to format a
    /// human-readable label.
    pub actors: Vec<ActivityFacetValue>,
}

fn encode_cursor(timestamp: i64, id: &str) -> String {
    format!("{timestamp}_{id}")
}

fn decode_cursor(cursor: &str) -> Result<(i64, String)> {
    let (ts, id) = cursor
        .split_once('_')
        .ok_or_else(|| ActivityStorageError::Cursor(cursor.to_string()))?;
    let ts: i64 = ts
        .parse()
        .map_err(|_| ActivityStorageError::Cursor(cursor.to_string()))?;
    Ok((ts, id.to_string()))
}

#[derive(Clone)]
pub struct ActivityRepository {
    pool: Pool,
}

impl ActivityRepository {
    pub fn new(pool: Pool) -> Self {
        Self { pool }
    }

    pub async fn get(&self, id: &str) -> Result<Option<ActivityEntry>> {
        let mut conn = self.pool.get().await?;
        let key = entry_key(id);
        let raw: Option<String> = conn.get(&key).await?;
        match raw {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| ActivityStorageError::Json(key, e)),
        }
    }

    /// Reads the currently configured retention window — `None` means "keep forever". Read fresh
    /// on every [`append`](Self::append) call rather than cached in memory: writes are far less
    /// frequent than an admin's occasional retention-setting change would need to propagate, so
    /// the extra `GET` costs nothing that matters and guarantees the value is never stale.
    pub async fn retention_secs(&self) -> Result<Option<i64>> {
        let mut conn = self.pool.get().await?;
        let raw: Option<i64> = conn.get(RETENTION_SECS_KEY).await?;
        Ok(raw)
    }

    pub async fn set_retention_secs(&self, secs: Option<i64>) -> Result<()> {
        let mut conn = self.pool.get().await?;
        match secs {
            Some(secs) => {
                let _: () = conn.set(RETENTION_SECS_KEY, secs).await?;
            }
            None => {
                let _: () = conn.del(RETENTION_SECS_KEY).await?;
            }
        }
        Ok(())
    }

    /// Writes `entry` plus every index it participates in. `ttl_secs` (from
    /// [`retention_secs`](Self::retention_secs), read by the caller) applies **only** to the
    /// primary record key — `ORDER_KEY`/`by_actor_key`/`by_action_type_key` are long-lived
    /// structures shared by every entry, not this one entry's own key, so Redis's `EXPIRE` (which
    /// has no notion of "expire just this one member of a set") can't be pointed at them; a
    /// member whose primary record has since expired is instead pruned lazily the next time a
    /// query encounters it (see [`list_page`](Self::list_page)).
    pub async fn append(&self, entry: &ActivityEntry, ttl_secs: Option<i64>) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let key = entry_key(&entry.id);
        let raw =
            serde_json::to_string(entry).map_err(|e| ActivityStorageError::Json(key.clone(), e))?;
        let actor_key = actor_index_key(&entry.actor);

        let _: () = conn.set(&key, raw).await?;
        let _: () = conn.zadd(ORDER_KEY, &entry.id, entry.timestamp).await?;
        let _: () = conn
            .zadd(by_actor_key(&actor_key), &entry.id, entry.timestamp)
            .await?;
        let _: () = conn.sadd(KNOWN_ACTORS_KEY, &actor_key).await?;
        let _: () = conn
            .zadd(
                by_action_type_key(&entry.action_type),
                &entry.id,
                entry.timestamp,
            )
            .await?;
        let _: () = conn
            .sadd(KNOWN_ACTION_TYPES_KEY, &entry.action_type)
            .await?;
        let _: () = conn
            .zadd(
                by_outcome_key(outcome_index_key(&entry.outcome)),
                &entry.id,
                entry.timestamp,
            )
            .await?;

        if let Some(secs) = ttl_secs {
            let ttl = secs.max(1);
            let _: () = conn.expire(&key, ttl).await?;
        }
        Ok(())
    }

    /// Cursor-paginated, newest-first. `cursor: None` starts from the newest entry; `Some(cursor)`
    /// continues strictly before it (exclusive), so the boundary entry from the previous page is
    /// never repeated. `limit` is expected to already be clamped by the caller (API layer).
    ///
    /// Combines up to four filter dimensions:
    /// - time range alone (or no filter at all) → scans `ORDER_KEY` directly.
    /// - `actor_keys`/`action_types`/`outcome_keys`, each alone (one or more values) → a single
    ///   value scans its own `by_*_key(...)` directly; multiple values within the *same* dimension
    ///   are OR'd via a short-lived `ZUNIONSTORE`'d temp key (30s TTL, self-reaping) first, since a
    ///   single entry only ever has one actor/one action_type/one outcome (see
    ///   [`ActivityFilter::actor_keys`]'s own docs on why this is "any of", never "all of", within
    ///   one dimension).
    /// - two or more dimensions together → whichever direct/`ZUNIONSTORE`'d key each active
    ///   dimension resolved to are `ZINTERSTORE`'d pairwise into a further temp key (also 30s TTL)
    ///   as the final scan source, since every input carries the same score (this entry's own
    ///   timestamp) for any member it shares — `ZINTERSTORE`'s default SUM aggregation multiplies
    ///   the score by however many dimensions matched, but preserves relative order, so the same
    ///   `ZREVRANGEBYSCORE` pagination logic below still works unmodified against it regardless of
    ///   how many dimensions were combined.
    pub async fn list_page(
        &self,
        filter: &ActivityFilter,
        cursor: Option<&str>,
        limit: isize,
    ) -> Result<ActivityPage> {
        let mut conn = self.pool.get().await?;

        fn new_temp_key() -> String {
            format!("LANRURUGI_ACTIVITY_TMP_{}", uuid::Uuid::new_v4())
        }

        let mut temp_keys: Vec<String> = Vec::new();

        // Resolves one filter dimension's `Vec<String>` of OR'd values (e.g. `actor_keys`) into a
        // single Redis key to intersect against the others — `None` when the dimension carries no
        // filter at all (every entry passes), a direct `by_*_key` lookup for exactly one value, or
        // a fresh `ZUNIONSTORE`'d temp key when the caller selected more than one value within this
        // dimension.
        async fn resolve_dimension(
            conn: &mut deadpool_redis::Connection,
            values: &[String],
            key_fn: impl Fn(&str) -> String,
            temp_keys: &mut Vec<String>,
        ) -> Result<Option<String>> {
            match values {
                [] => Ok(None),
                [single] => Ok(Some(key_fn(single))),
                many => {
                    let tmp = new_temp_key();
                    let source_keys: Vec<String> = many.iter().map(|v| key_fn(v)).collect();
                    let _: () = conn.zunionstore(&tmp, &source_keys).await?;
                    let _: () = conn.expire(&tmp, 30).await?;
                    temp_keys.push(tmp.clone());
                    Ok(Some(tmp))
                }
            }
        }

        let actor_side =
            resolve_dimension(&mut conn, &filter.actor_keys, by_actor_key, &mut temp_keys).await?;
        let action_type_side = resolve_dimension(
            &mut conn,
            &filter.action_types,
            by_action_type_key,
            &mut temp_keys,
        )
        .await?;
        let outcome_side = resolve_dimension(
            &mut conn,
            &filter.outcome_keys,
            by_outcome_key,
            &mut temp_keys,
        )
        .await?;

        let active_dimensions: Vec<String> = [actor_side, action_type_side, outcome_side]
            .into_iter()
            .flatten()
            .collect();

        let base_key = match active_dimensions.as_slice() {
            [] => ORDER_KEY.to_string(),
            [single] => single.clone(),
            many => {
                let tmp = new_temp_key();
                let _: () = conn.zinterstore(&tmp, many).await?;
                let _: () = conn.expire(&tmp, 30).await?;
                temp_keys.push(tmp.clone());
                tmp
            }
        };

        let max_score = filter.end_ts.unwrap_or(i64::MAX);
        let min_score = filter.start_ts.unwrap_or(i64::MIN);

        let max_bound = match cursor {
            None => max_score.to_string(),
            Some(c) => {
                let (ts, _id) = decode_cursor(c)?;
                // Exclusive upper bound — the cursor's own entry (already shown on the previous
                // page) must not reappear.
                format!("({ts}")
            }
        };

        // Over-fetch by one to know whether a next page exists without a second round-trip.
        let raw: Vec<(String, i64)> = conn
            .zrevrangebyscore_limit_withscores(&base_key, max_bound, min_score, 0, limit + 1)
            .await?;

        // Read before the temp keys are torn down below — `base_key` itself may *be* one of them
        // (whenever more than one filter dimension is active, see `active_dimensions` above), so
        // `ZCARD`ing it after `DEL` would always read back 0 regardless of how many entries
        // actually matched (a real bug this fixed: confirmed live, `total_estimate` silently came
        // back `0` for any multi-dimension or multi-value-within-one-dimension query despite
        // `entries` itself being correctly populated).
        let total_estimate: usize = conn.zcard(&base_key).await.unwrap_or(0);

        for tmp in &temp_keys {
            let _: () = conn.del(tmp).await?;
        }

        let has_more = raw.len() as isize > limit;
        let page_ids = &raw[..raw.len().min(limit as usize)];

        let mut entries = Vec::with_capacity(page_ids.len());
        for (id, _score) in page_ids {
            match self.get(id).await? {
                Some(entry) => entries.push(entry),
                None => {
                    // The primary record has expired since this id was indexed — lazily prune it
                    // from every index it could still be sitting in rather than surfacing a hole
                    // in the page. Doesn't count against `limit`; the caller only ever sees real
                    // entries.
                    self.prune_dangling_index_entry(id).await?;
                }
            }
        }

        let next_cursor = if has_more {
            entries.last().map(|e| encode_cursor(e.timestamp, &e.id))
        } else {
            None
        };

        Ok(ActivityPage {
            entries,
            next_cursor,
            total_estimate: Some(total_estimate),
        })
    }

    /// Removes `id` from every long-lived index it might still be a member of — called when a
    /// query discovers the primary record itself has already expired. Cheap best-effort: doesn't
    /// know which actor/action_type indexes actually contain `id` without the (now-gone) record
    /// to read them from, so it removes from `ORDER_KEY` unconditionally and leaves the
    /// actor/action_type sorted sets to the same lazy pruning the next time a query over *that*
    /// specific index happens to walk past this id.
    async fn prune_dangling_index_entry(&self, id: &str) -> Result<()> {
        let mut conn = self.pool.get().await?;
        let _: () = conn.zrem(ORDER_KEY, id).await?;
        Ok(())
    }

    /// Re-derives `by_outcome_key`'s two sorted sets (`"success"`/`"failure"`) from every entry
    /// still tracked in `ORDER_KEY` — needed because `outcome` didn't always exist on
    /// `ActivityEntry`: every entry written before this field shipped was never indexed by outcome
    /// at all (`append` only ever adds a new entry to this index at write time, never retroactively
    /// re-indexes an old one), so filtering by outcome on an instance with pre-existing activity
    /// history silently returned nothing for either "success" or "failure" until this ran once
    /// (confirmed live: an instance with real history returned zero entries for `outcome=success`
    /// despite `GET /activity` with no filter at all showing plenty of success entries). Safe to
    /// run repeatedly — each `ZADD` on an already-indexed entry is a harmless no-op — and cheap on
    /// an already-backfilled instance. Called unconditionally on every `serve` boot
    /// (`lanrurugi-server/src/main.rs`), same "run it every startup" pattern as
    /// `lanrurugi_storage::rebuild::backfill_reverse_indexes`.
    pub async fn backfill_outcome_index(&self) -> Result<usize> {
        let mut conn = self.pool.get().await?;
        let ids: Vec<String> = conn.zrange(ORDER_KEY, 0, -1).await?;
        let mut backfilled = 0usize;
        for id in ids {
            let Some(entry) = self.get(&id).await? else {
                continue;
            };
            let _: () = conn
                .zadd(
                    by_outcome_key(outcome_index_key(&entry.outcome)),
                    &entry.id,
                    entry.timestamp,
                )
                .await?;
            backfilled += 1;
        }
        Ok(backfilled)
    }

    /// Every action type / actor key that has ever appeared, filtered down to only those with at
    /// least one entry still actually present (an expired-out action type/actor no longer offers
    /// itself as a filterable candidate — see this module's own docs on `KNOWN_ACTORS_KEY`).
    pub async fn facets(&self) -> Result<ActivityFacets> {
        let mut conn = self.pool.get().await?;
        let known_actors: Vec<String> = conn.smembers(KNOWN_ACTORS_KEY).await?;
        let known_action_types: Vec<String> = conn.smembers(KNOWN_ACTION_TYPES_KEY).await?;

        let mut actors = Vec::new();
        for actor in known_actors {
            let count: usize = conn.zcard(by_actor_key(&actor)).await?;
            if count > 0 {
                actors.push(ActivityFacetValue {
                    value: actor,
                    count,
                });
            }
        }

        let mut action_types = Vec::new();
        for action_type in known_action_types {
            let count: usize = conn.zcard(by_action_type_key(&action_type)).await?;
            if count > 0 {
                action_types.push(ActivityFacetValue {
                    value: action_type,
                    count,
                });
            }
        }

        Ok(ActivityFacets {
            action_types,
            actors,
        })
    }

    /// Removes the primary record and every index entry pointing at it. Idempotent — deleting an
    /// already-absent id is a no-op, matching [`crate::api_tokens::ApiTokenRepository::delete`]'s
    /// own convention.
    pub async fn delete(&self, id: &str) -> Result<Option<ActivityEntry>> {
        let Some(entry) = self.get(id).await? else {
            return Ok(None);
        };
        let mut conn = self.pool.get().await?;
        let actor_key = actor_index_key(&entry.actor);
        let _: () = conn.del(entry_key(id)).await?;
        let _: () = conn.zrem(ORDER_KEY, id).await?;
        let _: () = conn.zrem(by_actor_key(&actor_key), id).await?;
        let _: () = conn
            .zrem(by_action_type_key(&entry.action_type), id)
            .await?;
        let _: () = conn
            .zrem(by_outcome_key(outcome_index_key(&entry.outcome)), id)
            .await?;
        Ok(Some(entry))
    }

    /// Bulk delete — a plain loop over [`delete`](Self::delete). Not pipelined: activity-record
    /// deletion is a rare, Session-only, human-triggered action (see `require_session`'s own
    /// gating in the API layer), never a hot path, so the simplicity of reusing the single-delete
    /// method outweighs any pipelining benefit.
    pub async fn delete_many(&self, ids: &[String]) -> Result<u32> {
        let mut deleted = 0u32;
        for id in ids {
            if self.delete(id).await?.is_some() {
                deleted += 1;
            }
        }
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> Option<Pool> {
        crate::test_support::test_pool().await
    }

    fn sample_entry(id: &str, timestamp: i64, actor: Actor, action_type: &str) -> ActivityEntry {
        ActivityEntry {
            id: id.to_string(),
            timestamp,
            actor,
            auto_or_manual: AutoOrManual::Manual,
            action_type: action_type.to_string(),
            target: ActivityTarget {
                id: Some("archive-1".to_string()),
                label: Some("Some Archive".to_string()),
                kind: Some("archive".to_string()),
            },
            outcome: Outcome::Success,
            client_ip: Some("127.0.0.1".to_string()),
            before: None,
            after: None,
            caused_by: None,
        }
    }

    fn session_actor() -> Actor {
        Actor {
            kind: ActorKind::Session,
            id: None,
            display_name: None,
        }
    }

    /// A fresh, real-time-derived base timestamp for each test run — real-world confirmed
    /// necessary: a previous version of these tests used small fixed literals (1_000, 2_000, ...)
    /// shared across every test function, which meant a *prior* failed run's own
    /// panicked-before-cleanup entries (still sitting in the shared `ORDER_KEY`/index sorted
    /// sets against the same test Redis instance every run reuses) collided on identical scores
    /// with a subsequent run's freshly-appended entries — `ZREVRANGEBYSCORE`'s tie-breaking on
    /// equal scores falls back to member (id) lexical order, not insertion order, so a collision
    /// silently produced a wrong-but-plausible-looking ordering instead of an obvious error.
    /// Deriving the base from the real clock makes every test run's own timestamps effectively
    /// unique regardless of what a previous failed run left behind, without needing a `FLUSHDB`
    /// between runs.
    ///
    /// **Seconds, not nanoseconds** — a real, live-caught bug: an earlier version of this
    /// function used `.as_nanos()`, which at today's wall-clock time is already past 1.7×10^18,
    /// well beyond `f64`'s exact-integer range (±2^53 ≈ 9×10^15 — Redis sorted-set scores are
    /// IEEE-754 doubles, per `ZADD`'s own docs). Every one of a test's own small same-base offsets
    /// (`base`, `base + 50`, `base + 100`, ...) silently rounded to the *same* representable
    /// double, so entries that should have had distinct, orderable scores all landed on one
    /// identical score instead — confirmed live via `ZRANGE ... WITHSCORES` showing three
    /// supposedly-different entries sharing the exact same score. `timestamp` is documented as
    /// Unix *seconds* everywhere else in this module (`ActivityEntry::timestamp`'s own doc
    /// comment) anyway, so `.as_secs()` is also the semantically correct unit here, not just the
    /// one that happens to avoid the precision cliff.
    fn test_time_base() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock is before UNIX_EPOCH")
            .as_secs() as i64
    }

    #[tokio::test]
    async fn appends_and_gets_an_entry() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);
        let id = uuid::Uuid::new_v4().to_string();
        let entry = sample_entry(
            &id,
            test_time_base(),
            session_actor(),
            action_types::ARCHIVE_DELETE,
        );

        repo.append(&entry, None).await.unwrap();
        let fetched = repo.get(&id).await.unwrap().unwrap();
        assert_eq!(fetched.action_type, action_types::ARCHIVE_DELETE);
        assert_eq!(fetched.target.label.as_deref(), Some("Some Archive"));

        repo.delete(&id).await.unwrap();
    }

    #[tokio::test]
    async fn list_page_returns_newest_first_and_paginates_via_cursor() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);
        let base = test_time_base();
        // A test-run-unique action type (not `ActivityFilter::default()`'s unfiltered scan of the
        // *global* `ORDER_KEY`) — real-world confirmed necessary: `cargo test` runs test functions
        // concurrently by default, and every test in this module shares the same Redis instance
        // (`LANRURUGI_TEST_REDIS_URL`), so an unfiltered query here would non-deterministically
        // also see entries other *concurrently running* tests in this same file had already
        // appended to that same shared global index — `test_time_base()` alone only solves
        // collisions *across separate test runs*, not concurrent tests *within* one run scanning
        // the same unfiltered index. Filtering by a unique-per-run action type scans this test's
        // own dedicated `by_action_type_key` sorted set instead, which only this test ever writes
        // to.
        let action_type = format!("test.list_page.{}", uuid::Uuid::new_v4());
        let ids: Vec<String> = (0..5).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        for (i, id) in ids.iter().enumerate() {
            let entry = sample_entry(id, base + i as i64, session_actor(), &action_type);
            repo.append(&entry, None).await.unwrap();
        }
        let filter = ActivityFilter {
            action_types: vec![action_type.clone()],
            ..Default::default()
        };

        let page1 = repo.list_page(&filter, None, 2).await.unwrap();
        assert_eq!(page1.entries.len(), 2);
        // Newest first: entries[4] then entries[3].
        assert_eq!(page1.entries[0].id, ids[4]);
        assert_eq!(page1.entries[1].id, ids[3]);
        assert!(page1.next_cursor.is_some());

        let page2 = repo
            .list_page(&filter, page1.next_cursor.as_deref(), 2)
            .await
            .unwrap();
        assert_eq!(page2.entries.len(), 2);
        assert_eq!(page2.entries[0].id, ids[2]);
        assert_eq!(page2.entries[1].id, ids[1]);
        // No id repeated across pages.
        assert_ne!(page1.entries[1].id, page2.entries[0].id);

        for id in &ids {
            repo.delete(id).await.unwrap();
        }
    }

    #[tokio::test]
    async fn filters_by_actor_and_action_type_combined() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);
        let base = test_time_base();
        let token_actor = Actor {
            kind: ActorKind::Token,
            id: Some("tok-1".to_string()),
            display_name: Some("Mihon phone".to_string()),
        };

        let matching_id = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(
                &matching_id,
                base,
                token_actor.clone(),
                action_types::ARCHIVE_DELETE,
            ),
            None,
        )
        .await
        .unwrap();

        // Same actor, different action type — must NOT match the combined filter.
        let wrong_action_id = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(
                &wrong_action_id,
                base + 1,
                token_actor.clone(),
                action_types::ARCHIVE_RENAME,
            ),
            None,
        )
        .await
        .unwrap();

        // Same action type, different actor — must NOT match the combined filter.
        let wrong_actor_id = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(
                &wrong_actor_id,
                base + 2,
                session_actor(),
                action_types::ARCHIVE_DELETE,
            ),
            None,
        )
        .await
        .unwrap();

        let filter = ActivityFilter {
            actor_keys: vec!["token:tok-1".to_string()],
            action_types: vec![action_types::ARCHIVE_DELETE.to_string()],
            ..Default::default()
        };
        let page = repo.list_page(&filter, None, 10).await.unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].id, matching_id);

        repo.delete(&matching_id).await.unwrap();
        repo.delete(&wrong_action_id).await.unwrap();
        repo.delete(&wrong_actor_id).await.unwrap();
    }

    #[tokio::test]
    async fn filters_by_multiple_action_types_and_multiple_actors_ored_together() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);
        let base = test_time_base();
        // Test-run-unique action types (same reasoning as the other list_page tests' own doc
        // comments — several tests share one Redis instance and run concurrently).
        let action_a = format!("test.multi.a.{}", uuid::Uuid::new_v4());
        let action_b = format!("test.multi.b.{}", uuid::Uuid::new_v4());
        let action_other = format!("test.multi.other.{}", uuid::Uuid::new_v4());
        let token_actor = Actor {
            kind: ActorKind::Token,
            id: Some(format!("tok-multi-{}", uuid::Uuid::new_v4())),
            display_name: Some("Mihon phone".to_string()),
        };

        let matches_action_a = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(&matches_action_a, base, session_actor(), &action_a),
            None,
        )
        .await
        .unwrap();
        let matches_action_b = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(&matches_action_b, base + 1, session_actor(), &action_b),
            None,
        )
        .await
        .unwrap();
        // Neither of the two requested action types — must not match.
        let matches_neither = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(&matches_neither, base + 2, session_actor(), &action_other),
            None,
        )
        .await
        .unwrap();

        // `action_types: [a, b]` alone (no actor filter) is an OR — both real entries appear,
        // the unrelated third one doesn't.
        let action_only_filter = ActivityFilter {
            action_types: vec![action_a.clone(), action_b.clone()],
            ..Default::default()
        };
        let page = repo.list_page(&action_only_filter, None, 10).await.unwrap();
        let ids: std::collections::HashSet<_> = page.entries.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&matches_action_a));
        assert!(ids.contains(&matches_action_b));
        assert!(!ids.contains(&matches_neither));

        // Combined with a multi-actor OR filter that only actually covers `session` (not the
        // token actor) — still returns both, since `actor_keys: [session, token:...]` is itself
        // an OR and both real entries were written by `session_actor()`.
        let combined_filter = ActivityFilter {
            action_types: vec![action_a.clone(), action_b.clone()],
            actor_keys: vec!["session".to_string(), actor_index_key(&token_actor)],
            ..Default::default()
        };
        let page2 = repo.list_page(&combined_filter, None, 10).await.unwrap();
        let ids2: std::collections::HashSet<_> =
            page2.entries.iter().map(|e| e.id.clone()).collect();
        assert_eq!(ids2.len(), 2);
        assert!(ids2.contains(&matches_action_a));
        assert!(ids2.contains(&matches_action_b));

        repo.delete(&matches_action_a).await.unwrap();
        repo.delete(&matches_action_b).await.unwrap();
        repo.delete(&matches_neither).await.unwrap();
    }

    #[tokio::test]
    async fn filters_by_time_range() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);
        let base = test_time_base();
        // Test-run-unique action type — same concurrent-tests-share-one-Redis-instance reasoning
        // as `list_page_returns_newest_first_and_paginates_via_cursor`'s own doc comment: without
        // this, another test's own entries could coincidentally land inside this test's
        // `[start_ts, end_ts]` window (scanned via the shared global `ORDER_KEY` when no
        // action_type filter narrows it down) and inflate the expected count of 1.
        let action_type = format!("test.time_range.{}", uuid::Uuid::new_v4());
        let early_id = uuid::Uuid::new_v4().to_string();
        let in_range_id = uuid::Uuid::new_v4().to_string();
        let late_id = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(&early_id, base, session_actor(), &action_type),
            None,
        )
        .await
        .unwrap();
        repo.append(
            &sample_entry(&in_range_id, base + 50, session_actor(), &action_type),
            None,
        )
        .await
        .unwrap();
        repo.append(
            &sample_entry(&late_id, base + 100, session_actor(), &action_type),
            None,
        )
        .await
        .unwrap();

        let filter = ActivityFilter {
            start_ts: Some(base + 10),
            end_ts: Some(base + 90),
            action_types: vec![action_type],
            ..Default::default()
        };
        let page = repo.list_page(&filter, None, 10).await.unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].id, in_range_id);

        repo.delete(&early_id).await.unwrap();
        repo.delete(&in_range_id).await.unwrap();
        repo.delete(&late_id).await.unwrap();
    }

    #[tokio::test]
    async fn expiring_entry_carries_a_real_redis_ttl() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let entry = sample_entry(
            &id,
            test_time_base(),
            session_actor(),
            action_types::ARCHIVE_DELETE,
        );
        repo.append(&entry, Some(3_600)).await.unwrap();

        let mut conn = pool.get().await.unwrap();
        let ttl: i64 = deadpool_redis::redis::cmd("TTL")
            .arg(entry_key(&id))
            .query_async(&mut conn)
            .await
            .unwrap();
        assert!(ttl > 0 && ttl <= 3_600, "got {ttl}");

        repo.delete(&id).await.unwrap();
    }

    #[tokio::test]
    async fn permanent_entry_carries_no_ttl() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let entry = sample_entry(
            &id,
            test_time_base(),
            session_actor(),
            action_types::ARCHIVE_DELETE,
        );
        repo.append(&entry, None).await.unwrap();

        let mut conn = pool.get().await.unwrap();
        let ttl: i64 = deadpool_redis::redis::cmd("TTL")
            .arg(entry_key(&id))
            .query_async(&mut conn)
            .await
            .unwrap();
        assert_eq!(ttl, -1, "a permanent entry must carry no TTL at all");

        repo.delete(&id).await.unwrap();
    }

    #[tokio::test]
    async fn facets_omits_action_types_and_actors_with_no_remaining_entries() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);
        let id = uuid::Uuid::new_v4().to_string();
        repo.append(
            &sample_entry(
                &id,
                test_time_base(),
                session_actor(),
                action_types::CATEGORY_CREATE,
            ),
            None,
        )
        .await
        .unwrap();

        let facets_before = repo.facets().await.unwrap();
        assert!(facets_before
            .action_types
            .iter()
            .any(|f| f.value == action_types::CATEGORY_CREATE));
        assert!(facets_before.actors.iter().any(|f| f.value == "session"));

        repo.delete(&id).await.unwrap();

        let facets_after = repo.facets().await.unwrap();
        assert!(
            !facets_after
                .action_types
                .iter()
                .any(|f| f.value == action_types::CATEGORY_CREATE && f.count > 0),
            "a fully-deleted action type must not still claim entries"
        );
    }

    #[tokio::test]
    async fn delete_removes_the_primary_record_and_every_index_entry() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);
        let id = uuid::Uuid::new_v4().to_string();
        let entry = sample_entry(
            &id,
            test_time_base(),
            session_actor(),
            action_types::TOKEN_CREATE,
        );
        repo.append(&entry, None).await.unwrap();

        let deleted = repo.delete(&id).await.unwrap();
        assert!(deleted.is_some());
        assert_eq!(repo.get(&id).await.unwrap(), None);

        let page = repo
            .list_page(
                &ActivityFilter {
                    action_types: vec![action_types::TOKEN_CREATE.to_string()],
                    ..Default::default()
                },
                None,
                10,
            )
            .await
            .unwrap();
        assert!(
            !page.entries.iter().any(|e| e.id == id),
            "deleted entry must not still appear in its action-type index"
        );

        // Idempotent.
        assert_eq!(repo.delete(&id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn retention_secs_round_trips_and_defaults_to_none() {
        let Some(pool) = test_pool().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let repo = ActivityRepository::new(pool);

        repo.set_retention_secs(None).await.unwrap();
        assert_eq!(repo.retention_secs().await.unwrap(), None);

        repo.set_retention_secs(Some(86_400 * 30)).await.unwrap();
        assert_eq!(repo.retention_secs().await.unwrap(), Some(86_400 * 30));

        repo.set_retention_secs(None).await.unwrap();
        assert_eq!(repo.retention_secs().await.unwrap(), None);
    }
}
