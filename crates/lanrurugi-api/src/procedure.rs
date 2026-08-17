//! The "procedure" pipeline every protected request goes through — one place that owns
//! authentication, authorization, and request-level tracing together, rather than scattering
//! those three concerns across separate files.
//!
//! [`require_api_key`] is the single gate, `.layer()`-ed on the whole protected router
//! (`lanrurugi-server`'s `app.rs`). Resolves *who* is making this request (a real session, or a
//! first-party API token with a role), rejects outright if that's nobody, records one structured
//! trace event (`operator`, `client_ip`), inserts [`crate::auth_context::AuthContext`] into the
//! request's extensions, and — via [`crate::authz::check_route`] against `policy/route_policy.csv`
//! — rejects any request the resolved role isn't allowed to make against the actual matched route
//! (`axum::extract::MatchedPath`) and method, covering both a `Guest`-role token's blanket
//! GET-only restriction and the handful of Session-only routes (`/tokens*`,
//! `/database/drop`, `/settings/password`) no API token, admin-role or not, may ever reach.
//!
//! Before issue #91, this was two separate pieces — `require_api_key` (identity only) plus a
//! second `require_session` middleware individually `.route_layer()`-ed onto each session-only
//! route's own sub-router, on the assumption that a route-aware check couldn't live in
//! `require_api_key` without knowing the matched route pattern, which axum only resolves *after*
//! the outer `.layer()` stack has already run. That assumption turned out to be wrong —
//! `MatchedPath` resolves correctly even read from an outer `.layer()` — so the two checks are now
//! one: `require_api_key` itself calls `check_route`, and `route_policy.csv` is the one place
//! that says which role may reach which route. An admin-role token hitting `/database/drop` used
//! to be *allowed through* `require_api_key`'s own identity check and only rejected afterward by
//! the separate `require_session` layer; now `require_api_key`'s single `check_route` call
//! rejects it directly, since `token_admin` was never in that route's own allow-list to begin
//! with.
//!
//! **Deliberately does not implement legacy's own API-key semantics anymore** — the
//! `Authorization: Bearer base64(apikey)` header format and the undocumented `?key=` query
//! parameter (both verified against `~/LANraragi/lib/LANraragi/Utils/Login.pm::is_logged_in_api`
//! in earlier versions of this module) are gone, replaced entirely by the multi-token system
//! above. This is a deliberate, pre-release break from constitution Principle II's "API-key
//! authentication semantics" clause — see that principle's own annotation in
//! `.specify/memory/constitution.md`.

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth_context::{AuthContext, AuthMethod};
use crate::AppState;

pub async fn require_api_key(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<std::net::SocketAddr>,
    matched_path: MatchedPath,
    mut request: Request,
    next: Next,
) -> Response {
    let cfg = match crate::auth::load(&state).await {
        Ok(cfg) => cfg,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load auth configuration: {e}"),
            )
                .into_response()
        }
    };

    // `no_fun_mode` deliberately overrides this bypass — see that field's own docs
    // (`LiveAuthConfig::no_fun_mode`) for why `enable_pass: false` must not mean "wide open" once
    // No-Fun Mode is on. No `AuthContext` is ever inserted here (there's no identity to attach —
    // this is the literal "anyone at all" case), but the Session-only routes must still be
    // unreachable even on an open instance (`route_policy.csv`'s own `anonymous` deny rules for
    // `/tokens*`/`/database/drop`/`/settings/password` exist specifically for this — an open
    // instance was never meant to let a random caller drop the whole database), so this still
    // routes through `check_route` with `auth: None` rather than skipping straight to `next`.
    if !cfg.enable_pass && !cfg.no_fun_mode {
        let obj = crate::authz::axum_path_to_casbin(matched_path.as_str());
        let authz = crate::authz::Authz::get().await;
        if !crate::authz::check_route(&authz.route, None, &obj, request.method().as_str()) {
            return route_forbidden_response();
        }
        return next.run(request).await;
    }

    let client_ip = client_ip(request.headers(), peer_addr);

    if let Some(bearer_token) = bearer_token(request.headers()) {
        if lanrurugi_storage::api_tokens::looks_like_api_token(bearer_token) {
            match state.api_tokens.verify(bearer_token).await {
                Ok(Some(record)) => {
                    touch_last_used_throttled(&state, &record.id, client_ip.clone()).await;
                    let auth = AuthContext {
                        method: AuthMethod::Token {
                            id: record.id,
                            role: record.role,
                        },
                        client_ip,
                    };
                    if !authorize_route(&matched_path, request.method().as_str(), &auth).await {
                        trace_request(&request, &auth, false);
                        return route_forbidden_response();
                    }
                    trace_request(&request, &auth, true);
                    request.extensions_mut().insert(auth);
                    return next.run(request).await;
                }
                Ok(None) => {} // not a valid token — fall through to the session-cookie check
                Err(e) => {
                    tracing::warn!(error = %e, "api token verification failed");
                    // A Redis hiccup here shouldn't silently deny every API-token client; fall
                    // through to the session check the same as "token not found" would.
                }
            }
        }
    }

    if crate::auth::session_is_valid(&cfg, request.headers()) {
        let auth = AuthContext {
            method: AuthMethod::Session,
            client_ip,
        };
        // Always allowed today (`route_policy.csv`'s own `p, session, /*, *, allow` has no
        // exceptions), but still routed through the same `authorize_route` check as the `Token`
        // branch above rather than skipped — `route_policy.csv` stays the one place that can say
        // "no" to *any* role, including `session`, without this function's own code needing to
        // change to enforce a future Session-side restriction.
        if !authorize_route(&matched_path, request.method().as_str(), &auth).await {
            trace_request(&request, &auth, false);
            return route_forbidden_response();
        }
        trace_request(&request, &auth, true);
        request.extensions_mut().insert(auth);
        return next.run(request).await;
    }

    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

