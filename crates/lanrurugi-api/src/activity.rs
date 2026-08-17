//! `GET /activity`, `GET /activity/facets`, `DELETE /activity/{id}`, `DELETE /activity` — the
//! operator activity log (issue #87), plus the write-side helpers (`record_manual`/
//! `record_automatic`) every mutating handler elsewhere in this crate calls into. No legacy
//! equivalent — additive, LANrurugi-only.
//!
//! # Why a shared write helper instead of each handler writing directly to
//! `state.activity`
//!
//! Every one of the ~30 write sites (`archives.rs`, `settings.rs`, `database.rs`, ...) needs the
//! *same* boilerplate around a single [`lanrurugi_storage::activity::ActivityRepository::append`]
//! call: resolve the current [`crate::auth_context::AuthContext`] into an `Actor` (with a token's
//! human-readable name snapshotted from `state.api_tokens` at write time — see
//! [`Actor::display_name`]'s own docs on why), read the configured retention window and turn it
//! into a TTL, generate an id/timestamp, and swallow (not propagate) a write failure so a Redis
//! hiccup never fails the *real* operation being audited. Centralizing this here means every call
//! site is a one-line `record_manual(&state, auth.as_ref(), action_types::X, target, before,
//! after).await;` instead of hand-rolling all of that at each of ~30 sites with the inevitable
//! drift that invites.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use futures_util::future::join_all;
use lanrurugi_core::ids::{ArchiveId, TankId};
use lanrurugi_storage::activity::{
    ActivityEntry, ActivityFilter, ActivityTarget, Actor, ActorKind, AutoOrManual, CausedBy,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth_context::{AuthContext, AuthMethod};
use crate::common::{error, ok};
use crate::state::AppState;

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs() as i64
}

/// `System`-actor display labels — fixed, not derived from anything per-call, since there's only
/// ever one of each subsystem (unlike `Token`, which has as many identities as there are issued
/// tokens).
fn system_actor(subsystem: &str) -> Actor {
    let display_name = match subsystem {
        "scanner" => "Scanner (File Watcher)",
        "metadata_plugin" => "Metadata Plugin (auto)",
        _ => subsystem,
    };
    Actor {
        kind: ActorKind::System,
        id: Some(subsystem.to_string()),
        display_name: Some(display_name.to_string()),
    }
}

/// Resolves `auth` into an [`Actor`] — the one place a `Token`'s current human-readable name is
/// looked up (`state.api_tokens.get(id)`) and snapshotted into the record, so a since-revoked
/// token's own history stays legible (see [`Actor::display_name`]'s own docs). `auth: None`
/// happens on an open instance (`enable_pass=false`), where `require_api_key` never inserts an
/// `AuthContext` at all — mapped to [`ActorKind::Anonymous`], never silently dropped or panicked
/// on, mirroring `settings::put_settings`'s own `Option<Extension<AuthContext>>` handling of the
/// exact same case.
async fn resolve_actor(state: &AppState, auth: Option<&AuthContext>) -> Actor {
    match auth {
        None => Actor {
            kind: ActorKind::Anonymous,
            id: None,
            display_name: None,
        },
        Some(a) => match &a.method {
            AuthMethod::Session => Actor {
                kind: ActorKind::Session,
                id: None,
                display_name: None,
            },
            AuthMethod::Token { id, .. } => {
                let display_name = state
                    .api_tokens
                    .get(id)
                    .await
                    .ok()
                    .flatten()
                    .map(|record| record.name);
                Actor {
                    kind: ActorKind::Token,
                    id: Some(id.clone()),
                    display_name,
                }
            }
        },
    }
}

