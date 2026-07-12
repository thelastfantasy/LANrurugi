//! Login/logout for the bundled SPA's own session (distinct from the third-party API-key
//! contract — constitution Principle II). Mirrors legacy `Controller/Login.pm::check`/`logout`,
//! but as a JSON API rather than a server-rendered form post/redirect, since this is our own
//! frontend's mechanism rather than part of the OpenAPI contract.
//!
//! Deliberately **not** merged into [`crate::router`] — these two routes must stay reachable
//! without a valid API key or session (otherwise nobody could ever log in), so the server wires
//! them into a separate, unprotected router (see `lanrurugi-server/src/app.rs`).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use lanrurugi_core::{password, session};
use serde::Deserialize;

use crate::auth::load as load_auth_config;
use crate::common::error;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/login/status", get(status))
        .route("/logout", post(logout))
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

async fn login(State(state): State<AppState>, axum::Form(form): axum::Form<LoginForm>) -> Response {
    let auth = match load_auth_config(&state).await {
        Ok(a) => a,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "login", e.to_string()),
    };

    if !password::verify_password(&form.password, &auth.password_hash) {
        return error(StatusCode::UNAUTHORIZED, "login", "Wrong password.");
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs();
    let token = session::issue_token(&auth.session_secret, now);
    let cookie = format!(
        "{}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax",
        session::COOKIE_NAME,
        token,
        session::SESSION_LIFETIME_SECS,
    );

    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        axum::Json(serde_json::json!({ "operation": "login", "success": 1 })),
    )
        .into_response()
}

/// `GET /login/status` — reports whether the caller is "logged in" for the purposes of gating
/// admin-only UI, matching legacy's own `userlogged` template variable
/// (`Controller/Reader.pm`/`Index.pm`: `enable_pass == 0 || session('is_logged')`). Deliberately
/// its own endpoint rather than a new field on `/info` — `/info` mirrors legacy's third-party
/// `ServerInfo` OpenAPI schema field-for-field (constitution Principle II), and `logged_in` has no
/// place in that contract.
async fn status(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = match load_auth_config(&state).await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "login_status",
                e.to_string(),
            )
        }
    };
    let logged_in = !auth.enable_pass || crate::auth::session_is_valid(&auth, &headers);
    axum::Json(serde_json::json!({ "logged_in": logged_in })).into_response()
}

async fn logout() -> Response {
    let cookie = format!(
        "{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax",
        session::COOKIE_NAME
    );
    (
        StatusCode::OK,
        [(header::SET_COOKIE, cookie)],
        axum::Json(serde_json::json!({ "operation": "logout", "success": 1 })),
    )
        .into_response()
}
