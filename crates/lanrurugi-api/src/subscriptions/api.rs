//! HTTP surface for subscriptions and the reservation list
//! (`specs/010-subscription-orchestrator/contracts/subscription-api.md`).
//!
//! Every path here is additive and sits outside the legacy path set, so no existing contract
//! changes (constitution Principle II).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use lanrurugi_storage::subscriptions::{
    CreditPolicy, Criteria, Filters, ReservationStatus, Subscription, SubscriptionState,
};

use crate::common::{error, not_found};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/subscriptions", get(list).post(create))
        .route("/subscriptions/sources", get(list_sources))
        .route(
            "/subscriptions/{id}",
            get(detail).put(update).delete(remove),
        )
        .route("/subscriptions/{id}/enable", post(enable))
        .route("/subscriptions/{id}/disable", post(disable))
        .route("/subscriptions/{id}/resume", post(resume))
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
    #[serde(default = "default_true")]
    pub enrich_metadata: bool,
    #[serde(default)]
    pub auto_download: bool,
    #[serde(default)]
    pub credit_policy: CreditPolicy,
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
        sources.push(json!({
            "namespace": ns,
            "suggested_secs": interval.as_ref().map(|i| i.suggested_secs),
            "minimum_secs": interval.as_ref().map(|i| i.minimum_secs),
            "description": interval.as_ref().map(|i| i.description.clone()),
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
        enrich_metadata: body.enrich_metadata,
        auto_download: body.auto_download,
        credit_policy: body.credit_policy,
        state: SubscriptionState::Enabled,
        last_checked_at: None,
        created_at: now_secs(),
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
        enrich_metadata: body.enrich_metadata,
        auto_download: body.auto_download,
        credit_policy: body.credit_policy,
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
