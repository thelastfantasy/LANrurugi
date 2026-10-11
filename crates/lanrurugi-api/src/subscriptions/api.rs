//! HTTP surface for subscriptions and the reservation list
//! (`specs/010-subscription-orchestrator/contracts/subscription-api.md`).
//!
//! Every path here is additive and sits outside the legacy path set, so no existing contract
//! changes (constitution Principle II).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use lanrurugi_storage::subscriptions::{
    CreditPolicy, Criteria, Filters, ReservationStatus, SourceChangePolicy, SourceRemovalPolicy,
    Subscription, SubscriptionState,
};

use crate::common::{error, not_found};

use super::ai_condition;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/subscriptions", get(list).post(create))
        .route("/subscriptions/sources", get(list_sources))
        // Registered before the `{id}` routes so the literal segments are unambiguous to read, even
        // though the router would prefer them anyway.
        .route("/subscriptions/pending", get(list_pending))
        .route("/subscriptions/pending/approve", post(approve_pending))
        .route("/subscriptions/pending/dismiss", post(dismiss_pending))
        // Before the `{id}` routes, like the pending ones above: Axum prefers a literal segment
        // anyway, but keeping them together makes it visible which paths are not ids.
        .route("/subscriptions/preview", post(preview_draft))
        .route("/subscriptions/preview/judge", post(preview_judge))
        .route("/subscriptions/condition/ai", post(ai_condition::generate))
        .route("/subscriptions/condition/parse", post(ai_condition::parse))
        .route(
            "/subscriptions/{id}",
            get(detail).put(update).delete(remove),
        )
        .route("/subscriptions/{id}/enable", post(enable))
        .route("/subscriptions/{id}/disable", post(disable))
        .route("/subscriptions/{id}/resume", post(resume))
        .route("/subscriptions/{id}/preview", post(preview))
        .route("/subscriptions/{id}/check", post(check_now))
        .route("/subscriptions/{id}/forget", post(forget_seen))
        .route("/subscriptions/{id}/block", post(block_sources))
        .route("/subscriptions/tracking", get(tracking_for))
        .route("/subscriptions/history", get(combined_history))
        .route("/subscriptions/blocked", get(blocked_works))
        .route("/reservations", get(list_reservations))
        .route("/reservations/{id}/discard", post(discard_reservation))
}

#[derive(Debug, Deserialize)]
pub struct SubscriptionBody {
    pub name: String,
    pub source: String,
    #[serde(default)]
    pub criteria: Criteria,
    #[serde(default)]
    pub filters: Filters,
    pub interval_secs: u64,
    #[serde(default)]
    pub target_category: Option<String>,
    /// Extra tags to merge onto every archive this subscription downloads, in addition to any
    /// metadata plugin's own tags. The backend deduplicates these against the archive's existing
    /// tags when applying them.
    #[serde(default)]
    pub metadata_tags: Vec<String>,
    #[serde(default = "default_true")]
    pub enrich_metadata: bool,
    #[serde(default)]
    pub auto_download: bool,
    #[serde(default)]
    pub credit_policy: CreditPolicy,
    /// What this subscription's downloads do when a filename collides with an existing archive.
    #[serde(default)]
    pub conflict_policy: lanrurugi_storage::subscriptions::ConflictPolicy,
    /// What to do when a tracked work's title or tags change at the source. Defaults to the least
    /// destructive option (FR-012c).
    /// Whether it starts enabled. Defaults to on, which is what creating one usually means — but a
    /// rule can also be written now and switched on once its preview looks right.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Cookies/headers for a source whose login plugin is missing. Optional, and empty by default.
    #[serde(default)]
    pub credentials: lanrurugi_storage::subscriptions::Credentials,
    #[serde(default)]
    pub on_source_changed: SourceChangePolicy,
    /// What to do when a tracked work disappears from the listing. No variant deletes a held
    /// archive (FR-012d).
    #[serde(default)]
    pub on_source_removed: SourceRemovalPolicy,
}

fn default_true() -> bool {
    true
}

/// Reads a source extension's declared interval floor, if it declares one.
///
/// `None` means the source offered no guidance, in which case the user's choice stands unchanged
/// (FR-004c) — absence of advice is not a reason to impose a number of our own.
async fn declared_minimum_interval(state: &AppState, source: &str) -> Option<u64> {
    state
        .plugins
        .plugin_options(source)
        .await
        .ok()
        .flatten()
        .and_then(|o| o.check_interval)
        .map(|i| i.minimum_secs)
}

