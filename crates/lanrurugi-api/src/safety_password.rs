//! Safety password (`specs/019-recycle-bin-safety-password`, FR-018–FR-026).
//!
//! A four-digit secondary credential that gates irreversible recycle-bin actions. Stored only as a
//! bcrypt hash in `LRR_CONFIG` (never echoed), verified per request, with a per-login-family
//! lockout after five failures. The reset path verifies the *login* password instead.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;

use crate::activity::record_manual;
use crate::auth_context::AuthContext;
use crate::common::error;
use crate::AppState;
use lanrurugi_core::password;
use lanrurugi_storage::activity::{action_types, ActivityTarget, Outcome};
use lanrurugi_storage::keys::{
    CONFIG_KEY, SAFETY_FAIL_KEY_PREFIX, SAFETY_LOCK_KEY_PREFIX, SAFETY_PASSWORD_HASH_FIELD,
    SAFETY_PASSWORD_SET_AT_FIELD,
};

const MAX_ATTEMPTS: i64 = 5;
const LOCK_SECS: i64 = 300;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/safety-password", get(get_status).post(set_or_change))
        .route("/safety-password/reset", post(reset))
}

#[derive(Debug, Deserialize)]
struct SetBody {
    #[serde(default)]
    current_password: Option<String>,
    new_password: String,
}

#[derive(Debug, Deserialize)]
struct ResetBody {
    login_password: String,
}

/// Why a verification failed. Every variant maps to a response that reveals nothing about which
/// archive id was asked for.
#[derive(Debug)]
pub(crate) enum VerifyError {
    NotConfigured,
    Locked { locked_until: i64 },
    Incorrect,
    Internal(String),
}

impl VerifyError {
    pub(crate) fn into_response(self, operation: &str) -> Response {
        match self {
            VerifyError::NotConfigured => (
                StatusCode::FORBIDDEN,
                axum::Json(json!({
                    "operation": operation,
                    "error": "safety_password_not_set",
                    "success": 0,
                })),
            )
                .into_response(),
            VerifyError::Locked { locked_until } => (
                StatusCode::TOO_MANY_REQUESTS,
                axum::Json(json!({
                    "operation": operation,
                    "error": "safety_password_locked",
                    "success": 0,
                    "locked_until": locked_until,
                })),
            )
                .into_response(),
            VerifyError::Incorrect => (
                StatusCode::FORBIDDEN,
                axum::Json(json!({
                    "operation": operation,
                    "error": "safety_password_incorrect",
                    "success": 0,
                })),
            )
                .into_response(),
            VerifyError::Internal(e) => error(StatusCode::INTERNAL_SERVER_ERROR, operation, e),
        }
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// One lockout bucket per login family; a session cookie without a family claim shares the
/// historical `"session"` bucket.
fn family_key(auth: Option<&AuthContext>) -> String {
    auth.and_then(|a| a.session_family_id.clone())
        .unwrap_or_else(|| "session".to_string())
}

fn valid_pin(pin: &str) -> bool {
    pin.len() == 4 && pin.bytes().all(|b| b.is_ascii_digit())
}

async fn stored_hash(state: &AppState) -> Result<Option<String>, String> {
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    conn.hget::<_, _, Option<String>>(CONFIG_KEY, SAFETY_PASSWORD_HASH_FIELD)
        .await
        .map_err(|e| e.to_string())
}

async fn failure_count(state: &AppState, family: &str) -> i64 {
    let Ok(mut conn) = state.redis.config.get().await else {
        return 0;
    };
    conn.get::<_, Option<String>>(format!("{SAFETY_FAIL_KEY_PREFIX}{family}"))
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

async fn locked_until(state: &AppState, family: &str) -> Option<i64> {
    let mut conn = state.redis.config.get().await.ok()?;
    let value: Option<String> = conn
        .get(format!("{SAFETY_LOCK_KEY_PREFIX}{family}"))
        .await
        .ok()
        .flatten()?;
    let until: i64 = value?.parse().ok()?;
    (until > now_secs()).then_some(until)
}

/// Verifies `pin` for this request, applying lockout. Callers must invoke this *before* resolving
/// which entry an action targets.
pub(crate) async fn verify(
    state: &AppState,
    auth: Option<&AuthContext>,
    pin: &str,
) -> Result<(), VerifyError> {
    let family = family_key(auth);
    if let Some(until) = locked_until(state, &family).await {
        return Err(VerifyError::Locked {
            locked_until: until,
        });
    }
    let hash = match stored_hash(state).await {
        Ok(Some(hash)) => hash,
        Ok(None) => return Err(VerifyError::NotConfigured),
        Err(e) => return Err(VerifyError::Internal(e)),
    };
    let candidate = pin.to_string();
    let matched = tokio::task::spawn_blocking(move || password::verify_password(&candidate, &hash))
        .await
        .unwrap_or(false);
    if matched {
        if let Ok(mut conn) = state.redis.config.get().await {
            let _: Result<(), _> = conn.del(format!("{SAFETY_FAIL_KEY_PREFIX}{family}")).await;
        }
        return Ok(());
    }

    // A malformed pin counts exactly like a wrong one — reporting a format error here would let a
    // caller probe without spending an attempt.
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => return Err(VerifyError::Internal(e.to_string())),
    };
    let fail_key = format!("{SAFETY_FAIL_KEY_PREFIX}{family}");
    let count: i64 = conn.incr(&fail_key, 1i64).await.unwrap_or(1);
    let _: Result<(), _> = conn.expire(&fail_key, LOCK_SECS).await;
    if count >= MAX_ATTEMPTS {
        let until = now_secs() + LOCK_SECS;
        let _: Result<(), _> = conn
            .set_ex(
                format!("{SAFETY_LOCK_KEY_PREFIX}{family}"),
                until,
                LOCK_SECS as u64,
            )
            .await;
        let _: Result<(), _> = conn.del(&fail_key).await;
        return Err(VerifyError::Locked {
            locked_until: until,
        });
    }
    Err(VerifyError::Incorrect)
}

async fn status_json(state: &AppState, auth: Option<&AuthContext>) -> serde_json::Value {
    let family = family_key(auth);
    let configured = matches!(stored_hash(state).await, Ok(Some(_)));
    let until = locked_until(state, &family).await;
    let remaining = if configured {
        Some((MAX_ATTEMPTS - failure_count(state, &family).await).max(0))
    } else {
        None
    };
    json!({
        "configured": configured,
        "locked": until.is_some(),
        "locked_until": until,
        "attempts_remaining": remaining,
    })
}

async fn get_status(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    axum::Json(status_json(&state, auth.as_ref().map(|e| &e.0)).await).into_response()
}

async fn set_or_change(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<SetBody>,
) -> Response {
    let auth = auth.as_ref().map(|e| &e.0);
    if !valid_pin(&body.new_password) {
        return (
            StatusCode::BAD_REQUEST,
            axum::Json(json!({
                "operation": "set_safety_password",
                "error": "invalid_safety_password_format",
                "success": 0,
            })),
        )
            .into_response();
    }
    let configured = matches!(stored_hash(&state).await, Ok(Some(_)));
    if configured {
        let Some(current) = body.current_password.as_deref() else {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(json!({
                    "operation": "set_safety_password",
                    "error": "current_safety_password_required",
                    "success": 0,
                })),
            )
                .into_response();
        };
        if let Err(e) = verify(&state, auth, current).await {
            return e.into_response("set_safety_password");
        }
    }

    let plaintext = body.new_password.clone();
    let hashed =
        match tokio::task::spawn_blocking(move || password::hash_password(&plaintext)).await {
            Ok(Ok(hash)) => hash,
            _ => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "set_safety_password",
                    "failed to hash the safety password",
                )
            }
        };
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "set_safety_password",
                e.to_string(),
            )
        }
    };
    let set_at = now_secs();
    let write: Result<(), deadpool_redis::redis::RedisError> = async {
        conn.hset::<_, _, _, ()>(CONFIG_KEY, SAFETY_PASSWORD_HASH_FIELD, &hashed)
            .await?;
        conn.hset::<_, _, _, ()>(CONFIG_KEY, SAFETY_PASSWORD_SET_AT_FIELD, set_at.to_string())
            .await?;
        conn.del::<_, ()>(format!("{SAFETY_FAIL_KEY_PREFIX}{}", family_key(auth)))
            .await?;
        Ok(())
    }
    .await;
    if let Err(e) = write {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "set_safety_password",
            format!("{e:?}"),
        );
    }

    record_manual(
        &state,
        auth,
        action_types::SAFETY_PASSWORD_SET,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("settings".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "changed": if configured { "change" } else { "set" } })),
    )
    .await;

    axum::Json(status_json(&state, auth).await).into_response()
}

