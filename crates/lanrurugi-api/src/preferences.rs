//! Account-scoped display preferences.
//!
//! The Library homepage's sort field/direction used to live only in `localStorage`, so every
//! browser/device had its own answer even for the same account. These routes persist the few
//! display preferences that should follow the logged-in account instead, under a small Redis hash
//! in the config logical DB.
//!
//! The deployment currently has one real account (the admin session, or an Admin-role API token),
//! so every authenticated principal maps to the same `"admin"` owner. Keeping the owner resolver
//! separate means a future multi-user auth model can key by a real user id without moving the
//! storage shape again.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_storage::api_tokens::TokenRole;
use serde::Deserialize;
use serde_json::json;

use crate::auth_context::{AuthContext, AuthMethod};
use crate::common::{error, ok};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/preferences", get(get_preferences).put(put_preferences))
}

const DEFAULT_SORTBY: &str = "title";
const DEFAULT_SORTDIR: &str = "asc";

/// The only authenticated principals this app currently has share the single admin account.
fn preference_owner(auth: Option<&AuthContext>) -> Option<&'static str> {
    match auth.map(|context| &context.method) {
        Some(AuthMethod::Session)
        | Some(AuthMethod::Token {
            role: TokenRole::Admin,
            ..
        }) => Some("admin"),
        _ => None,
    }
}

fn preferences_key(owner: &str) -> String {
    format!("LANRURUGI_USER_PREFS_{owner}")
}

fn preferences_from_fields(fields: &HashMap<String, String>) -> serde_json::Value {
    json!({
        "library_sortby": fields
            .get("library_sortby")
            .cloned()
            .unwrap_or_else(|| DEFAULT_SORTBY.to_string()),
        "library_sortdir": fields
            .get("library_sortdir")
            .cloned()
            .unwrap_or_else(|| DEFAULT_SORTDIR.to_string()),
    })
}

async fn get_preferences(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
) -> Response {
    let Some(owner) = preference_owner(auth.as_deref()) else {
        return axum::Json(json!({
            "library_sortby": DEFAULT_SORTBY,
            "library_sortdir": DEFAULT_SORTDIR,
        }))
        .into_response();
    };

    let mut conn = match state.redis.config.get().await {
        Ok(conn) => conn,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_preferences",
                e.to_string(),
            )
        }
    };
    let fields: HashMap<String, String> = match conn.hgetall(preferences_key(owner)).await {
        Ok(fields) => fields,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_preferences",
                e.to_string(),
            )
        }
    };
    axum::Json(preferences_from_fields(&fields)).into_response()
}

#[derive(Debug, Deserialize)]
struct UpdatePreferencesBody {
    library_sortby: Option<String>,
    library_sortdir: Option<String>,
}

async fn put_preferences(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    axum::Json(body): axum::Json<UpdatePreferencesBody>,
) -> Response {
    let Some(owner) = preference_owner(auth.as_deref()) else {
        return error(
            StatusCode::FORBIDDEN,
            "put_preferences",
            "display preferences require an authenticated account".to_string(),
        );
    };

    let sortby = body.library_sortby.map(|value| value.trim().to_string());
    if let Some(sortby) = &sortby {
        if sortby.is_empty() || sortby.len() > 64 || sortby.contains(['\r', '\n']) {
            return error(
                StatusCode::BAD_REQUEST,
                "put_preferences",
                "library_sortby must be 1..64 characters without newlines".to_string(),
            );
        }
    }
    if let Some(sortdir) = &body.library_sortdir {
        if sortdir != "asc" && sortdir != "desc" {
            return error(
                StatusCode::BAD_REQUEST,
                "put_preferences",
                "library_sortdir must be \"asc\" or \"desc\"".to_string(),
            );
        }
    }

    let mut conn = match state.redis.config.get().await {
        Ok(conn) => conn,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_preferences",
                e.to_string(),
            )
        }
    };
    let key = preferences_key(owner);
    if let Some(sortby) = &sortby {
        if let Err(e) = conn
            .hset::<_, _, _, ()>(&key, "library_sortby", sortby)
            .await
        {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_preferences",
                e.to_string(),
            );
        }
    }
    if let Some(sortdir) = &body.library_sortdir {
        if let Err(e) = conn
            .hset::<_, _, _, ()>(&key, "library_sortdir", sortdir)
            .await
        {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_preferences",
                e.to_string(),
            );
        }
    }

    ok("put_preferences", []).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(method: AuthMethod) -> AuthContext {
        AuthContext {
            method,
            client_ip: None,
            user_agent: None,
            client_reported: None,
            session_family_id: None,
        }
    }

    #[test]
    fn maps_authenticated_admin_principals_to_the_admin_owner() {
        assert_eq!(
            preference_owner(Some(&context(AuthMethod::Session))),
            Some("admin")
        );
        assert_eq!(
            preference_owner(Some(&context(AuthMethod::Token {
                id: "token-1".to_string(),
                role: TokenRole::Admin,
            }))),
            Some("admin")
        );
        assert_eq!(
            preference_owner(Some(&context(AuthMethod::Token {
                id: "token-2".to_string(),
                role: TokenRole::Guest,
            }))),
            None
        );
        assert_eq!(preference_owner(None), None);
    }

    #[test]
    fn applies_defaults_and_reads_saved_sort_preferences() {
        let mut fields = HashMap::new();
        assert_eq!(
            preferences_from_fields(&fields),
            json!({ "library_sortby": "title", "library_sortdir": "asc" })
        );

        fields.insert("library_sortby".to_string(), "date_added".to_string());
        fields.insert("library_sortdir".to_string(), "desc".to_string());
        assert_eq!(
            preferences_from_fields(&fields),
            json!({ "library_sortby": "date_added", "library_sortdir": "desc" })
        );
    }
}