/// Whether this source can back a subscription at all (FR-002b).
async fn offers_discovery(state: &AppState, source: &str) -> bool {
    state
        .plugins
        .plugin_introspect(source)
        .await
        .map(|i| i.exports_discover)
        .unwrap_or(false)
}

/// Validates a submitted subscription against its source.
///
/// Both failures **refuse with a stated reason** rather than quietly adjusting the input. A silently
/// raised interval would leave the user believing checks happen more often than they do, and
/// misreading every later result; a silently accepted source with no discovery would look fine until
/// its first scheduled check failed, hours later and unattended.
async fn validate(state: &AppState, body: &SubscriptionBody) -> Result<(), Response> {
    if !offers_discovery(state, &body.source).await {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "create_subscription",
            format!(
                "The source {:?} cannot discover new works, so it cannot back a subscription.",
                body.source
            ),
        ));
    }
    if let Some(minimum) = declared_minimum_interval(state, &body.source).await {
        if body.interval_secs < minimum {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "create_subscription",
                format!(
                    "This source asks for at least {minimum} seconds between checks; {} is too frequent.",
                    body.interval_secs
                ),
            ));
        }
    }
    Ok(())
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn list(State(state): State<AppState>) -> Response {
    match state.subscriptions.list_all().await {
        Ok(items) => axum::Json(json!({ "subscriptions": items })).into_response_owned(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_subscriptions",
            e.to_string(),
        ),
    }
}

/// Which sources can back a subscription, with their interval bounds — so the UI can offer only
/// viable sources and show the bounds *before* the user submits, rather than teaching them the
/// rules through a rejection.
async fn list_sources(State(state): State<AppState>) -> Response {
    let namespaces = crate::plugins::discover_namespaces(&state.plugins_dir).await;
    let mut sources = Vec::new();
    for ns in namespaces {
        if !offers_discovery(&state, &ns).await {
            continue;
        }
        let interval = state
            .plugins
            .plugin_options(&ns)
            .await
            .ok()
            .flatten()
            .and_then(|o| o.check_interval);
        // Which fields this source populates, so the form offers filters only over fields that will
        // actually be there (FR-011a). Derived, never stored per subscription — a stored copy would be
        // a second separately-derived answer to the same question.
        let candidate_fields = state
            .plugins
            .plugin_introspect(&ns)
            .await
            .map(|i| i.candidate_fields)
            .unwrap_or_default();
        // Which login and download plugin this source would actually use. Resolved rather than
        // merely declared: an extension names a `login_from` namespace, and whether anything installed
        // answers to it is the part worth showing — a missing one degrades silently to a signed-out
        // check, which returns a smaller listing rather than an error.
        let info = state.plugins.plugin_info(&ns).await.ok();
        let login = match info.as_ref().and_then(|i| i.login_from.as_ref()) {
            Some(declared) => {
                match crate::plugins::resolve_declared_namespace(&state, declared).await {
                    Some((resolved, _)) => json!({ "declared": declared, "resolved": resolved }),
                    None => json!({ "declared": declared, "resolved": null }),
                }
            }
            None => serde_json::Value::Null,
        };

        // The download plugin is chosen per URL at download time, so the best that can be said here is
        // which one matches the source's own domain. Download-kind plugins only, and by domain rather
        // than by URL: login/metadata/discovery plugins declare the same domains, so an unfiltered
        // lookup could name one of those as "the download plugin this source uses".
        let download = match info.as_ref().and_then(|i| i.domain_match.first()) {
            Some(domain) => crate::plugins::resolve_download_plugin_for_domain(&state, domain)
                .await
                .map(|(resolved, _)| serde_json::Value::String(resolved))
                .unwrap_or(serde_json::Value::Null),
            None => serde_json::Value::Null,
        };

        sources.push(json!({
            "namespace": ns,
            "login_plugin": login,
            "download_plugin": download,
            "suggested_secs": interval.as_ref().map(|i| i.suggested_secs),
            "minimum_secs": interval.as_ref().map(|i| i.minimum_secs),
            "description": interval.as_ref().map(|i| i.description.clone()),
            "candidate_fields": candidate_fields,
        }));
    }
    axum::Json(json!({ "sources": sources })).into_response_owned()
}