/// Records a manually-triggered (Session- or Token-authenticated, or Anonymous on an open
/// instance) action. `auth` is whatever the calling handler already extracted via
/// `Option<axum::extract::Extension<AuthContext>>` — this function never extracts it itself
/// (extractors only work as handler parameters), it just resolves the already-extracted value
/// into an `Actor`. Returns the written entry's own id (`None` on a write failure) — same shape as
/// [`record_automatic`]'s own return, needed by call sites whose real work finishes *after* this
/// call (e.g. `download_queue.start`'s own background download task, which patches this entry's
/// `after` once ingestion completes — see `patch_after`) rather than everything being known
/// up-front the way most other write sites' `before`/`after` already are.
pub async fn record_manual(
    state: &AppState,
    auth: Option<&AuthContext>,
    action_type: &str,
    target: ActivityTarget,
    before: Option<Value>,
    after: Option<Value>,
) -> Option<String> {
    let actor = resolve_actor(state, auth).await;
    let client_ip = auth.and_then(|a| a.client_ip.clone());
    let id = uuid::Uuid::new_v4().to_string();
    let entry = ActivityEntry {
        id: id.clone(),
        timestamp: now_secs(),
        actor,
        auto_or_manual: AutoOrManual::Manual,
        action_type: action_type.to_string(),
        target,
        client_ip,
        before,
        after,
        caused_by: None,
    };
    let ttl = state.activity.retention_secs().await.unwrap_or(None);
    match state.activity.append(&entry, ttl).await {
        Ok(()) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, action_type, "failed to write activity log entry");
            None
        }
    }
}

/// Fetches entry `id`, merges `patch` into its own `after` (creating `after` from scratch if it
/// was `None`, overwriting any keys `patch` also has), and re-`append`s it under the same id —
/// `ActivityRepository` has no real partial-update primitive, only `get` + `append`-by-id
/// (a plain `SET`, so re-appending under the same id overwrites in place rather than duplicating).
/// Best-effort, same posture as every other activity write here: a missing entry (already expired/
/// deleted) or a write failure only `tracing::warn!`s, never propagates — this exists purely to
/// enrich an audit trail after the fact, never to gate the real operation it describes. Re-reads
/// the entry's own current TTL-implying retention window fresh (same reasoning as `write`'s own
/// docs) rather than trying to preserve whatever TTL the original `append` computed, since a
/// retention-window change between the original write and this patch should apply to the patch
/// the same way it would to a fresh write.
pub async fn patch_after(state: &AppState, id: &str, patch: Value) {
    let Ok(Some(mut entry)) = state.activity.get(id).await else {
        tracing::warn!(id, "failed to patch activity entry: not found");
        return;
    };
    let merged = match (entry.after.take(), patch) {
        (Some(Value::Object(mut existing)), Value::Object(new_fields)) => {
            existing.extend(new_fields);
            Value::Object(existing)
        }
        (_, patch) => patch,
    };
    entry.after = Some(merged);
    let ttl = state.activity.retention_secs().await.unwrap_or(None);
    if let Err(e) = state.activity.append(&entry, ttl).await {
        tracing::warn!(error = %e, id, "failed to write patched activity entry");
    }
}

/// Records a system-triggered action (`action_types::SCANNER_INGEST`/
/// `action_types::METADATA_PLUGIN_AUTORUN`, and eventually issue #55's auto-download). `subsystem`
/// becomes both the `Actor::id` and (via [`system_actor`]) its fixed `display_name` — currently
/// `"scanner"` or `"metadata_plugin"`. Returns the written entry's own id (`None` on a write
/// failure) so a *causally dependent* second call — currently only
/// `metadata_plugin.autorun`'s own `caused_by.source_entry_id` pointing back at the
/// `scanner.ingest` entry that triggered it — has something to link to.
pub async fn record_automatic(
    state: &AppState,
    subsystem: &str,
    action_type: &str,
    target: ActivityTarget,
    caused_by: Option<CausedBy>,
) -> Option<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let entry = ActivityEntry {
        id: id.clone(),
        timestamp: now_secs(),
        actor: system_actor(subsystem),
        auto_or_manual: AutoOrManual::Automatic,
        action_type: action_type.to_string(),
        target,
        client_ip: None,
        before: None,
        after: None,
        caused_by,
    };
    let ttl = state.activity.retention_secs().await.unwrap_or(None);
    match state.activity.append(&entry, ttl).await {
        Ok(()) => Some(id),
        Err(e) => {
            tracing::warn!(error = %e, action_type, "failed to write activity log entry");
            None
        }
    }
}