/// Shared by both the `Token` and `Session` branches of [`require_api_key`] — see that function's
/// own docs on why both go through the identical [`crate::authz::check_route`] call against
/// `policy/route_policy.csv` rather than the `Token` branch alone. Takes `method` as an already-
/// extracted `&str` rather than `&Request` itself — holding a live `&Request` reference across
/// the `.await` inside here makes the enclosing future `!Sync` (axum's own `Request`/`Body` wraps
/// a `Box<dyn HttpBody>`, which isn't `Sync`), and `FromFn`'s own `Service` impl requires the
/// whole `require_api_key` future to be `Sync` — confirmed live as a real compile failure only
/// after adding this function, not present in either of the two branches' predecessor code.
async fn authorize_route(matched_path: &MatchedPath, method: &str, auth: &AuthContext) -> bool {
    let obj = crate::authz::axum_path_to_casbin(matched_path.as_str());
    let authz = crate::authz::Authz::get().await;
    crate::authz::check_route(&authz.route, Some(auth), &obj, method)
}

/// Covers every reason [`authorize_route`] can say no — a `Guest`-role token's non-`GET` request,
/// or any role hitting a route `route_policy.csv` denies it (the Session-only routes, formerly
/// `require_session`'s own separate check — see this module's own top-level docs). Deliberately
/// generic rather than trying to guess which specific rule fired, since `route_policy.csv` is the
/// single source of truth for that and a wrong guess here would be actively misleading.
fn route_forbidden_response() -> Response {
    (
        StatusCode::FORBIDDEN,
        "Your current credentials are not authorized to make this request.",
    )
        .into_response()
}

/// One structured event per authenticated (or rejected) request — `operator` is `"session"` or
/// `"token:<id>"` (never a raw token value, which `AuthContext` never carries in the first place),
/// so an admin can answer "who did this" from the logs alone. Lands in the same `general.log`
/// category every other `lanrurugi_api`/`lanrurugi_server` event not otherwise categorized does
/// (`lanrurugi_server::telemetry::categorize`) — not `http.log` (that category is reserved for
/// `tower_http`/`axum`'s own framework-level request-lifecycle events, a different concern from
/// this app-level identity audit trail).
fn trace_request(request: &Request, auth: &AuthContext, allowed: bool) {
    tracing::info!(
        method = %request.method(),
        path = %request.uri().path(),
        operator = %auth.trace_label(),
        client_ip = auth.client_ip.as_deref().unwrap_or("unknown"),
        allowed,
        "authenticated api request"
    );
}