async fn create(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<SubscriptionBody>,
) -> Response {
    if let Err(resp) = validate(&state, &body).await {
        return resp;
    }
    let subscription = Subscription {
        id: uuid::Uuid::new_v4().to_string(),
        name: body.name,
        source: body.source,
        criteria: body.criteria,
        filters: body.filters,
        interval_secs: body.interval_secs,
        target_category: body.target_category,
        metadata_tags: body.metadata_tags,
        enrich_metadata: body.enrich_metadata,
        auto_download: body.auto_download,
        credit_policy: body.credit_policy,
        conflict_policy: body.conflict_policy,
        credentials: body.credentials,
        on_source_changed: body.on_source_changed,
        on_source_removed: body.on_source_removed,
        state: if body.enabled {
            SubscriptionState::Enabled
        } else {
            SubscriptionState::Disabled
        },
        last_checked_at: None,
        created_at: now_secs(),
        // Starts the settle window: a rule saved a moment ago is the one most likely to be wrong.
        settings_changed_at: Some(now_secs()),
    };
    match state.subscriptions.save(&subscription).await {
        Ok(()) => axum::Json(json!({ "subscription": subscription })).into_response_owned(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "create_subscription",
            e.to_string(),
        ),
    }
}

async fn detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.subscriptions.get(&id).await {
        Ok(Some(s)) => {
            let cycles = state
                .subscriptions
                .recent_cycles(&id, 20)
                .await
                .unwrap_or_default();
            axum::Json(json!({ "subscription": s, "cycles": cycles })).into_response_owned()
        }
        Ok(None) => not_found("get_subscription", format!("{id} does not exist.")),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_subscription",
            e.to_string(),
        ),
    }
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<SubscriptionBody>,
) -> Response {
    let Ok(Some(existing)) = state.subscriptions.get(&id).await else {
        return not_found("update_subscription", format!("{id} does not exist."));
    };
    if let Err(resp) = validate(&state, &body).await {
        return resp;
    }
    let updated = Subscription {
        name: body.name,
        source: body.source,
        criteria: body.criteria,
        filters: body.filters,
        interval_secs: body.interval_secs,
        target_category: body.target_category,
        metadata_tags: body.metadata_tags,
        enrich_metadata: body.enrich_metadata,
        auto_download: body.auto_download,
        credit_policy: body.credit_policy,
        conflict_policy: body.conflict_policy,
        credentials: body.credentials,
        on_source_changed: body.on_source_changed,
        on_source_removed: body.on_source_removed,
        // Restarts the settle window on every edit, not only on creation: changing a rule is as
        // likely to be a mistake as writing one, and the check that follows is just as unattended.
        settings_changed_at: Some(now_secs()),
        ..existing
    };
    match state.subscriptions.save(&updated).await {
        Ok(()) => axum::Json(json!({ "subscription": updated })).into_response_owned(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_subscription",
            e.to_string(),
        ),
    }
}

async fn remove(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.subscriptions.delete(&id).await {
        Ok(()) => axum::Json(json!({ "operation": "delete_subscription", "success": 1 }))
            .into_response_owned(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_subscription",
            e.to_string(),
        ),
    }
}

async fn set_state(state: &AppState, id: &str, new_state: SubscriptionState, op: &str) -> Response {
    match state.subscriptions.get(id).await {
        Ok(Some(mut s)) => {
            s.state = new_state;
            match state.subscriptions.save(&s).await {
                Ok(()) => axum::Json(json!({ "subscription": s })).into_response_owned(),
                Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, op, e.to_string()),
            }
        }
        Ok(None) => not_found(op, format!("{id} does not exist.")),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, op, e.to_string()),
    }
}

async fn enable(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_state(
        &state,
        &id,
        SubscriptionState::Enabled,
        "enable_subscription",
    )
    .await
}

async fn disable(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_state(
        &state,
        &id,
        SubscriptionState::Disabled,
        "disable_subscription",
    )
    .await
}

/// Clears a system-applied pause. Distinct from `enable`, because a paused subscription was stopped
/// *by the system* and owes the user an explanation — resuming is the user answering it.
async fn resume(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    set_state(
        &state,
        &id,
        SubscriptionState::Enabled,
        "resume_subscription",
    )
    .await
}

