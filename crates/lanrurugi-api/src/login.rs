//! Login/logout/refresh for the bundled SPA's own session (distinct from the third-party
//! API-token contract, `crate::api_tokens` — constitution Principle II's own annotation on this
//! project's deliberate departure from legacy's single-fixed-key mechanism). Mirrors legacy
//! `Controller/Login.pm::check`/`logout` for `login`/`logout`'s own shape, but as a JSON API
//! rather than a server-rendered form post/redirect (this is our own frontend's mechanism, not
//! part of the legacy OpenAPI contract) — `refresh` has no legacy equivalent at all.
//!
//! Deliberately **not** merged into [`crate::router`] — these routes must stay reachable without
//! a valid access token (otherwise nobody could ever log in, or silently refresh an expired one),
//! so the server wires them into a separate, unprotected router (see
//! `lanrurugi-server/src/app.rs`).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use lanrurugi_core::{password, session};
use lanrurugi_storage::refresh_tokens::RotateOutcome;
use serde::Deserialize;

use crate::auth::load as load_auth_config;
use crate::auth::LiveAuthConfig;
use crate::common::error;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login", post(login))
        .route("/login/status", get(status))
        .route("/logout", post(logout))
        .route("/token/refresh", post(refresh))
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs()
}

/// The refresh cookie's own `Path` — narrower than the access cookie's `/` so the browser only
/// ever attaches it to the one endpoint that actually reads it, `POST /api/token/refresh` (not to
/// every single request the way a `Path=/` cookie would be). `logout` deliberately does **not**
/// read this cookie (see that handler's own docs on why it reads `fid` out of the access token
/// instead) specifically so this can stay this narrow without also needing to cover `/api/logout`.
/// Must exactly match the `Path` `cleared_auth_cookies` clears with below — a `Set-Cookie` that
/// clears the same *name* but a different `Path` creates an unrelated second cookie instead of
/// removing this one (RFC 6265's own per-`(name, domain, path)` cookie identity), leaking a
/// zombie cookie that never actually gets cleared.
const REFRESH_COOKIE_PATH: &str = "/api/token/refresh";

/// Builds both `Set-Cookie` header values for a freshly-issued (or rotated) token pair. `Secure`
/// is appended only when `cfg.force_secure_cookies` is set (see that field's own docs on why it's
/// an explicit opt-in, not inferred from a request header).
fn auth_cookies(
    cfg: &LiveAuthConfig,
    access_token: &str,
    refresh_cookie_value: &str,
) -> [String; 2] {
    let secure = if cfg.force_secure_cookies {
        "; Secure"
    } else {
        ""
    };
    [
        format!(
            "{}={}; Path=/; Max-Age={}; HttpOnly; SameSite=Lax{}",
            session::COOKIE_NAME,
            access_token,
            cfg.access_token_lifetime_secs,
            secure,
        ),
        format!(
            "{}={}; Path={}; Max-Age={}; HttpOnly; SameSite=Lax{}",
            session::REFRESH_COOKIE_NAME,
            refresh_cookie_value,
            REFRESH_COOKIE_PATH,
            cfg.refresh_token_lifetime_secs,
            secure,
        ),
    ]
}

fn cleared_auth_cookies() -> [String; 2] {
    [
        format!(
            "{}=; Path=/; Max-Age=0; HttpOnly; SameSite=Lax",
            session::COOKIE_NAME
        ),
        format!(
            "{}=; Path={}; Max-Age=0; HttpOnly; SameSite=Lax",
            session::REFRESH_COOKIE_NAME,
            REFRESH_COOKIE_PATH,
        ),
    ]
}

/// `[(SET_COOKIE, ..); 2]`'s own `IntoResponseParts` impl (`axum-core`) builds the header map via
/// `HeaderMap::insert`, which *overwrites* same-name entries rather than adding a second one — two
/// array elements sharing the `Set-Cookie` key silently collapse to just the last one, so the
/// browser never actually receives both cookies (caught live by `tests/auth_flow.rs`: `login`
/// only ever set the refresh cookie). `HeaderMap::append` is the one that keeps both.
fn cookie_headers(cookies: [String; 2]) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    for cookie in cookies {
        headers.append(
            header::SET_COOKIE,
            cookie.parse().expect("valid cookie header value"),
        );
    }
    headers
}

async fn login(State(state): State<AppState>, axum::Form(form): axum::Form<LoginForm>) -> Response {
    let auth = match load_auth_config(&state).await {
        Ok(a) => a,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "login", e.to_string()),
    };

    if !password::verify_password(&form.password, &auth.password_hash) {
        return error(StatusCode::UNAUTHORIZED, "login", "Wrong password.");
    }

    let now = now_secs();
    let issued = match state
        .refresh_tokens
        .issue_new_family(now as i64, auth.refresh_token_lifetime_secs as i64)
        .await
    {
        Ok(issued) => issued,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "login", e.to_string()),
    };
    let access_token = session::issue_access_token(
        &auth.session_secret,
        now,
        auth.access_token_lifetime_secs,
        &issued.record.family_id,
    );
    let refresh_cookie_value = format!("{}.{}", issued.record.token_id, issued.secret);
    let cookies = auth_cookies(&auth, &access_token, &refresh_cookie_value);

    (
        StatusCode::OK,
        cookie_headers(cookies),
        axum::Json(serde_json::json!({ "operation": "login", "success": 1 })),
    )
        .into_response()
}