/// `Authorization: Bearer <value>` — unlike legacy's own header (`Bearer base64(apikey)`, an
/// exact match against one precomputed string), an API token's bearer value is the raw token
/// itself, verified by hash lookup rather than string comparison.
fn bearer_token(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// Best-effort client IP for `ApiTokenRecord::last_used_ip`/request tracing (display/diagnostic
/// only — see that field's own docs on why this is never a security decision). Prefers
/// `X-Forwarded-For`'s first hop (the real client, under a reverse-proxy deployment — the
/// realistic shape for this app per its own Docker-oriented docs) over the raw TCP peer address,
/// since behind a proxy the peer address is just the proxy itself. No trusted-proxy allowlist
/// exists to validate the header against, so this is inherently spoofable by a request that
/// reaches the proxy able to set its own `X-Forwarded-For` — acceptable for a display/audit field,
/// unacceptable for anything gating access, which is why nothing here does that.
pub(crate) fn client_ip(
    headers: &axum::http::HeaderMap,
    peer_addr: std::net::SocketAddr,
) -> Option<String> {
    if let Some(forwarded) = headers.get("X-Forwarded-For").and_then(|v| v.to_str().ok()) {
        if let Some(first) = forwarded.split(',').next() {
            let trimmed = first.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    Some(peer_addr.ip().to_string())
}

/// Write-behind throttle for `ApiTokenRepository::touch_last_used` — see
/// `AppState::api_token_last_touch`'s own docs for why this exists (avoids a Redis write on
/// every single API-token-authenticated request). 60 seconds: frequent enough that the Settings
/// page's "last used" display stays reasonably fresh, infrequent enough that a client hammering
/// the API doesn't turn this into a write-per-request.
const LAST_USED_TOUCH_INTERVAL_SECS: i64 = 60;

async fn touch_last_used_throttled(state: &AppState, token_id: &str, ip: Option<String>) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    {
        let mut last_touch = state.api_token_last_touch.lock().await;
        let should_touch = match last_touch.get(token_id) {
            Some(&last) => now - last >= LAST_USED_TOUCH_INTERVAL_SECS,
            None => true,
        };
        if !should_touch {
            return;
        }
        last_touch.insert(token_id.to_string(), now);
    }
    if let Err(e) = state.api_tokens.touch_last_used(token_id, now, ip).await {
        tracing::warn!(error = %e, "failed to record api token last-used time");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::LiveAuthConfig;

    fn cfg(enable_pass: bool) -> LiveAuthConfig {
        LiveAuthConfig {
            enable_pass,
            no_fun_mode: false,
            password_hash: String::new(),
            session_secret: Vec::new(),
            access_token_lifetime_secs: 3_600,
            refresh_token_lifetime_secs: 604_800,
            force_secure_cookies: false,
        }
    }

    fn headers_with_cookie(value: &str) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        h.insert(axum::http::header::COOKIE, value.parse().unwrap());
        h
    }

    #[test]
    fn open_instance_bypasses_session_check() {
        // `enable_pass: false` is checked directly in `require_api_key` before either credential
        // path runs — no cookie/token needed to be "authorized" on an open instance. Exercised
        // here at the `LiveAuthConfig` level since `require_api_key` itself needs a live
        // `AppState`/Redis to run end-to-end (see `lanrurugi-server`'s `tests/auth_flow.rs`).
        assert!(!cfg(false).enable_pass);
    }

    #[test]
    fn no_fun_mode_overrides_the_open_instance_bypass() {
        // The actual condition `require_api_key` checks (`!enable_pass && !no_fun_mode`) — with
        // `no_fun_mode: true`, an open instance (`enable_pass: false`) must NOT bypass auth,
        // matching legacy's own `enable_nofun` forcing `logged_in_api` regardless of `enable_pass`.
        let mut auth = cfg(false);
        auth.no_fun_mode = true;
        assert!(!(!auth.enable_pass && !auth.no_fun_mode));
    }

    #[test]
    fn valid_session_cookie_is_recognized() {
        use lanrurugi_core::session;
        let secret = b"test-secret";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let token = session::issue_access_token(secret, now, 3_600, "family-1");
        let mut auth = cfg(true);
        auth.session_secret = secret.to_vec();
        assert!(crate::auth::session_is_valid(
            &auth,
            &headers_with_cookie(&format!("{}={token}", session::COOKIE_NAME)),
        ));
    }

    #[test]
    fn expired_or_forged_session_cookie_is_rejected() {
        let auth = cfg(true);
        assert!(!crate::auth::session_is_valid(
            &auth,
            &headers_with_cookie(&format!("{}=garbage", lanrurugi_core::session::COOKIE_NAME)),
        ));
    }

    #[test]
    fn bearer_token_extraction() {
        let mut h = axum::http::HeaderMap::new();
        h.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer lru_abc123".parse().unwrap(),
        );
        assert_eq!(bearer_token(&h), Some("lru_abc123"));

        let empty = axum::http::HeaderMap::new();
        assert_eq!(bearer_token(&empty), None);

        let mut wrong_scheme = axum::http::HeaderMap::new();
        wrong_scheme.insert(
            axum::http::header::AUTHORIZATION,
            "Basic dXNlcjpwYXNz".parse().unwrap(),
        );
        assert_eq!(bearer_token(&wrong_scheme), None);
    }

    #[test]
    fn client_ip_prefers_x_forwarded_for_first_hop() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("X-Forwarded-For", "203.0.113.5, 10.0.0.1".parse().unwrap());
        let peer: std::net::SocketAddr = "10.0.0.1:1234".parse().unwrap();
        assert_eq!(client_ip(&h, peer).as_deref(), Some("203.0.113.5"));
    }

    #[test]
    fn client_ip_falls_back_to_peer_addr_when_no_forwarded_header() {
        let h = axum::http::HeaderMap::new();
        let addr: std::net::SocketAddr = "127.0.0.1:12345".parse().unwrap();
        assert_eq!(client_ip(&h, addr).as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn guest_token_context_is_recognized() {
        let guest = AuthContext {
            method: AuthMethod::Token {
                id: "abc".to_string(),
                role: lanrurugi_storage::api_tokens::TokenRole::Guest,
            },
            client_ip: None,
        };
        assert!(guest.is_token());
        assert!(guest.is_guest_token());
        assert_eq!(guest.trace_label(), "token:abc");

        let admin = AuthContext {
            method: AuthMethod::Token {
                id: "xyz".to_string(),
                role: lanrurugi_storage::api_tokens::TokenRole::Admin,
            },
            client_ip: None,
        };
        assert!(admin.is_token());
        assert!(!admin.is_guest_token());

        let session = AuthContext {
            method: AuthMethod::Session,
            client_ip: None,
        };
        assert!(!session.is_token());
        assert!(!session.is_guest_token());
        assert_eq!(session.trace_label(), "session");
    }
}
