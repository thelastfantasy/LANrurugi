//! Signed, stateless browser-login session tokens. Distinct from the API-key contract
//! (constitution Principle II) — this exists purely for the bundled SPA's own login flow, the
//! equivalent of legacy's `$c->session('is_logged')` (`Utils/Login.pm`), but implemented as a
//! self-contained signed token (checked entirely from its own contents + a server secret) rather
//! than Mojolicious's server-side signed-cookie session store, since this project has no
//! per-request session storage layer to hook into.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

pub const COOKIE_NAME: &str = "lanrurugi_session";
/// Matches legacy's own session expiration (`Controller/Login.pm::check`: `expiration => 60*60*24`).
pub const SESSION_LIFETIME_SECS: u64 = 60 * 60 * 24;

fn sign(secret: &[u8], expiry: u64) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(&expiry.to_be_bytes());
    let sig = mac.finalize().into_bytes();
    format!("{expiry}.{}", hex_encode(&sig))
}

/// Issues a token valid from `now_unix_secs` for [`SESSION_LIFETIME_SECS`].
pub fn issue_token(secret: &[u8], now_unix_secs: u64) -> String {
    sign(secret, now_unix_secs + SESSION_LIFETIME_SECS)
}

/// Verifies a token's signature and that it hasn't expired.
pub fn verify_token(secret: &[u8], token: &str, now_unix_secs: u64) -> bool {
    let Some((expiry_str, sig_hex)) = token.split_once('.') else {
        return false;
    };
    let Ok(expiry) = expiry_str.parse::<u64>() else {
        return false;
    };
    if now_unix_secs > expiry {
        return false;
    }
    let expected = sign(secret, expiry);
    let Some((_, expected_sig_hex)) = expected.split_once('.') else {
        return false;
    };
    constant_time_eq(sig_hex, expected_sig_hex)
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").expect("writing to a String cannot fail");
    }
    s
}

/// Same constant-time comparison rationale as `lanrurugi_server::middleware::auth` — avoids a
/// timing side channel on the signature check.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issued_token_verifies_immediately() {
        let secret = b"test-secret";
        let token = issue_token(secret, 1_000);
        assert!(verify_token(secret, &token, 1_000));
        assert!(verify_token(secret, &token, 1_000 + SESSION_LIFETIME_SECS));
    }

    #[test]
    fn expired_token_is_rejected() {
        let secret = b"test-secret";
        let token = issue_token(secret, 1_000);
        assert!(!verify_token(
            secret,
            &token,
            1_000 + SESSION_LIFETIME_SECS + 1
        ));
    }

    #[test]
    fn tampered_expiry_is_rejected() {
        let secret = b"test-secret";
        let token = issue_token(secret, 1_000);
        let (_, sig) = token.split_once('.').unwrap();
        let forged = format!("{}.{sig}", 1_000 + SESSION_LIFETIME_SECS * 100);
        assert!(!verify_token(secret, &forged, 1_000));
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let token = issue_token(b"secret-a", 1_000);
        assert!(!verify_token(b"secret-b", &token, 1_000));
    }
}