/// `POST /token/refresh` — no legacy equivalent. Redeems the refresh cookie for a fresh access
/// token + rotated refresh cookie (see `lanrurugi_storage::refresh_tokens::rotate`'s own docs for
/// the rotation/reuse-detection semantics). The frontend calls this transparently on a 401 from
/// any other endpoint (`apps/frontend/src/api/client.ts`), before ever redirecting to `/login`.
async fn refresh(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = match load_auth_config(&state).await {
        Ok(a) => a,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "token_refresh",
                e.to_string(),
            )
        }
    };

    let Some(cookie_header) = headers.get(header::COOKIE).and_then(|v| v.to_str().ok()) else {
        return error(
            StatusCode::UNAUTHORIZED,
            "token_refresh",
            "No refresh cookie.",
        );
    };
    let Some(cookie_value) = crate::auth::find_cookie(cookie_header, session::REFRESH_COOKIE_NAME)
    else {
        return error(
            StatusCode::UNAUTHORIZED,
            "token_refresh",
            "No refresh cookie.",
        );
    };
    let Some((token_id, secret)) = cookie_value.split_once('.') else {
        return error(
            StatusCode::UNAUTHORIZED,
            "token_refresh",
            "Malformed refresh cookie.",
        );
    };

    let now = now_secs();
    let outcome = match state
        .refresh_tokens
        .rotate(token_id, secret, now as i64)
        .await
    {
        Ok(outcome) => outcome,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "token_refresh",
                e.to_string(),
            )
        }
    };

    match outcome {
        RotateOutcome::NotFound => error(
            StatusCode::UNAUTHORIZED,
            "token_refresh",
            "Invalid or expired refresh token.",
        ),
        RotateOutcome::ReuseDetected => {
            // The whole family was just burned by `rotate` itself — clear both cookies so the
            // browser doesn't keep presenting now-dead credentials on its next request.
            let cookies = cleared_auth_cookies();
            (
                StatusCode::UNAUTHORIZED,
                cookie_headers(cookies),
                axum::Json(serde_json::json!({
                    "operation": "token_refresh",
                    "success": 0,
                    "error": "Refresh token reuse detected — all sessions for this login have been revoked.",
                })),
            )
                .into_response()
        }
        RotateOutcome::Rotated { record, secret } => {
            let access_token = session::issue_access_token(
                &auth.session_secret,
                now,
                auth.access_token_lifetime_secs,
                &record.family_id,
            );
            let refresh_cookie_value = format!("{}.{}", record.token_id, secret);
            let cookies = auth_cookies(&auth, &access_token, &refresh_cookie_value);
            (
                StatusCode::OK,
                cookie_headers(cookies),
                axum::Json(serde_json::json!({ "operation": "token_refresh", "success": 1 })),
            )
                .into_response()
        }
    }
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
    // Drives the homepage's "you're using the default password" warning toast (legacy's own
    // `[% IF usingdefpass %]`, `Controller/Index.pm`) — never itself a security boundary (nothing
    // here decides whether a login succeeds), just a UI nudge, so it's safe to expose to anyone.
    let using_default_password = auth.password_hash == crate::auth::DEFAULT_PASSWORD_HASH;
    axum::Json(serde_json::json!({
        "logged_in": logged_in,
        "using_default_password": using_default_password,
    }))
    .into_response()
}

/// Now that refresh tokens are stateful (`lanrurugi_storage::refresh_tokens`), logout actually
/// revokes — burns the entire refresh-token family the caller's access token was minted from
/// (same remediation `rotate`'s own reuse-detection path takes), not just clearing the browser's
/// cookies and hoping the now-orphaned access token quietly expires on its own within the next
/// few hours.
///
/// Reads `fid` out of the *access* token (`session::COOKIE_NAME`, `Path=/`), not the refresh
/// cookie — deliberately, so `REFRESH_COOKIE_PATH` can stay scoped to only
/// `POST /api/token/refresh` without also needing to cover this endpoint. Uses
/// `family_id_ignoring_expiry` rather than the normal expiry-checking verifier: a user who clicks
/// "log out" after their access token has already expired (idle tab, hasn't hit any endpoint that
/// would have triggered a silent refresh yet) must still get their refresh-token family actually
/// revoked — an already-expired access token still proves genuine origin via its signature, which
/// is all this needs.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let auth = load_auth_config(&state).await.ok();
    if let (Some(auth), Some(cookie_header)) = (
        auth,
        headers.get(header::COOKIE).and_then(|v| v.to_str().ok()),
    ) {
        if let Some(access_token) = crate::auth::find_cookie(cookie_header, session::COOKIE_NAME) {
            if let Some(family_id) =
                session::family_id_ignoring_expiry(&auth.session_secret, &access_token)
            {
                if let Err(e) = state.refresh_tokens.burn_family(&family_id).await {
                    tracing::warn!(error = %e, "logout: failed to burn refresh-token family");
                }
            }
        }
    }

    let cookies = cleared_auth_cookies();
    (
        StatusCode::OK,
        cookie_headers(cookies),
        axum::Json(serde_json::json!({ "operation": "logout", "success": 1 })),
    )
        .into_response()
}
