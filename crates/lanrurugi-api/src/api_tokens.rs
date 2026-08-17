//! `GET/POST /api/tokens`, `DELETE /api/tokens/{id}` — first-party API token management
//! (issue #54), replacing legacy's single fixed `apikey` field entirely (constitution Principle
//! II's own annotation covers this deliberate departure). No legacy equivalent — additive,
//! LANrurugi-only.
//!
//! Mounted in the *protected* router group (`crate::router`, not `login::router`'s unprotected
//! one) — creating/listing/revoking tokens is itself an admin action gated by an already-valid
//! session (JWT cookie) or an existing API token, never a bootstrapping-free operation.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use lanrurugi_storage::api_tokens::TokenRole;
use serde::Deserialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found, ok};
use crate::state::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget};

pub fn router() -> Router<AppState> {
    // Token management itself must never be reachable via API-token auth (even an admin-role
    // token) — only a real session-cookie login (a human who's already proven the admin password)
    // may create/list/revoke/rename tokens. Enforced by `require_api_key` itself (issue #91's
    // `route_policy.csv` `deny` rules for `token_admin`/`token_guest` against these routes) — see
    // `procedure.rs`'s own module docs for why this used to be a separate `route_layer` and no
    // longer is.
    Router::new()
        .route("/tokens", get(list_tokens).post(create_token))
        .route(
            "/tokens/{id}",
            axum::routing::delete(delete_token).patch(rename_token),
        )
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs() as i64
}

/// Never includes `token_hash` — see `lanrurugi_storage::api_tokens::ApiTokenRecord`'s own docs
/// on why the raw value can't be recovered, and correspondingly why its hash has no business
/// leaving the server either.
fn token_json(record: &lanrurugi_storage::api_tokens::ApiTokenRecord) -> serde_json::Value {
    json!({
        "id": record.id,
        "name": record.name,
        "created_at": record.created_at,
        "role": record.role,
        "expires_at": record.expires_at,
        "last_used_at": record.last_used_at,
        "last_used_ip": record.last_used_ip,
    })
}

async fn list_tokens(State(state): State<AppState>) -> Response {
    match state.api_tokens.list_all().await {
        Ok(tokens) => {
            let body: Vec<_> = tokens.iter().map(token_json).collect();
            axum::Json(body).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_tokens",
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct CreateTokenBody {
    name: String,
    /// Defaults to `Admin` when omitted — matches `TokenRole`'s own `#[serde(default)]`, so an
    /// older client that doesn't yet know about roles still gets the pre-role-system behavior
    /// (full access) rather than being silently downgraded to `Guest`.
    #[serde(default)]
    role: TokenRole,
    /// Duration in seconds from issuance until this token stops working, or absent/`null` for a
    /// permanent token. The server computes the absolute `expires_at` from its own clock (`now +
    /// expires_in_secs`) rather than accepting a client-supplied absolute timestamp, so a client
    /// with a skewed clock can't mint a token that outlives (or undershoots) what it asked for.
    expires_in_secs: Option<i64>,
}

async fn create_token(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<CreateTokenBody>,
) -> Response {
    let name = body.name.trim();
    if name.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "create_token",
            "name cannot be empty",
        );
    }
    match state
        .api_tokens
        .issue(
            name.to_string(),
            body.role,
            now_secs(),
            body.expires_in_secs,
        )
        .await
    {
        Ok(issued) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::TOKEN_CREATE,
                ActivityTarget {
                    id: Some(issued.record.id.clone()),
                    label: Some(issued.record.name.clone()),
                    kind: Some("token".to_string()),
                },
                None,
                Some(json!({ "role": issued.record.role, "expires_at": issued.record.expires_at })),
            )
            .await;
            let mut body = token_json(&issued.record);
            // The one and only time the raw value is ever sent to a client — see the storage
            // layer's own docs on why it's never stored, and `token_json`'s own docs on why it's
            // otherwise excluded.
            body["token"] = json!(issued.raw_token);
            ok("create_token", [("data", body)])
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "create_token",
            e.to_string(),
        ),
    }
}

async fn delete_token(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    let existing = match state.api_tokens.get(&id).await {
        Ok(None) => return not_found("delete_token", format!("token {id} does not exist.")),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_token",
                e.to_string(),
            )
        }
        Ok(Some(record)) => record,
    };
    match state.api_tokens.delete(&id).await {
        Ok(()) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::TOKEN_REVOKE,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: Some(existing.name.clone()),
                    kind: Some("token".to_string()),
                },
                None,
                None,
            )
            .await;
            ok("delete_token", [])
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_token",
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct RenameTokenBody {
    name: String,
}

async fn rename_token(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<RenameTokenBody>,
) -> Response {
    let name = body.name.trim();
    if name.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "rename_token",
            "name cannot be empty",
        );
    }
    let old_name = state
        .api_tokens
        .get(&id)
        .await
        .ok()
        .flatten()
        .map(|r| r.name);
    match state.api_tokens.rename(&id, name.to_string()).await {
        Ok(None) => not_found("rename_token", format!("token {id} does not exist.")),
        Ok(Some(record)) => {
            record_manual(
                &state,
                auth.as_ref().map(|e| &e.0),
                action_types::TOKEN_RENAME,
                ActivityTarget {
                    id: Some(id.clone()),
                    label: Some(record.name.clone()),
                    kind: Some("token".to_string()),
                },
                old_name.map(|n| json!({ "name": n })),
                Some(json!({ "name": record.name })),
            )
            .await;
            ok("rename_token", [("data", token_json(&record))])
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rename_token",
            e.to_string(),
        ),
    }
}
