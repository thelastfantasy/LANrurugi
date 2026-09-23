//! `GET/PATCH/DELETE /api/sessions` — active login-device management for the Settings UI.
//!
//! One "session" is one rotating refresh-token family (created by a successful password login and
//! renewed by `/token/refresh`). Listing never exposes token ids/secrets, only the family id plus
//! display metadata (auto-generated or custom name, last IP, timestamps, and whether the caller
//! itself is currently using that family).
//!
//! Every route here is session-cookie-only: even an Admin-role API token must not be able to
//! enumerate or revoke login devices (`route_policy.csv` has explicit deny rules for both token
//! roles), matching the same "account-security surface" boundary already enforced for password
//! changes and token management.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::{error, not_found, ok};
use crate::state::AppState;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};

pub fn router() -> Router<AppState> {
    Router::new().route("/sessions", get(list_sessions)).route(
        "/sessions/{family_id}",
        axum::routing::patch(rename_session).delete(revoke_session),
    )
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs() as i64
}

/// Access-token-based identification of the family making this request. The access token is
/// stateless and carries `fid`; using `family_id_ignoring_expiry` means an expired-but-signed
/// access token can still say "this is the current device", which is exactly the display/diagnostic
/// use here (the request itself already passed the real session route gate).
fn current_family_id(cfg: &crate::auth::LiveAuthConfig, headers: &HeaderMap) -> Option<String> {
    crate::auth::session_family_id(cfg, headers)
}

fn session_json(
    meta: &lanrurugi_storage::refresh_tokens::RefreshFamilyMeta,
    current: bool,
) -> serde_json::Value {
    json!({
        "family_id": meta.family_id,
        "name": meta.device_name(),
        "auto_name": meta
            .device_info
            .as_ref()
            .map(lanrurugi_storage::device_info::DeviceInfo::display_name)
            .unwrap_or_else(|| "Unknown device".to_string()),
        "custom_name": meta.custom_name,
        "ip": meta.last_ip,
        "created_at": meta.created_at,
        "last_seen_at": meta.last_seen_at,
        "idle_expires_at": meta.idle_expires_at,
        "expires_at": meta.expires_at,
        "current": current,
    })
}

async fn list_sessions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let cfg = match crate::auth::load(&state).await {
        Ok(cfg) => cfg,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "list_sessions",
                e.to_string(),
            )
        }
    };
    let current = current_family_id(&cfg, &headers);
    match state.refresh_tokens.list_active_families(now_secs()).await {
        Ok(families) => {
            let body: Vec<_> = families
                .iter()
                .map(|meta| {
                    let is_current = current.as_deref() == Some(meta.family_id.as_str());
                    session_json(meta, is_current)
                })
                .collect();
            axum::Json(body).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "list_sessions",
            e.to_string(),
        ),
    }
}

#[derive(Deserialize)]
struct RenameSessionBody {
    name: String,
}

async fn rename_session(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    headers: HeaderMap,
    Path(family_id): Path<String>,
    axum::Json(body): axum::Json<RenameSessionBody>,
) -> Response {
    let name = body.name.trim();
    if name.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "rename_session",
            "name cannot be empty",
        );
    }
    if name.chars().count() > 80 {
        return error(
            StatusCode::BAD_REQUEST,
            "rename_session",
            "name must be 80 characters or fewer",
        );
    }
    let current_family = match crate::auth::load(&state).await {
        Ok(cfg) => current_family_id(&cfg, &headers),
        Err(_) => None,
    };
    match state
        .refresh_tokens
        .rename_family(&family_id, name, now_secs())
        .await
    {
        Ok(None) => not_found(
            "rename_session",
            format!("session {family_id} does not exist."),
        ),
        Ok(Some(meta)) => {
            let is_current = current_family.as_deref() == Some(meta.family_id.as_str());
            record_manual(
                &state,
                auth.as_ref().map(|extension| &extension.0),
                action_types::SESSION_DEVICE_RENAMED,
                ActivityTarget {
                    id: Some(family_id.clone()),
                    label: Some(meta.device_name()),
                    kind: Some("session".to_string()),
                },
                Outcome::Success,
                None,
                Some(json!({ "name": meta.device_name() })),
            )
            .await;
            ok(
                "rename_session",
                [("data", session_json(&meta, is_current))],
            )
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "rename_session",
            e.to_string(),
        ),
    }
}

async fn revoke_session(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    headers: HeaderMap,
    Path(family_id): Path<String>,
) -> Response {
    let cfg = match crate::auth::load(&state).await {
        Ok(cfg) => cfg,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "revoke_session",
                e.to_string(),
            )
        }
    };
    let is_current = current_family_id(&cfg, &headers).as_deref() == Some(family_id.as_str());
    let meta = match state.refresh_tokens.get_family_meta(&family_id).await {
        Ok(meta) => meta,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "revoke_session",
                e.to_string(),
            )
        }
    };
    match state.refresh_tokens.revoke_family(&family_id).await {
        Ok(false) => not_found(
            "revoke_session",
            format!("session {family_id} does not exist."),
        ),
        Ok(true) => {
            record_manual(
                &state,
                auth.as_ref().map(|extension| &extension.0),
                action_types::SESSION_DEVICE_REVOKED,
                ActivityTarget {
                    id: Some(family_id.clone()),
                    label: meta.as_ref().map(|meta| meta.device_name()),
                    kind: Some("session".to_string()),
                },
                Outcome::Success,
                None,
                None,
            )
            .await;
            if is_current {
                // Current device: clear both cookies so the browser immediately falls back to
                // guest/login, same effect as `POST /logout` but for the "revoke this row" path.
                let cookies = crate::login::cleared_auth_cookies(cfg.force_secure_cookies);
                (
                    StatusCode::OK,
                    crate::login::cookie_headers(cookies),
                    axum::Json(json!({ "operation": "revoke_session", "success": 1 })),
                )
                    .into_response()
            } else {
                ok("revoke_session", [])
            }
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "revoke_session",
            e.to_string(),
        ),
    }
}