/// Every work the user has explicitly blocked, newest first, across all subscriptions.
///
/// Its own endpoint rather than a filter over the history: a block is an *answer* — the user said no
/// to this work — and the history is a log of decisions that keeps being rewritten by later cycles
/// (a work queued and then catalogued reads `already_held` in every record). Only the block list
/// stays exactly as the user left it, which is what makes it worth showing on its own and undoing
/// from there.
async fn blocked_works(State(state): State<AppState>) -> Response {
    let Ok(subscriptions) = state.subscriptions.list_all().await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "blocked_works",
            "could not read subscriptions",
        );
    };

    let mut entries: Vec<serde_json::Value> = Vec::new();
    for subscription in &subscriptions {
        let Ok(blocked) = state.subscriptions.blocked_all(&subscription.id).await else {
            continue;
        };
        for (source, at) in blocked {
            entries.push(json!({
                "subscription_id": subscription.id,
                "subscription_name": subscription.name,
                "source_url": source,
                "blocked_at": at,
            }));
        }
    }

    entries.sort_by_key(|e| std::cmp::Reverse(e["blocked_at"].as_i64().unwrap_or(0)));
    axum::Json(json!({ "blocked": entries })).into_response_owned()
}

/// Every subscription's recent candidates, newest first, as one stream.
///
/// Per-subscription history already exists, but it answers "what did *this* rule do" — the question
/// actually asked is usually the other one: what has arrived lately, and why did something expected
/// not. That needs the subscriptions interleaved rather than read one at a time.
async fn combined_history(
    State(state): State<AppState>,
    Query(params): Query<HistoryQuery>,
) -> Response {
    let Ok(subscriptions) = state.subscriptions.list_all().await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "subscription_history",
            "could not read subscriptions",
        );
    };

    // What became of the works this subscription queued. A cycle records the *decision* — "queued" —
    // and nothing afterwards updates it, so a download that then failed still reads as queued unless
    // the queue itself is consulted. Joined here rather than copied into the cycle record, which
    // would be a second copy of a state that keeps changing.
    let queue = state.download_queue.list_all().await.unwrap_or_default();
    let outcome_by_source: std::collections::HashMap<String, serde_json::Value> = queue
        .iter()
        .filter(|item| item.subscription_id.is_some())
        .map(|item| {
            (
                crate::plugins::trim_url(&item.url),
                json!({
                    "id": item.id,
                    "state": item.state,
                    "error": item.error,
                    // Restartable states, so the interface can offer a retry rather than only naming
                    // the failure (`is_startable`'s own set).
                    "can_retry": matches!(
                        item.state,
                        lanrurugi_storage::download_queue::DownloadQueueState::Error
                            | lanrurugi_storage::download_queue::DownloadQueueState::Cancelled
                    ),
                }),
            )
        })
        .collect();

    // Per subscription, because that is how cycles are stored; merged afterwards by time.
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for subscription in &subscriptions {
        let cycles = state
            .subscriptions
            .recent_cycles(&subscription.id, CYCLES_PER_SUBSCRIPTION)
            .await
            .unwrap_or_default();
        for cycle in cycles {
            for record in &cycle.candidates {
                entries.push(json!({
                    "subscription_id": subscription.id,
                    "subscription_name": subscription.name,
                    "checked_at": cycle.finished_at,
                    "outcome": cycle.outcome,
                    "candidate": record,
                    // Absent for anything never queued — rejected, still waiting, already held.
                    "download": outcome_by_source.get(&record.source_url),
                }));
            }
        }
    }

    entries.sort_by_key(|e| std::cmp::Reverse(e["checked_at"].as_i64().unwrap_or(0)));

    // One row per work per subscription, newest cycle first: the same listing is read every cycle, so
    // without this a single cycle's 400 records fill the window on their own and the reader sees less
    // than one check — which is how a "queued" row from an older cycle could vanish entirely. The
    // newest record is the one that describes the work's current state; a cycle's own verdicts are
    // still what they are, and `download` carries what became of the decision.
    let mut seen: std::collections::HashSet<(String, String)> = Default::default();
    entries.retain(|e| {
        let key = (
            e["subscription_id"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            e["candidate"]["source_url"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        );
        seen.insert(key)
    });

    let limit = params.limit.unwrap_or(1000).min(1000);
    entries.truncate(limit);

    axum::Json(json!({ "history": entries })).into_response_owned()
}

/// How far back to read per subscription before merging. A ceiling rather than everything: history is
/// bounded by age anyway, and reading all of it for every subscription to then discard most of it
/// would cost more the longer the install has been running.
const CYCLES_PER_SUBSCRIPTION: isize = 20;

#[derive(Debug, Deserialize)]
struct HistoryQuery {
    limit: Option<usize>,
}

/// Which subscriptions have already handled a given work, by its source URL.
///
/// Asked before deleting an archive, so the delete flow only offers "stop this coming back" for an
/// archive a subscription actually brought in — offering it for a hand-uploaded file would be an
/// option with no effect, which is worse than no option at all.
async fn tracking_for(
    State(state): State<AppState>,
    Query(params): Query<TrackingQuery>,
) -> Response {
    let source = crate::plugins::trim_url(&params.source);
    let Ok(subscriptions) = state.subscriptions.list_all().await else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "subscription_tracking",
            "could not read subscriptions",
        );
    };

    let mut tracking = Vec::new();
    for subscription in subscriptions {
        if state
            .subscriptions
            .is_seen(&subscription.id, &source)
            .await
            .unwrap_or(false)
        {
            tracking.push(json!({ "id": subscription.id, "name": subscription.name }));
        }
    }
    axum::Json(json!({ "tracking": tracking })).into_response_owned()
}

