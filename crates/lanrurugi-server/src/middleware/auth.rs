//! API-key authentication middleware, matching legacy auth semantics exactly (constitution
//! Principle II / `contracts/rest-api.md`).
//!
//! **Verified against source** (`~/LANraragi/lib/LANraragi/Utils/Login.pm::is_logged_in_api`):
//! a request is authorized if *any* of the following hold:
//! - `enable_pass` is off (an intentionally open, no-password instance) — always authorized;
//! - the `Authorization` header equals exactly `"Bearer " ++ base64(api_key)` — note the key is
//!   base64-**encoded** in the header, not sent raw, and the whole header value (not just a
//!   decoded token) is compared;
//! - the `key` query parameter equals the *raw* (non-base64) `api_key` — undocumented, legacy
//!   supports it mainly for OPDS clients that can't set custom headers;
//! - `$c->session('is_logged')` — legacy's own server-rendered UI's session cookie. Reproduced
//!   here too (unlike the API-key checks above, this one's a same-repo addition, not a
//!   third-party contract) via a signed, stateless token — see `lanrurugi_core::session` — since
//!   the bundled SPA needs some way to stay authenticated across requests once a password is set.
//!
//! `enable_pass`/`api_key` are read live from Redis on every request (`lanrurugi_api::auth::load`)
//! rather than cached at boot, so a value changed through the Settings page takes effect
//! immediately without a restart — matching legacy's own behavior of never caching config.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use lanrurugi_api::auth::LiveAuthConfig;
use lanrurugi_api::AppState;

pub async fn require_api_key(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let cfg = match lanrurugi_api::auth::load(&state).await {
        Ok(cfg) => cfg,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to load auth configuration: {e}"),
            )
                .into_response()
        }
    };

    if is_authorized(&cfg, request.headers(), request.uri().query()) {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
    }
}

fn is_authorized(
    cfg: &LiveAuthConfig,
    headers: &axum::http::HeaderMap,
    query: Option<&str>,
) -> bool {
    if !cfg.enable_pass {
        return true;
    }

    if !cfg.api_key.is_empty() {
        let expected_header = format!("Bearer {}", BASE64.encode(cfg.api_key.as_bytes()));
        if let Some(actual) = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        {
            if constant_time_eq(actual, &expected_header) {
                return true;
            }
        }

        if let Some(key_param) = query.and_then(|q| find_query_param(q, "key")) {
            if !key_param.is_empty() && constant_time_eq(&key_param, &cfg.api_key) {
                return true;
            }
        }
    }

    if lanrurugi_api::auth::session_is_valid(cfg, headers) {
        return true;
    }

    false
}

/// Avoids leaking how many leading bytes of a submitted key/header matched the expected value via
/// response-time differences (a timing side channel on API-key validation, CWE-208) — a plain
/// `==` short-circuits on the first mismatched byte, which `subtle`-style constant-time
/// comparisons like this one deliberately don't. The length check still short-circuits, but the
/// content (the actually secret part) doesn't.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Minimal query-string lookup (percent-decoding one param), so this middleware doesn't need a
/// full `axum::extract::Query` extraction (which requires a `Deserialize` target type) just to
/// read one optional field before the request is routed.
fn find_query_param(query: &str, name: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == name {
            return Some(percent_decode(v));
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.bytes().peekable();
    while let Some(b) = chars.next() {
        match b {
            b'+' => out.push(' '),
            b'%' => {
                let hi = chars.next();
                let lo = chars.next();
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    if let Ok(byte) =
                        u8::from_str_radix(&format!("{}{}", hi as char, lo as char), 16)
                    {
                        out.push(byte as char);
                        continue;
                    }
                }
                out.push('%');
            }
            b => out.push(b as char),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;
    use lanrurugi_core::session;

    fn cfg(api_key: &str, enable_pass: bool) -> LiveAuthConfig {
        LiveAuthConfig {
            api_key: api_key.to_string(),
            enable_pass,
            password_hash: String::new(),
            session_secret: Vec::new(),
        }
    }

    fn headers_with_auth(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::AUTHORIZATION, value.parse().unwrap());
        h
    }

    fn headers_with_cookie(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(axum::http::header::COOKIE, value.parse().unwrap());
        h
    }

    #[test]
    fn open_instance_always_authorized() {
        assert!(is_authorized(&cfg("", false), &HeaderMap::new(), None));
    }

    #[test]
    fn correct_bearer_base64_header_is_authorized() {
        let expected = format!("Bearer {}", BASE64.encode(b"s3cr3t"));
        assert!(is_authorized(
            &cfg("s3cr3t", true),
            &headers_with_auth(&expected),
            None
        ));
    }

    #[test]
    fn raw_key_in_header_is_rejected() {
        // A client sending the raw key (not base64-encoded) in the header must NOT authenticate —
        // this is exactly the legacy behavior, even though it's a common integration mistake.
        assert!(!is_authorized(
            &cfg("s3cr3t", true),
            &headers_with_auth("Bearer s3cr3t"),
            None
        ));
    }

    #[test]
    fn key_query_param_is_authorized() {
        assert!(is_authorized(
            &cfg("s3cr3t", true),
            &HeaderMap::new(),
            Some("key=s3cr3t")
        ));
        assert!(is_authorized(
            &cfg("s3cr3t", true),
            &HeaderMap::new(),
            Some("foo=bar&key=s3cr3t")
        ));
    }

    #[test]
    fn wrong_key_is_rejected() {
        assert!(!is_authorized(
            &cfg("s3cr3t", true),
            &HeaderMap::new(),
            Some("key=nope")
        ));
        assert!(!is_authorized(
            &cfg("s3cr3t", true),
            &HeaderMap::new(),
            None
        ));
    }

    #[test]
    fn empty_configured_key_never_authorizes_via_header_or_param_when_pass_enabled() {
        // Matches the legacy edge case: enable_pass=1 but no api_key ever set means API auth can
        // never succeed via header/param alone (a valid session cookie can still authorize).
        assert!(!is_authorized(
            &cfg("", true),
            &HeaderMap::new(),
            Some("key=")
        ));
    }

    #[test]
    fn valid_session_cookie_is_authorized() {
        let secret = b"test-secret";
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let token = session::issue_token(secret, now);
        let mut auth = cfg("s3cr3t", true);
        auth.session_secret = secret.to_vec();
        assert!(is_authorized(
            &auth,
            &headers_with_cookie(&format!("{}={token}", session::COOKIE_NAME)),
            None
        ));
    }

    #[test]
    fn expired_or_forged_session_cookie_is_rejected() {
        let auth = cfg("s3cr3t", true);
        assert!(!is_authorized(
            &auth,
            &headers_with_cookie(&format!("{}=garbage", session::COOKIE_NAME)),
            None
        ));
    }
}
