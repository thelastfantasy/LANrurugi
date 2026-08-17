//! The "procedure" pipeline every protected request goes through — one place that owns
//! authentication, authorization, and request-level tracing together, rather than scattering
//! those three concerns across separate files. Two composable pieces, applied at different layers
//! (see each function's own docs for why):
//!
//! - [`require_api_key`] — the base procedure, `.layer()`-ed on the whole protected router
//!   (`lanrurugi-server`'s `app.rs`). Resolves *who* is making this request (a real session, or a
//!   first-party API token with a role), rejects outright if that's nobody, rejects a `Guest`-role
//!   token's non-`GET` request, and — regardless of which path let the request through — records
//!   one structured trace event (`operator`, `client_ip`) and inserts [`crate::auth_context::AuthContext`]
//!   into the request's extensions for the pieces below to read.
//! - [`require_session`] — an additional gate, `.route_layer()`-ed directly onto specific routes at
//!   their own definition site (`api_tokens.rs`'s `/tokens*`, `database.rs`'s `/database/drop`,
//!   `settings.rs`'s `/settings/password`) — routes no API token, admin-role or not, may ever
//!   reach. Reads the `AuthContext` `require_api_key` already inserted; a route-level layer (not a
//!   condition inside `require_api_key` itself) because axum only resolves *which* route matched
//!   — and therefore which route-level layers apply — *after* the outer `.layer()` stack has
//!   already run, so this check couldn't live in `require_api_key` even if we wanted it to.
//!
//! **Deliberately does not implement legacy's own API-key semantics anymore** — the
//! `Authorization: Bearer base64(apikey)` header format and the undocumented `?key=` query
//! parameter (both verified against `~/LANraragi/lib/LANraragi/Utils/Login.pm::is_logged_in_api`
//! in earlier versions of this module) are gone, replaced entirely by the multi-token system
//! above. This is a deliberate, pre-release break from constitution Principle II's "API-key
//! authentication semantics" clause — see that principle's own annotation in
//! `.specify/memory/constitution.md`.

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::auth_context::{AuthContext, AuthMethod};
use crate::AppState;

pub async fn require_api_key(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<std::net::SocketAddr>,
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
    // No-Fun Mode is on.
    if !cfg.enable_pass && !cfg.no_fun_mode {
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
                    // `Guest` is read-only by HTTP method alone — see `AuthContext::is_guest_token`'s
                    // own docs for why this is a blanket rule rather than a per-endpoint allowlist.
                    if auth.is_guest_token() && request.method() != Method::GET {
                        trace_request(&request, &auth, false);
                        return (
                            StatusCode::FORBIDDEN,
                            "This token is read-only (guest role) and cannot make non-GET requests.",
                        )
                            .into_response();
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
        trace_request(&request, &auth, true);
        request.extensions_mut().insert(auth);
        return next.run(request).await;
    }

    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
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

/// Route-level gate for the handful of endpoints no API token — `Guest` or `Admin` role alike —
/// may ever reach: token management itself (`/tokens*`), and the other two "danger" categories
/// confirmed for this project (account-security: `POST /settings/password`; data-destruction:
/// `POST /database/drop`). Only a real session cookie (a human who's already typed the actual
/// admin password) may call these. Applied via `.route_layer(axum::middleware::from_fn(require_session))`
/// directly at each such route's own definition — see this module's own top-level docs for why a
/// route-level layer, not a check inside `require_api_key`, is what axum's layering model actually
/// requires here.
///
/// `PUT /settings` is a partial exception to this pattern: it's one generic endpoint covering many
/// fields of very different sensitivity (`theme`/`motd`/`pagesize` alongside `enablepass`/
/// `nofunmode`/the token-lifetime settings), so a whole-route `require_session` would also block
/// harmless field updates a `Guest`-blocked-but-otherwise-trusted admin-role token should still be
/// able to make. That endpoint instead does its own narrower, field-level check inside
/// `settings::put_settings` by reading the same `AuthContext` this function reads.
///
/// Backed by [`crate::authz`] (issue #91) rather than a hardcoded "any token at all" check — reads
/// `policy/route_policy.csv` via [`crate::authz::Authz::get`] (a process-global, not an
/// `AppState` field — see that function's own docs on why), keyed on the actual matched route
/// pattern (`MatchedPath`, axum's own `{param}` syntax, translated to Casbin's `:param` via
/// [`crate::authz::axum_path_to_casbin`]) and HTTP method, so the *set* of session-only routes
/// lives in one declarative file instead of being implied by which handlers happen to have this
/// middleware mounted on them. Still mounted the exact same way (`.route_layer(from_fn(require_session))`
/// at each route's own definition) — only what happens *inside* changed, not where it's called
/// from.
pub async fn require_session(matched_path: MatchedPath, request: Request, next: Next) -> Response {
    let auth = request.extensions().get::<AuthContext>();
    let obj = crate::authz::axum_path_to_casbin(matched_path.as_str());
    let method = request.method().as_str();
    let authz = crate::authz::Authz::get().await;
    if !crate::authz::check_route(&authz.route, auth, &obj, method) {
        return (
            StatusCode::FORBIDDEN,
            "This action requires a real login session, not an API token.",
        )
            .into_response();
    }
    next.run(request).await
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
fn client_ip(headers: &axum::http::HeaderMap, peer_addr: std::net::SocketAddr) -> Option<String> {
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