#[derive(Debug, Deserialize)]
struct TrackingQuery {
    source: String,
}

/// Makes a subscription reconsider works it has already handled.
///
/// Being seen is otherwise permanent, which is correct for the ordinary case — a work the user
/// deleted on purpose must not come back on the next check. But two situations need it undone: a
/// deletion made by mistake, and a rule that has since been widened (works rejected under the old
/// rule were recorded as seen at the moment they were rejected, so nothing would reconsider them).
///
/// With `sources`, forgets exactly those. With `all: true`, clears the set — kept as a separate,
/// explicit request because on a long-running subscription it can match the entire back catalogue at
/// once, which is a different order of consequence.
async fn forget_seen(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<ForgetBody>,
) -> Response {
    if state.subscriptions.get(&id).await.ok().flatten().is_none() {
        return not_found("forget_seen", format!("{id} does not exist."));
    }

    let outcome = if body.all {
        state.subscriptions.forget_all_seen(&id).await
    } else if body.sources.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "forget_seen",
            "Name the works to forget, or pass `all` to clear the whole set.",
        );
    } else {
        state.subscriptions.forget_seen(&id, &body.sources).await
    };

    match outcome {
        Ok(()) => axum::Json(json!({
            "operation": "forget_seen",
            "success": 1,
            "forgotten": if body.all { None } else { Some(body.sources.len()) },
        }))
        .into_response_owned(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "forget_seen",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct BlockBody {
    #[serde(default)]
    sources: Vec<String>,
}

/// The user's own "no": these works are never downloaded by this subscription again.
///
/// Distinct from a rule rejection on purpose — a rule reads a value that can change, so a rejection is
/// provisional and decided afresh every cycle (FR-011i), while this is an answer, and re-deciding it
/// would ask again what the user already said. The preview offers it per row; `forget_seen` undoes it.
async fn block_sources(
    State(state): State<AppState>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<BlockBody>,
) -> Response {
    if state.subscriptions.get(&id).await.ok().flatten().is_none() {
        return not_found(
            "block_subscription_sources",
            format!("{id} does not exist."),
        );
    }
    if body.sources.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "block_subscription_sources",
            "Name at least one work to block.",
        );
    }

    match state.subscriptions.block(&id, &body.sources).await {
        Ok(()) => axum::Json(json!({
            "operation": "block_subscription_sources",
            "success": 1,
            "blocked": body.sources.len(),
        }))
        .into_response_owned(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "block_subscription_sources",
            e.to_string(),
        ),
    }
}

/// Re-judges a listing already fetched, against possibly-different conditions.
///
/// Changing a condition changes nothing about what the source would return — only `criteria` shapes
/// the request — so re-fetching would be wasted, slow, and one more hit on a source that rate-limits.
/// The judgment still happens here rather than in the browser, because a second implementation of the
/// condition tree would eventually disagree with the real one.
async fn preview_judge(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<JudgeBody>,
) -> Response {
    let subscription = draft_subscription(body.subscription);
    let decision = super::runner::preview_decide(&state, &subscription, &body.listing).await;
    axum::Json(json!({
        "outcome": decision.outcome,
        "candidates": decision.records,
        // A preview reads a narrower window than a scheduled check (see
        // `runner::PREVIEW_LISTING_PAGES`), and the pane says so rather than letting the shorter
        // list read as the whole result set. The data itself is always a live fetch — nothing
        // here is replayed from a stored copy the source has not just confirmed.
        "listing_pages": super::runner::PREVIEW_LISTING_PAGES,
        "check_listing_pages": super::runner::MAX_LISTING_PAGES,
        "would_act_on": decision.actionable.len(),
    }))
    .into_response_owned()
}

