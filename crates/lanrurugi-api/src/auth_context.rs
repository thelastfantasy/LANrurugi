//! Per-request authentication identity — resolved once by
//! `lanrurugi_server::middleware::auth::require_api_key` and inserted into the request's
//! extensions, so downstream code (route-level `require_session` gates, `settings::put_settings`'s
//! own field-level check, request tracing) can see *how* this request authenticated without
//! re-deriving it.

use lanrurugi_storage::api_tokens::TokenRole;

#[derive(Debug, Clone)]
pub enum AuthMethod {
    /// The bundled SPA's own JWT session cookie — a real human who's already proven the admin
    /// password. Never subject to a token's own `Guest`-role or `require_session`-route
    /// restrictions.
    Session,
    /// A first-party API token (issue #54). `id` is the token's own record id (not the raw
    /// secret, which is never retained past issuance) — carried through for tracing/audit
    /// ("which token did this").
    Token { id: String, role: TokenRole },
    /// 007-guest-restricted-access: an unauthenticated caller granted scoped access because guest
    /// mode is on and at least one category is `visible_to_guest` — carries no persistent
    /// identity (no id, no role), unlike `Token`, since a guest visitor is re-evaluated fresh on
    /// every single request against current config rather than representing a stored credential.
    GuestVisitor,
}

#[derive(Debug, Clone)]
pub struct AuthContext {
    pub method: AuthMethod,
    /// Best-effort client IP (see `middleware::auth::client_ip`'s own docs on why this is
    /// display/diagnostic-only, never a security control) — carried here so request tracing can
    /// log it without re-deriving it from headers a second time.
    pub client_ip: Option<String>,
}

impl AuthContext {
    pub fn is_token(&self) -> bool {
        matches!(self.method, AuthMethod::Token { .. })
    }

    /// `Guest`-role tokens are read-only — enforced by HTTP method alone (not a per-endpoint
    /// allowlist), since "did this request mutate anything" is exactly what GET-vs-everything-else
    /// already means at the protocol level, and a per-endpoint list would silently miss a future
    /// route someone forgets to classify.
    pub fn is_guest_token(&self) -> bool {
        matches!(
            self.method,
            AuthMethod::Token {
                role: TokenRole::Guest,
                ..
            }
        )
    }

    /// A short label for tracing/logging (`"session"` or `"token:<id>"`) — never the raw token
    /// value itself, which this type never carries in the first place.
    pub fn trace_label(&self) -> String {
        match &self.method {
            AuthMethod::Session => "session".to_string(),
            AuthMethod::Token { id, .. } => format!("token:{id}"),
            AuthMethod::GuestVisitor => "guest_visitor".to_string(),
        }
    }
}
