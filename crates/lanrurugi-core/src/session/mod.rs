//! Browser-login session tokens for the bundled SPA (distinct from the API-token contract —
//! `lanrurugi_storage::api_tokens` — which is for third-party/programmatic clients). Two-token
//! design: a short-lived, stateless JWT **access token** (this module's [`jwt`] submodule) plus a
//! long-lived, Redis-backed, rotating **refresh token** (`lanrurugi_storage::refresh_tokens`,
//! which this crate has no dependency on — refresh-token *storage* belongs at the storage layer,
//! this crate only knows how to mint/verify the stateless half).
//!
//! Legacy has no equivalent of this — legacy signs its session cookie with Mojolicious's own
//! app-level secret. Both cookies are signed with the same `session_secret`
//! (`lanrurugi_api::auth::LiveAuthConfig`), persisted once in `LRR_CONFIG` on first use so
//! already-issued tokens keep verifying across process restarts.

pub mod jwt;

pub use jwt::{family_id_ignoring_expiry, issue_access_token, verify_access_token, AccessClaims};

/// Cookie carrying the JWT access token.
pub const COOKIE_NAME: &str = "lanrurugi_session";
/// Cookie carrying the opaque `"{token_id}.{secret}"` refresh token
/// (`lanrurugi_storage::refresh_tokens`).
pub const REFRESH_COOKIE_NAME: &str = "lanrurugi_refresh";

/// Default access-token lifetime — short, since it's stateless/unrevocable on its own; a stolen
/// access token is only usable for this long. Overridable via the `access_token_lifetime_secs`
/// setting (`lanrurugi_api::settings`); this constant is the fallback when that field has never
/// been set, and the value `settings.rs`'s own default table uses.
pub const DEFAULT_ACCESS_TOKEN_LIFETIME_SECS: u64 = 4 * 60 * 60;
/// Default refresh-token lifetime — long, but real: exceeding it forces a full re-login even for
/// a chain that's been actively rotating the whole time (see `refresh_tokens`' own rotation docs
/// on why the absolute expiry is anchored to the original login, not extended per-rotation).
/// Overridable via the `refresh_token_lifetime_secs` setting, same pattern as the access token.
pub const DEFAULT_REFRESH_TOKEN_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;