async fn reset(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<ResetBody>,
) -> Response {
    let auth = auth.as_ref().map(|e| &e.0);
    let stored = match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget::<_, _, Option<String>>(CONFIG_KEY, "password")
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| crate::auth::DEFAULT_PASSWORD_HASH.to_string()),
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "reset_safety_password",
                e.to_string(),
            )
        }
    };
    let supplied = body.login_password.clone();
    let matched =
        tokio::task::spawn_blocking(move || password::verify_password(&supplied, &stored))
            .await
            .unwrap_or(false);
    if !matched {
        return (
            StatusCode::FORBIDDEN,
            axum::Json(json!({
                "operation": "reset_safety_password",
                "error": "wrong_password",
                "success": 0,
            })),
        )
            .into_response();
    }

    let family = family_key(auth);
    if let Ok(mut conn) = state.redis.config.get().await {
        let _: Result<(), _> = conn
            .hdel(
                CONFIG_KEY,
                &[SAFETY_PASSWORD_HASH_FIELD, SAFETY_PASSWORD_SET_AT_FIELD][..],
            )
            .await;
        let _: Result<(), _> = conn.del(format!("{SAFETY_FAIL_KEY_PREFIX}{family}")).await;
        let _: Result<(), _> = conn.del(format!("{SAFETY_LOCK_KEY_PREFIX}{family}")).await;
    }

    record_manual(
        &state,
        auth,
        action_types::SAFETY_PASSWORD_RESET,
        ActivityTarget {
            id: None,
            label: None,
            kind: Some("settings".to_string()),
        },
        Outcome::Success,
        None,
        Some(json!({ "changed": "reset" })),
    )
    .await;

    axum::Json(status_json(&state, auth).await).into_response()
}
