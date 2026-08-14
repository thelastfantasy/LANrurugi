//! Small crypto helpers shared across this crate's own signing code (`session::jwt`) and
//! `lanrurugi-server::middleware::auth`'s API-token comparison — factored out once a third
//! near-identical copy of the same constant-time comparison was about to exist (constitution's
//! "near-identical logic across sibling modules MUST be factored into a shared helper" rule).

/// Avoids leaking how many leading bytes of a submitted value matched the expected one via
/// response-time differences (a timing side channel, CWE-208) — a plain `==` short-circuits on
/// the first mismatched byte, which this deliberately doesn't for the compared content (the
/// length check itself still short-circuits, but length isn't the secret part).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
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
    fn equal_strings_match() {
        assert!(constant_time_eq("hello", "hello"));
    }

    #[test]
    fn different_strings_do_not_match() {
        assert!(!constant_time_eq("hello", "world"));
    }

    #[test]
    fn different_lengths_do_not_match() {
        assert!(!constant_time_eq("short", "a much longer string"));
    }

    #[test]
    fn empty_strings_match() {
        assert!(constant_time_eq("", ""));
    }
}