#[derive(Debug, Deserialize)]
struct JudgeBody {
    subscription: SubscriptionBody,
    /// The listing a previous preview returned, replayed rather than fetched again.
    listing: lanrurugi_plugin::protocol::DiscoveryResult,
}

/// A throwaway `Subscription` standing in for one being drafted. Nothing reads its id: a draft has no
/// history, no seen set and no pending approvals to look up.
fn draft_subscription(body: SubscriptionBody) -> Subscription {
    Subscription {
        id: String::new(),
        name: body.name,
        source: body.source,
        criteria: body.criteria,
        filters: body.filters,
        interval_secs: body.interval_secs,
        target_category: body.target_category,
        metadata_tags: body.metadata_tags,
        enrich_metadata: body.enrich_metadata,
        auto_download: body.auto_download,
        credit_policy: body.credit_policy,
        conflict_policy: body.conflict_policy,
        credentials: body.credentials,
        on_source_changed: body.on_source_changed,
        on_source_removed: body.on_source_removed,
        state: SubscriptionState::Enabled,
        last_checked_at: None,
        created_at: now_secs(),
        // A draft is never scheduled, so it has no settle window to observe.
        settings_changed_at: None,
    }
}

/// Previews a subscription that does not exist yet.
///
/// The id-bearing route cannot serve this: there is no record to read. Yet a subscription being
/// created is exactly when "what would this actually catch" matters most — afterwards the user has
/// already committed to the rule. The draft is evaluated against the source as it is now, and nothing
/// is stored either way.
async fn preview_draft(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<SubscriptionBody>,
) -> Response {
    if let Err(resp) = validate(&state, &body).await {
        return resp;
    }
    // A throwaway subscription standing in for the one about to be created. Nothing reads its id,
    // because a draft has no history, no seen set and no pending approvals to look up.
    let subscription = Subscription {
        id: String::new(),
        name: body.name,
        source: body.source,
        criteria: body.criteria,
        filters: body.filters,
        interval_secs: body.interval_secs,
        target_category: body.target_category,
        metadata_tags: body.metadata_tags,
        enrich_metadata: body.enrich_metadata,
        auto_download: body.auto_download,
        credit_policy: body.credit_policy,
        conflict_policy: body.conflict_policy,
        credentials: body.credentials,
        on_source_changed: body.on_source_changed,
        on_source_removed: body.on_source_removed,
        state: SubscriptionState::Enabled,
        last_checked_at: None,
        created_at: now_secs(),
        settings_changed_at: None,
    };

    match super::runner::preview_check_raw(&state, &subscription).await {
        Ok((decision, listing)) => axum::Json(json!({
            "outcome": decision.outcome,
            "candidates": decision.records,
        // A preview reads a narrower window than a scheduled check (see
        // `runner::PREVIEW_LISTING_PAGES`), and the pane says so rather than letting the shorter
        // list read as the whole result set. The data itself is always a live fetch — nothing
        // here is replayed from a stored copy the source has not just confirmed.
        "listing_pages": super::runner::PREVIEW_LISTING_PAGES,
        "check_listing_pages": super::runner::MAX_LISTING_PAGES,
            "would_act_on": decision.actionable.len(),
            // Handed back so changing a *condition* can be re-judged against this same listing: only
            // `criteria` shapes the request, so a rule change should not fetch the source again.
            "listing": listing,
        }))
        .into_response_owned(),
        Err(e) => error(StatusCode::BAD_GATEWAY, "preview_subscription", e),
    }
}

