//! Password hashing/verification, read-compatible with a migrated legacy password hash
//! (constitution Principle I). Legacy stores the admin password as an RFC2307-tagged hash —
//! verified: `Model/Config.pm::get_password`'s literal default value is
//! `{CRYPT}$2a$08$4AcMwwkGXnWtFTOLuw/hduQlRdqWQIBzX3UuKn.M1qTFX5R4CALxy` (bcrypt for
//! "kamimamita"), checked via `Authen::Passphrase->from_rfc2307(...)->match($pw)`
//! (`Controller/Login.pm::check`) — so the `{CRYPT}` tag just wraps a standard bcrypt hash. This
//! module strips/adds that same tag so a password set through legacy keeps working here with no
//! migration step, and a password set here remains legacy-compatible if ever pointed back at it.

const RFC2307_CRYPT_PREFIX: &str = "{CRYPT}";

/// Hashes a new plaintext password, returning it in the same `{CRYPT}$2a$...` shape legacy stores.
pub fn hash_password(plaintext: &str) -> Result<String, bcrypt::BcryptError> {
    let hash = bcrypt::hash(plaintext, bcrypt::DEFAULT_COST)?;
    Ok(format!("{RFC2307_CRYPT_PREFIX}{hash}"))
}

/// Verifies `plaintext` against a stored hash, accepting either the RFC2307-tagged legacy shape
/// or a bare bcrypt hash.
pub fn verify_password(plaintext: &str, stored: &str) -> bool {
    let bcrypt_hash = stored.strip_prefix(RFC2307_CRYPT_PREFIX).unwrap_or(stored);
    bcrypt::verify(plaintext, bcrypt_hash).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_verify_roundtrip() {
        let stored = hash_password("hunter2").unwrap();
        assert!(stored.starts_with(RFC2307_CRYPT_PREFIX));
        assert!(verify_password("hunter2", &stored));
        assert!(!verify_password("wrong", &stored));
    }

    #[test]
    fn verifies_the_actual_legacy_default_password_hash() {
        // Verified literal default from `Model/Config.pm::get_password` — confirms this module
        // reads a real migrated instance's untouched default password correctly.
        let legacy_default = "{CRYPT}$2a$08$4AcMwwkGXnWtFTOLuw/hduQlRdqWQIBzX3UuKn.M1qTFX5R4CALxy";
        assert!(verify_password("kamimamita", legacy_default));
        assert!(!verify_password("wrong", legacy_default));
    }
}
