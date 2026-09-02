//! Live authentication configuration, read from the same `LRR_CONFIG` hash legacy itself reads
//! (`Model/Config.pm::enable_pass`/`get_password`, keys `enablepass`/`password`) so a value
//! already set through legacy's settings page takes effect here with no migration step
//! (Principle I), and a value changed through our own Settings page is live immediately — no
//! server restart, matching legacy's own behavior of reading Redis fresh on every request rather
//! than caching config at boot.
//!
//! `apikey` (legacy's single fixed API key) is deliberately **not** read here anymore — issue #54
//! replaced it with `lanrurugi_storage::api_tokens`'s real multi-token system (see
//! `.specify/memory/constitution.md` Principle II's own annotation on this being a deliberate,
//! pre-release break from legacy compatibility).
//!
//! `session_secret` has no legacy equivalent (legacy signs its session cookie with Mojolicious's
//! own app-level secret, unrelated to Redis) — it's generated once on first use and persisted
//! back to the same hash so subsequently issued tokens (both the JWT access token and the
//! refresh token, which is itself hashed-and-stored rather than signed, but shares this same
//! secret's lifecycle for simplicity — see `lanrurugi_core::session`) keep verifying across
//! restarts.

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;
use rand::RngExt;

use crate::state::AppState;
use lanrurugi_storage::keys::CONFIG_KEY;

#[derive(Debug, thiserror::Error)]
pub enum AuthConfigError {
    #[error("redis pool error: {0}")]
    Pool(#[from] deadpool_redis::PoolError),
    #[error("redis error: {0}")]
    Redis(#[from] deadpool_redis::redis::RedisError),
    /// `guestmode` is a required `LRR_CONFIG` field as of 007-guest-restricted-access —
    /// deliberately a hard error rather than a silent default: an instance predating this feature
    /// must be migrated once via the dedicated migration tool, not carry a permanent runtime
    /// fallback for a field every real deployment is expected to have going forward.
    #[error("required LRR_CONFIG field {0:?} missing — run the migration tool")]
    MissingField(&'static str),
}

const SESSION_SECRET_FIELD: &str = "session_secret";

/// Legacy's own default (`Model/Config.pm::get_password`) — the bcrypt hash for "kamimamita",
/// RFC2307-tagged. Used only when no instance (legacy or ours) has ever set a password.
pub const DEFAULT_PASSWORD_HASH: &str =
    "{CRYPT}$2a$08$4AcMwwkGXnWtFTOLuw/hduQlRdqWQIBzX3UuKn.M1qTFX5R4CALxy";

pub struct LiveAuthConfig {
    /// 007-guest-restricted-access: the site-wide guest-mode master switch (`guestmode` setting) —
    /// on its own it grants nothing; `procedure::require_api_key`'s guest-eligibility branch also
    /// requires at least one `Category::visible_to_guest` before treating an unauthenticated
    /// request as `AuthMethod::GuestVisitor` (spec FR-005/FR-006). Replaces the removed
    /// `enable_pass`/`no_fun_mode` — password login is unconditional now, so there is no longer an
    /// "open instance" concept for this struct to represent.
    pub guest_mode_enabled: bool,
    pub password_hash: String,
    pub session_secret: Vec<u8>,
    /// Overridable via the `access_token_lifetime_secs` setting (`settings.rs::NUMBER_FIELDS`) —
    /// falls back to `lanrurugi_core::session::DEFAULT_ACCESS_TOKEN_LIFETIME_SECS` when unset.
    pub access_token_lifetime_secs: u64,
    /// Overridable via the `refresh_token_lifetime_secs` setting, same pattern as above — falls
    /// back to `lanrurugi_core::session::DEFAULT_REFRESH_TOKEN_LIFETIME_SECS`.
    pub refresh_token_lifetime_secs: u64,
    pub force_secure_cookies: bool,
}

/// Reads current auth-relevant settings, falling back to `state.auth`'s CLI-supplied defaults
/// only for fields the config hash has never had set (mirrors `get_redis_conf($key, $default)`'s
/// fallback, which never writes the default back) — except `session_secret`, which genuinely
/// needs to be generated and persisted once since there's no legacy value to fall back to.
pub async fn load(state: &AppState) -> Result<LiveAuthConfig, AuthConfigError> {
    let mut conn = state.redis.config.get().await?;
    let fields: HashMap<String, String> = conn.hgetall(CONFIG_KEY).await?;

    let guest_mode_enabled = fields
        .get("guestmode")
        .map(|v| v != "0")
        .ok_or(AuthConfigError::MissingField("guestmode"))?;
    let password_hash = fields
        .get("password")
        .cloned()
        .unwrap_or_else(|| DEFAULT_PASSWORD_HASH.to_string());
    let access_token_lifetime_secs = fields
        .get("access_token_lifetime_secs")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(lanrurugi_core::session::DEFAULT_ACCESS_TOKEN_LIFETIME_SECS);
    let refresh_token_lifetime_secs = fields
        .get("refresh_token_lifetime_secs")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(lanrurugi_core::session::DEFAULT_REFRESH_TOKEN_LIFETIME_SECS);

    let session_secret = match fields.get(SESSION_SECRET_FIELD) {
        Some(hex) if !hex.is_empty() => hex_decode(hex),
        _ => {
            let generated: Vec<u8> = (0..32).map(|_| rand::rng().random()).collect();
            let _: () = conn
                .hset(CONFIG_KEY, SESSION_SECRET_FIELD, hex_encode(&generated))
                .await?;
            generated
        }
    };

    Ok(LiveAuthConfig {
        guest_mode_enabled,
        password_hash,
        session_secret,
        access_token_lifetime_secs,
        refresh_token_lifetime_secs,
        force_secure_cookies: state.auth.force_secure_cookies,
    })
}

/// Whether `headers` carries a currently-valid session cookie (the SPA's own login, not a
/// third-party API key) — shared by [`crate::login::status`] and
/// `lanrurugi_server::middleware::auth::is_authorized`'s cookie branch, so the two never drift.
pub fn session_is_valid(cfg: &LiveAuthConfig, headers: &axum::http::HeaderMap) -> bool {
    let Some(cookie_header) = headers
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let Some(token) = find_cookie(cookie_header, lanrurugi_core::session::COOKIE_NAME) else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs();
    lanrurugi_core::session::verify_access_token(&cfg.session_secret, &token, now).is_some()
}

/// Minimal `Cookie` header lookup — avoids pulling in a full cookie-jar crate for a single-name
/// lookup. `pub(crate)` — also used by `login.rs`'s `refresh` handler to read the refresh-token
/// cookie.
pub(crate) fn find_cookie(cookie_header: &str, name: &str) -> Option<String> {
    for pair in cookie_header.split(';') {
        let pair = pair.trim();
        let mut it = pair.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == name {
            return Some(v.to_string());
        }
    }
    None
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("writing to a String cannot fail");
    }
    s
}

fn hex_decode(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .filter_map(|i| s.get(i..i + 2))
        .filter_map(|byte| u8::from_str_radix(byte, 16).ok())
        .collect()
}