/// What a check would do, without doing it.
///
/// Deliberately works on a **disabled** subscription: "what would this rule actually catch" is asked
/// precisely while tuning one that is switched off. Nothing is queued, recorded as seen, or logged,
/// so it is safe to run repeatedly — the only cost is one request to the source.
///
/// Accepts an optional body carrying an unsaved subscription, so a rule can be previewed before it is
/// committed. Without that, a user would have to save a rule to find out what it does.
async fn preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Option<axum::Json<SubscriptionBody>>,
) -> Response {
    let Ok(Some(saved)) = state.subscriptions.get(&id).await else {
        return not_found("preview_subscription", format!("{id} does not exist."));
    };
    // The draft's rules over the saved subscription's identity: previewing must not be able to
    // retarget a different source or a different id.
    let subscription = match body {
        Some(axum::Json(draft)) => Subscription {
            criteria: draft.criteria,
            filters: draft.filters,
            ..saved
        },
        None => saved,
    };

    match super::runner::preview_check_raw(&state, &subscription).await {
        Ok((decision, listing)) => axum::Json(json!({
            "outcome": decision.outcome,
            "candidates": decision.records,
        // A preview reads a narrower window than a scheduled check (see
        // `runner::PREVIEW_LISTING_PAGES`), and the pane says so rather than letting the shorter
        // list read as the whole result set. The data itself is always a live fetch — nothing
        // here is replayed from a stored copy the source has not just confirmed.
        "listing_pages": super::runner::PREVIEW_LISTING_PAGES,
        "check_listing_pages": super::runner::MAX_LISTING_PAGES,
            "would_act_on": decision.actionable.len(),
            // Handed back so changing a *condition* can be re-judged against this same listing: only
            // `criteria` shapes the request, so a rule change should not fetch the source again.
            "listing": listing,
        }))
        .into_response_owned(),
        Err(e) => error(StatusCode::BAD_GATEWAY, "preview_subscription", e),
    }
}

/// Runs a real check now rather than waiting for the schedule (FR-024).
///
/// Spawned rather than awaited: a check contacts the source and may page through it, which is longer
/// than a request should hold open. The result shows up in the subscription's own history.
async fn check_now(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let Ok(Some(_)) = state.subscriptions.get(&id).await else {
        return not_found("check_subscription", format!("{id} does not exist."));
    };
    let spawned = state.clone();
    tokio::spawn(async move {
        super::runner::run_check(&spawned, &id).await;
    });
    axum::Json(json!({ "operation": "check_subscription", "success": 1 })).into_response_owned()
}

/// Matched works waiting for the user's go-ahead. Only reachable because `auto_download` defaults
/// off — without this, that default would leave candidates recorded but unanswerable.
async fn list_pending(State(state): State<AppState>) -> Response {
    match state.subscriptions.list_pending().await {
        Ok(items) => axum::Json(json!({ "pending": items })).into_response_owned(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_pending",
            e.to_string(),
        ),
    }
}

/// Which already-handled works a subscription should reconsider.
#[derive(Debug, Deserialize)]
struct ForgetBody {
    #[serde(default)]
    sources: Vec<String>,
    /// Clears the whole seen set rather than named works.
    #[serde(default)]
    all: bool,
}

/// Which pending items an approve/dismiss call applies to. A list, not a single id, because FR-007b
/// requires acting on matches "individually or together" — one id is just a list of length one.
#[derive(Debug, Deserialize)]
struct PendingIds {
    ids: Vec<String>,
}

