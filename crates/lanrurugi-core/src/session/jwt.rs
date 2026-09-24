//! Minimal RFC 7519 JWT (HS256 only) — hand-rolled rather than pulling in a general-purpose JWT
//! crate (which would carry alg-confusion-prone API surface and RSA/EC machinery this project
//! never uses), matching the same "small, purpose-built primitive over a heavyweight dependency"
//! stance the previous flat `session.rs` this module replaces already took for its own
//! HMAC-SHA256 signing.
//!
//! Structure is the real RFC 7519 shape (`base64url(header).base64url(payload).base64url(sig)`,
//! HMAC-SHA256 over the first two dot-joined segments) — not a lookalike — so a token issued here
//! is inspectable by any standard JWT tool for debugging, even though nothing else in this
//! codebase currently needs a general-purpose decoder.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::crypto::constant_time_eq;

#[derive(Debug, Serialize, Deserialize)]
struct Header {
    alg: String,
    typ: String,
}

const ALG: &str = "HS256";
const TYP: &str = "JWT";

/// Claims carried by an access token. Single-user system (no user table exists anywhere in this
/// codebase's Redis schema) — `sub` is a fixed constant identifying "the one admin principal"
/// rather than a real per-user ID, since RFC 7519's `sub` only needs to name who `exp`/`iat`
/// apply to, not key any lookup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessClaims {
    pub sub: String,
    /// Issued-at, unix seconds.
    pub iat: u64,
    /// Expiry, unix seconds.
    pub exp: u64,
    /// The refresh-token family (`lanrurugi_storage::refresh_tokens`) this access token was
    /// minted from — never consulted during verification (access tokens stay self-contained and
    /// unrevocable on their own, by design; only refresh tokens are revocable), but carried
    /// through so a future "log out everywhere" diagnostic/admin action can correlate an access
    /// token back to which login chain it came from without an extra Redis round-trip.
    pub fid: String,
}

/// Fixed subject value — see [`AccessClaims::sub`]'s own docs for why this is a constant rather
/// than a real user ID.
pub const SUBJECT: &str = "admin";

/// Issues a signed access token valid from `now_unix_secs` for `lifetime_secs`.
pub fn issue_access_token(
    secret: &[u8],
    now_unix_secs: u64,
    lifetime_secs: u64,
    family_id: &str,
) -> String {
    let header = Header {
        alg: ALG.to_string(),
        typ: TYP.to_string(),
    };
    let claims = AccessClaims {
        sub: SUBJECT.to_string(),
        iat: now_unix_secs,
        exp: now_unix_secs + lifetime_secs,
        fid: family_id.to_string(),
    };
    let header_b64 =
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("Header always serializes"));
    let claims_b64 = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).expect("AccessClaims always serializes"));
    let signing_input = format!("{header_b64}.{claims_b64}");
    let sig_b64 = sign(secret, signing_input.as_bytes());
    format!("{signing_input}.{sig_b64}")
}

/// Verifies a token's structure, signature, and expiry. Returns the parsed claims on success —
/// callers that only need a yes/no answer can use `.is_some()`, but returning the claims costs
/// nothing extra and avoids a second parse for a caller that wants `fid`/`sub`/`iat`.
pub fn verify_access_token(secret: &[u8], token: &str, now_unix_secs: u64) -> Option<AccessClaims> {
    let claims = decode_claims_ignoring_expiry(secret, token)?;
    if now_unix_secs > claims.exp {
        return None;
    }
    Some(claims)
}

/// Structure + signature verification only — deliberately **not** a general-purpose entry point;
/// only [`verify_access_token`] (which adds the expiry check back) and
/// [`family_id_ignoring_expiry`] (which needs `fid` specifically from a token that may already be
/// expired) may call this. A cryptographically valid-but-expired token still proves "this really
/// was issued by this server," which is all `family_id_ignoring_expiry`'s own single narrow use
/// case (`login::logout`, correlating back to which refresh-token family to revoke without
/// depending on the refresh cookie itself still being present/readable) needs — it is never a
/// substitute for `verify_access_token` in any context that authorizes a request.
fn decode_claims_ignoring_expiry(secret: &[u8], token: &str) -> Option<AccessClaims> {
    let mut parts = token.split('.');
    let header_b64 = parts.next()?;
    let claims_b64 = parts.next()?;
    let sig_b64 = parts.next()?;
    if parts.next().is_some() {
        return None; // more than 3 segments — not a well-formed JWT
    }

    let header_bytes = URL_SAFE_NO_PAD.decode(header_b64).ok()?;
    let header: Header = serde_json::from_slice(&header_bytes).ok()?;
    if header.alg != ALG || header.typ != TYP {
        return None;
    }

    let signing_input = format!("{header_b64}.{claims_b64}");
    let expected_sig_b64 = sign(secret, signing_input.as_bytes());
    if !constant_time_eq(sig_b64, &expected_sig_b64) {
        return None;
    }

    let claims_bytes = URL_SAFE_NO_PAD.decode(claims_b64).ok()?;
    serde_json::from_slice(&claims_bytes).ok()
}