pub fn router() -> Router<AppState> {
    // Deletion (single or bulk) and changing the retention window are both Session-only — an API
    // token (any role) must never be able to erase its own trail or shorten how long it survives,
    // the same reasoning `database.rs`'s `/database/drop` and `api_tokens.rs`'s whole router
    // already apply to their own irreversible/self-incriminating actions. Enforced by
    // `require_api_key` itself now (issue #91's `route_policy.csv` `deny` rules for these three
    // exact `(role, path, method)` combinations), not a separate `route_layer` here — see
    // `procedure.rs`'s own module docs for why the two enforcement layers were merged into one.
    Router::new()
        .route("/activity", get(list_activity).delete(bulk_delete_activity))
        .route("/activity/facets", get(get_facets))
        .route("/activity/{id}", axum::routing::delete(delete_activity))
        .route("/activity/retention", get(get_retention).put(put_retention))
}

/// Whether `target` still points at a real, undeleted resource — checked live against the
/// database rather than inferred from other activity entries (an `archive.delete` record for the
/// same id might sit on a different page, behind a filter, or have already expired past the
/// retention window, none of which changes whether the archive is actually gone). Only
/// `archive`/`tankoubon` kinds have a real per-resource existence check worth doing here — every
/// other kind (`token`, `category`, `settings`, `plugin`, ...) either always links to a fixed page
/// rather than a specific instance, or already renders as plain text for its own deletion-type
/// entries (`isDeletionActionType`'s own frontend list), so `None` (meaning "not applicable, don't
/// affect rendering") is the right answer for them rather than an extra lookup.
async fn target_exists(state: &AppState, target: &ActivityTarget) -> Option<bool> {
    let id = target.id.as_deref()?;
    match target.kind.as_deref() {
        Some("archive") => Some(
            state
                .repos
                .archives
                .get(&ArchiveId(id.to_string()))
                .await
                .ok()
                .flatten()
                .is_some(),
        ),
        Some("tankoubon") => Some(
            state
                .repos
                .groupings
                .get(&TankId(id.to_string()))
                .await
                .ok()
                .flatten()
                .is_some(),
        ),
        _ => None,
    }
}

async fn entry_json_with_exists(state: &AppState, entry: &ActivityEntry) -> Value {
    let mut value = json!(entry);
    if let Some(exists) = target_exists(state, &entry.target).await {
        value["target"]["exists"] = json!(exists);
    }
    value
}

#[derive(Deserialize)]
struct ListActivityParams {
    cursor: Option<String>,
    limit: Option<isize>,
    start_ts: Option<i64>,
    end_ts: Option<i64>,
    /// Comma-separated — matches this codebase's own established convention for a multi-value
    /// query param (`database.rs`'s `excludednamespaces`, `artist_backfill.rs`, `opds.rs`, ...),
    /// rather than a repeated `?actor=a&actor=b` key (axum's plain `Query` extractor, backed by
    /// `serde_urlencoded`, doesn't collect repeated keys into a `Vec` without pulling in a second
    /// query-string crate this codebase has no other use for). Multiple values are OR'd together
    /// — see `ActivityFilter::actor_keys`'s own docs.
    actor: Option<String>,
    /// Comma-separated, same convention as `actor` above. See `ActivityFilter::action_types`'s
    /// own docs.
    action_type: Option<String>,
}