/// Approving is the moment unattended matching becomes attended spending — nothing before this point
/// consumes download credit.
///
/// Partial success is reported rather than rolled back: with several items, one that fails to queue
/// is no reason to un-approve the rest, and the caller is told exactly which ones did not make it.
async fn approve_pending(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<PendingIds>,
) -> Response {
    let mut approved = 0usize;
    let mut failures: Vec<serde_json::Value> = Vec::new();

    for id in &body.ids {
        let Ok(Some(entry)) = state.subscriptions.get_pending(id).await else {
            failures.push(json!({ "id": id, "error": "no such pending approval" }));
            continue;
        };
        let Ok(Some(subscription)) = state.subscriptions.get(&entry.subscription_id).await else {
            failures.push(json!({ "id": id, "error": "the subscription no longer exists" }));
            continue;
        };

        // Same resolution the unattended runner performs — the subscription's `source` is the
        // discovery plugin, which has no `execDownload`; the download plugin is what owns this URL,
        // and every download-plugin setting is keyed off its namespace. See
        // `plugins::resolve_download_plugin_for_url`.
        let Some((download_namespace, _)) =
            crate::plugins::resolve_download_plugin_for_url(&state, &entry.source_url).await
        else {
            // Left pending rather than dismissed: installing the missing plugin is a real fix, and
            // the user's answer should survive until then.
            failures.push(json!({
                "id": id,
                "error": "no installed download plugin matches this source URL",
            }));
            continue;
        };
        let overwrite_on_duplicate = super::overwrite_under_conflict_policy(
            subscription.conflict_policy,
            crate::plugins::resolve_queue_overwrite_on_duplicate(&state, &download_namespace).await,
        );

        let queued = state
            .download_queue
            .add(lanrurugi_storage::download_queue::NewQueueItem {
                origin: lanrurugi_storage::download_queue::QueueItemOrigin::Download,
                subscription_id: Some(entry.subscription_id.clone()),
                url: entry.source_url.clone(),
                plugin_namespace: download_namespace,
                file_size: None,
                category: subscription.target_category.clone(),
                metadata_tags: subscription.metadata_tags.clone(),
                auto_fetch_metadata: subscription.enrich_metadata,
                overwrite_on_duplicate,
                conflict_policy: subscription.conflict_policy,
                // Enters the queue exactly as a manually added URL would, inheriting the existing
                // start/stop/retry behaviour rather than getting a path of its own.
                state: lanrurugi_storage::download_queue::DownloadQueueState::Queued,
            })
            .await;

        match queued {
            Ok(item) => {
                // Deleted only after the queue accepted it; the other order would lose the work
                // entirely while the user believed they had approved it.
                let _ = state.subscriptions.delete_pending(id).await;
                approved += 1;
                // Approving *is* the decision this download was waiting for, so it starts here
                // rather than parking as a second, redundant "press Start" step — see
                // `download_queue::start_item_from_subscription`'s own docs.
                if let Err(e) =
                    crate::download_queue::start_item_from_subscription(&state, &item.id).await
                {
                    // Still queued and startable from the upload page, so this is a warning.
                    failures.push(json!({ "id": id, "error": e, "queued": true }));
                }
            }
            Err(e) => failures.push(json!({ "id": id, "error": e.to_string() })),
        }
    }

    axum::Json(json!({
        "operation": "approve_pending",
        "success": 1,
        "approved": approved,
        "failures": failures,
    }))
    .into_response_owned()
}

/// Dismissing marks each work seen, so later checks do not raise the same question again. This is
/// the user answering "no" — distinct from a reservation, which is the system reporting it *could*
/// not download something.
async fn dismiss_pending(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<PendingIds>,
) -> Response {
    let mut dismissed = 0usize;
    for id in &body.ids {
        let Ok(Some(entry)) = state.subscriptions.get_pending(id).await else {
            continue;
        };
        let _ = state
            .subscriptions
            .mark_seen(
                &entry.subscription_id,
                std::slice::from_ref(&entry.source_url),
            )
            .await;
        let _ = state.subscriptions.delete_pending(id).await;
        dismissed += 1;
    }
    axum::Json(json!({
        "operation": "dismiss_pending",
        "success": 1,
        "dismissed": dismissed,
    }))
    .into_response_owned()
}

async fn list_reservations(State(state): State<AppState>) -> Response {
    match state.subscriptions.list_reservations().await {
        Ok(items) => {
            // Only waiting entries are the user's to-do list; discarded ones are retained purely so
            // later cycles know not to re-offer that work (FR-016), and showing them would turn a
            // settled decision back into an open question.
            let waiting: Vec<_> = items
                .into_iter()
                .filter(|e| e.status == ReservationStatus::Waiting)
                .collect();
            axum::Json(json!({ "reservations": waiting })).into_response_owned()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_reservations",
            e.to_string(),
        ),
    }
}

async fn discard_reservation(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match state.subscriptions.get_reservation(&id).await {
        Ok(Some(mut entry)) => {
            // Marked, not deleted — a deleted entry would let the next cycle re-offer the very work
            // the user just refused.
            entry.status = ReservationStatus::Discarded;
            match state.subscriptions.save_reservation(&entry).await {
                Ok(()) => axum::Json(json!({ "operation": "discard_reservation", "success": 1 }))
                    .into_response_owned(),
                Err(e) => error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "discard_reservation",
                    e.to_string(),
                ),
            }
        }
        Ok(None) => not_found("discard_reservation", format!("{id} does not exist.")),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "discard_reservation",
            e.to_string(),
        ),
    }
}

/// Local shim so the handlers above read uniformly; `axum::Json` already implements `IntoResponse`.
trait IntoResponseOwned {
    fn into_response_owned(self) -> Response;
}

impl<T: serde::Serialize> IntoResponseOwned for axum::Json<T> {
    fn into_response_owned(self) -> Response {
        axum::response::IntoResponse::into_response(self)
    }
}
