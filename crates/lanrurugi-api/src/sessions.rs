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
    absolute_lifetime_secs: i64,
) -> serde_json::Value {
    // `null` rather than a sentinel timestamp: with no absolute cap configured there is no date to
    // show, and `i64::MAX` would render as a year far in the future.
    let expires_at = meta.expires_at_under(absolute_lifetime_secs);
    let expires_at = if expires_at == lanrurugi_storage::refresh_tokens::NO_ABSOLUTE_EXPIRY {
        serde_json::Value::Null
    } else {
        json!(expires_at)
    };
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
        "expires_at": expires_at,
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
    let absolute_lifetime_secs = cfg.refresh_token_lifetime_secs as i64;
    match state
        .refresh_tokens
        .list_active_families(now_secs(), absolute_lifetime_secs)
        .await
    {
        Ok(families) => {
            let body: Vec<_> = families
                .iter()
                .map(|meta| {
                    let is_current = current.as_deref() == Some(meta.family_id.as_str());
                    session_json(meta, is_current, absolute_lifetime_secs)
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
    let (current_family, absolute_lifetime_secs) = match crate::auth::load(&state).await {
        Ok(cfg) => (
            current_family_id(&cfg, &headers),
            cfg.refresh_token_lifetime_secs as i64,
        ),
        Err(_) => (None, 0),
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
                [(
                    "data",
                    session_json(&meta, is_current, absolute_lifetime_secs),
                )],
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
                let cookies = crate::login::cleared_auth_cookies(
                    Some(&cfg.cookie_names),
                    cfg.force_secure_cookies,
                    cfg.retired_cookie_domain.as_deref(),
                );
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

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::refresh_tokens::{RefreshFamilyMeta, NO_ABSOLUTE_EXPIRY};

    fn meta() -> RefreshFamilyMeta {
        RefreshFamilyMeta {
            family_id: "family".to_string(),
            created_at: 1_000,
            last_seen_at: 1_500,
            idle_expires_at: 2_500,
            expires_at: NO_ABSOLUTE_EXPIRY,
            device_info: None,
            last_ip: None,
            current_token_id: "token".to_string(),
            custom_name: None,
            device_id: None,
        }
    }

    #[test]
    fn an_uncapped_session_reports_no_absolute_expiry() {
        // `null`, not `i64::MAX`: the Settings device list has no date to render, and a far-future
        // timestamp would be worse than saying "no limit".
        let json = session_json(&meta(), false, 0);
        assert!(json["expires_at"].is_null());
    }

    #[test]
    fn a_capped_session_reports_the_cap_counted_from_the_original_login() {
        let json = session_json(&meta(), false, 5_000);
        assert_eq!(json["expires_at"], json!(6_000));
        // The stored snapshot is irrelevant: the current setting wins.
        assert_ne!(meta().expires_at, 6_000);
    }
}