/// Splits a comma-separated query param into its trimmed, non-empty parts — shared by `actor`/
/// `action_type` above (both use the same convention). `None`/empty input yields an empty `Vec`
/// (no filter on that dimension), matching `ActivityFilter`'s own "empty `Vec` = unfiltered"
/// semantics.
fn split_csv_param(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

/// Clamped to a small, fixed range regardless of what the client asks for — an unbounded `limit`
/// would let a single request force an arbitrarily large `ZREVRANGEBYSCORE ... LIMIT` scan.
const MIN_LIMIT: isize = 1;
const MAX_LIMIT: isize = 200;
const DEFAULT_LIMIT: isize = 50;

async fn list_activity(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Query(params): Query<ListActivityParams>,
) -> Response {
    let limit = params
        .limit
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(MIN_LIMIT, MAX_LIMIT);
    let filter = ActivityFilter {
        start_ts: params.start_ts,
        end_ts: params.end_ts,
        actor_keys: split_csv_param(params.actor.as_deref()),
        action_types: split_csv_param(params.action_type.as_deref()),
    };
    match state
        .activity
        .list_page(&filter, params.cursor.as_deref(), limit)
        .await
    {
        Ok(page) => {
            let visible =
                filter_visible_entries(&state, auth.as_ref().map(|e| &e.0), page.entries).await;
            let entries = join_all(
                visible
                    .iter()
                    .map(|entry| entry_json_with_exists(&state, entry)),
            )
            .await;
            axum::Json(json!({
                "entries": entries,
                "next_cursor": page.next_cursor,
                "total_estimate": page.total_estimate,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_activity",
            e.to_string(),
        ),
    }
}

/// Filters a page of already-fetched entries down to the ones `auth` may actually see — issue
/// #91's own resource-level rule (`session` sees everything, an admin-role token sees its own
/// entries plus every guest-role token's, a guest-role token sees only its own). Runs *after* the
/// storage-level query, not as a Redis-side filter — `ActivityRepository::list_page` has no
/// concept of "visible to whom" at all, and folding that in there would mean every future caller
/// of `list_page` (there's only this one today, but the repository itself is generic storage) has
/// to reason about authorization, not just this one HTTP-facing handler.
///
/// A `Token`-kind entry's own role is looked up once per distinct token id (not once per entry —
/// `role_cache` — since a busy token can easily have several entries on the same page) via
/// `state.api_tokens.get`; a since-revoked token (lookup returns `None`) maps to
/// `authz::can_view_activity_entry`'s own conservative `"token_revoked"` bucket, visible only to
/// `session`.
async fn filter_visible_entries(
    state: &AppState,
    auth: Option<&AuthContext>,
    entries: Vec<ActivityEntry>,
) -> Vec<ActivityEntry> {
    let requester_role = crate::authz::requester_role(auth);
    let requester_id = crate::authz::requester_id(auth);
    let authz = crate::authz::Authz::get().await;

    let mut role_cache: std::collections::HashMap<
        String,
        Option<lanrurugi_storage::api_tokens::TokenRole>,
    > = std::collections::HashMap::new();
    let mut visible = Vec::with_capacity(entries.len());
    for entry in entries {
        let token_role = if entry.actor.kind == lanrurugi_storage::activity::ActorKind::Token {
            let id = entry.actor.id.clone().unwrap_or_default();
            if let Some(role) = role_cache.get(&id) {
                *role
            } else {
                let role = state
                    .api_tokens
                    .get(&id)
                    .await
                    .ok()
                    .flatten()
                    .map(|r| r.role);
                role_cache.insert(id, role);
                role
            }
        } else {
            None
        };
        if crate::authz::can_view_activity_entry(
            &authz.activity,
            requester_role,
            requester_id,
            entry.actor.kind,
            entry.actor.id.as_deref(),
            token_role,
        ) {
            visible.push(entry);
        }
    }
    visible
}

/// Turns a raw `actor_key` string ("session" / "token:<id>" / "system:<x>" / "anonymous" — see
/// `lanrurugi_storage::activity`'s own `actor_index_key` docs for the format) into the
/// structured `{kind, id, display_name}` shape the frontend's facets Combobox renders. Looks up
/// exactly one representative entry from that actor's own index to read a `display_name` back
/// out of, rather than trying to parse one out of the key string itself (which never carries a
/// display name, only an id).
async fn describe_actor_facet(state: &AppState, actor_key: &str) -> Value {
    let (kind, id) = if actor_key == "session" {
        ("session", None)
    } else if actor_key == "anonymous" {
        ("anonymous", None)
    } else if let Some(rest) = actor_key.strip_prefix("token:") {
        ("token", Some(rest.to_string()))
    } else if let Some(rest) = actor_key.strip_prefix("system:") {
        ("system", Some(rest.to_string()))
    } else {
        ("session", None)
    };

    let display_name = match (kind, &id) {
        // A revoked token has no cached name left anywhere in this facet index (only the raw
        // `actor_key`, no name snapshot) — the id itself is genuinely the only thing left to show.
        // Falls back to the bare id, not a baked-in English "(revoked)" suffix: that phrasing is
        // the frontend's own call (`activityTarget.ts`/`ActivityFilterCombobox.tsx`'s
        // `revokedTokenName` i18n string), and baking one in here doubled up with it — a revoked
        // token's chip literally read "<id> (revoked) （已撤销）", confirmed live.
        ("token", Some(token_id)) => state
            .api_tokens
            .get(token_id)
            .await
            .ok()
            .flatten()
            .map(|r| r.name)
            .unwrap_or_else(|| token_id.clone()),
        ("system", Some(subsystem)) => system_actor(subsystem)
            .display_name
            .unwrap_or_else(|| subsystem.clone()),
        _ => kind.to_string(),
    };

    json!({ "kind": kind, "id": id, "display_name": display_name })
}

/// Turns a facet's own `(kind, id)` string pair (already resolved by [`describe_actor_facet`])
/// into the `token_role` [`crate::authz::can_view_activity_entry`] expects — same lookup
/// [`filter_visible_entries`] does per-entry, but there's no page of `ActivityEntry`s here to
/// cache a role lookup across (facets aggregate the *whole* retention window, already one row per
/// distinct actor), so this just looks up fresh each call.
async fn facet_actor_role(
    state: &AppState,
    kind: &str,
    id: Option<&str>,
) -> Option<lanrurugi_storage::api_tokens::TokenRole> {
    if kind != "token" {
        return None;
    }
    let id = id?;
    state
        .api_tokens
        .get(id)
        .await
        .ok()
        .flatten()
        .map(|r| r.role)
}

fn facet_actor_kind(kind: &str) -> lanrurugi_storage::activity::ActorKind {
    use lanrurugi_storage::activity::ActorKind;
    match kind {
        "session" => ActorKind::Session,
        "token" => ActorKind::Token,
        "system" => ActorKind::System,
        _ => ActorKind::Anonymous,
    }
}

async fn get_facets(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    let auth = auth.as_ref().map(|e| &e.0);
    let requester_role = crate::authz::requester_role(auth);
    let requester_id = crate::authz::requester_id(auth);
    let authz = crate::authz::Authz::get().await;

    match state.activity.facets().await {
        Ok(facets) => {
            let action_types: Vec<Value> = facets
                .action_types
                .iter()
                .map(|f| json!({ "value": f.value, "count": f.count }))
                .collect();
            let mut actors = Vec::with_capacity(facets.actors.len());
            for f in &facets.actors {
                let described = describe_actor_facet(&state, &f.value).await;
                let kind = described["kind"].as_str().unwrap_or_default();
                let id = described["id"].as_str();
                let token_role = facet_actor_role(&state, kind, id).await;
                if !crate::authz::can_view_activity_entry(
                    &authz.activity,
                    requester_role,
                    requester_id,
                    facet_actor_kind(kind),
                    id,
                    token_role,
                ) {
                    continue;
                }
                let mut described = described;
                described["count"] = json!(f.count);
                actors.push(described);
            }
            axum::Json(json!({ "action_types": action_types, "actors": actors })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_facets",
            e.to_string(),
        ),
    }
}

async fn delete_activity(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.activity.delete(&id).await {
        Ok(Some(_)) => ok("delete_activity", []),
        Ok(None) => {
            crate::common::not_found("delete_activity", format!("entry {id} does not exist."))
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_activity",
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct BulkDeleteBody {
    ids: Vec<String>,
}

async fn bulk_delete_activity(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<BulkDeleteBody>,
) -> Response {
    match state.activity.delete_many(&body.ids).await {
        Ok(deleted_count) => ok(
            "bulk_delete_activity",
            [("deleted_count", json!(deleted_count))],
        ),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "bulk_delete_activity",
            e.to_string(),
        ),
    }
}

async fn get_retention(State(state): State<AppState>) -> Response {
    match state.activity.retention_secs().await {
        Ok(secs) => axum::Json(json!({ "retention_secs": secs })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_activity_retention",
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct PutRetentionBody {
    /// `None`/omitted means "keep forever" — matches `ActivityRepository::set_retention_secs`'s
    /// own `Option<i64>` semantics (same convention as `api_tokens.rs::CreateTokenBody::
    /// expires_in_secs`), not a sentinel value the client would have to know to send.
    retention_secs: Option<i64>,
}

async fn put_retention(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<PutRetentionBody>,
) -> Response {
    match state.activity.set_retention_secs(body.retention_secs).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                lanrurugi_storage::activity::action_types::SETTINGS_UPDATE,
                ActivityTarget {
                    id: None,
                    label: Some("activity_retention".to_string()),
                    kind: Some("settings".to_string()),
                },
                None,
                Some(json!({ "retention_secs": body.retention_secs })),
            )
            .await;
            ok("put_activity_retention", [])
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "put_activity_retention",
            e.to_string(),
        ),
    }
}