/// Extracts `fid` (refresh-token family id) from an access token whose signature still checks
/// out, **without** requiring it to still be unexpired — see
/// [`decode_claims_ignoring_expiry`]'s own docs for why this is safe for this one narrow purpose
/// and must not be reused as a general auth check. Used only by `login::logout`, so a user who
/// clicks "log out" after their access token has already expired (but before the browser tab was
/// refreshed/closed) still gets their refresh-token family actually revoked, rather than logout
/// silently becoming a no-op in exactly the case where cleaning up server-side state matters most.
pub fn family_id_ignoring_expiry(secret: &[u8], token: &str) -> Option<String> {
    decode_claims_ignoring_expiry(secret, token).map(|claims| claims.fid)
}

fn sign(secret: &[u8], data: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(data);
    URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &[u8] = b"test-secret";

    #[test]
    fn issued_token_verifies_immediately_and_carries_claims() {
        let token = issue_access_token(SECRET, 1_000, 3_600, "family-1");
        let claims = verify_access_token(SECRET, &token, 1_000).expect("should verify");
        assert_eq!(claims.sub, SUBJECT);
        assert_eq!(claims.iat, 1_000);
        assert_eq!(claims.exp, 4_600);
        assert_eq!(claims.fid, "family-1");
    }

    #[test]
    fn token_is_valid_right_up_to_expiry_but_not_after() {
        let token = issue_access_token(SECRET, 1_000, 3_600, "family-1");
        assert!(verify_access_token(SECRET, &token, 4_600).is_some());
        assert!(verify_access_token(SECRET, &token, 4_601).is_none());
    }

    #[test]
    fn tampered_payload_is_rejected() {
        let token = issue_access_token(SECRET, 1_000, 3_600, "family-1");
        let mut parts: Vec<&str> = token.split('.').collect();
        // Forge a payload claiming a much later expiry, keep the original (now-mismatched) sig.
        let forged_claims = AccessClaims {
            sub: SUBJECT.to_string(),
            iat: 1_000,
            exp: 999_999_999,
            fid: "family-1".to_string(),
        };
        let forged_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged_claims).unwrap());
        parts[1] = &forged_b64;
        let forged = parts.join(".");
        assert!(verify_access_token(SECRET, &forged, 1_000).is_none());
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let token = issue_access_token(b"secret-a", 1_000, 3_600, "family-1");
        assert!(verify_access_token(b"secret-b", &token, 1_000).is_none());
    }

    #[test]
    fn malformed_structure_is_rejected() {
        assert!(verify_access_token(SECRET, "not-a-jwt", 1_000).is_none());
        assert!(verify_access_token(SECRET, "only.two", 1_000).is_none());
        assert!(verify_access_token(SECRET, "too.many.parts.here", 1_000).is_none());
        assert!(verify_access_token(SECRET, "", 1_000).is_none());
    }

    #[test]
    fn tampered_algorithm_header_is_rejected() {
        // A header claiming a different alg (e.g. attempting an alg-confusion attack) must fail
        // even if the rest of the token structure is otherwise well-formed — this is exactly the
        // new attack surface a real JWT header (vs. the old flat `expiry.hexsig` format, which
        // had no attacker-modifiable header at all) introduces, so it gets its own explicit test.
        let token = issue_access_token(SECRET, 1_000, 3_600, "family-1");
        let mut parts: Vec<&str> = token.split('.').collect();
        let forged_header = Header {
            alg: "none".to_string(),
            typ: TYP.to_string(),
        };
        let forged_header_b64 = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged_header).unwrap());
        parts[0] = &forged_header_b64;
        let forged = parts.join(".");
        assert!(verify_access_token(SECRET, &forged, 1_000).is_none());
    }

    #[test]
    fn family_id_ignoring_expiry_still_returns_fid_after_the_token_has_expired() {
        let token = issue_access_token(SECRET, 1_000, 3_600, "family-1");
        // Confirmed genuinely expired by this point per the normal, expiry-checking path.
        assert!(verify_access_token(SECRET, &token, 4_601).is_none());
        assert_eq!(
            family_id_ignoring_expiry(SECRET, &token).as_deref(),
            Some("family-1"),
        );
    }

    #[test]
    fn family_id_ignoring_expiry_still_rejects_a_bad_signature() {
        let token = issue_access_token(b"secret-a", 1_000, 3_600, "family-1");
        assert_eq!(family_id_ignoring_expiry(b"secret-b", &token), None);
    }
}
