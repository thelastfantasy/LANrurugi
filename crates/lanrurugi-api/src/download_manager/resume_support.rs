//! Remembers, per resolved domain-rule pattern, whether a server honours `Range`/`If-Range`.
//!
//! Keyed by the *matched* rule pattern (`*.hath.network`) rather than the exact hostname: an H@H
//! archive URL points at a per-node random subdomain, so an exact-host record would never be
//! consulted again, while the nodes behind one rule share the same server software. The catch-all
//! `*` rule is never used as a key — one unknown host's behaviour must not become a global claim —
//! so the exact hostname is used there instead.
//!
//! The value is the *observed* outcome (`206` accepted / `200` rejected), not a capability
//! declaration: `Accept-Ranges` is only a hint, and the proof is the response to the resume itself.

use deadpool_redis::redis::AsyncCommands;

const KEY: &str = "LANRURUGI_DOWNLOAD_RESUME_SUPPORT";

/// The key this host's observations belong to: the plugin's matched rule pattern, or the lowercased
/// hostname when only the catch-all rule matched.
pub fn pattern_for(rules: &[super::domain_rules::DomainRule], hostname: &str) -> String {
    let matched = super::domain_rules::resolved_key(rules, hostname);
    if matched == "*" {
        hostname.to_ascii_lowercase()
    } else {
        matched
    }
}

/// `None` when this pattern has never been observed.
pub async fn get(pool: &deadpool_redis::Pool, pattern: &str) -> Option<bool> {
    let mut conn = pool.get().await.ok()?;
    let raw: Option<String> = conn.hget(KEY, pattern).await.ok()?;
    match raw.as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

pub async fn set(pool: &deadpool_redis::Pool, pattern: &str, supported: bool) {
    let Ok(mut conn) = pool.get().await else {
        return;
    };
    let value = if supported { "1" } else { "0" };
    if let Err(e) = conn.hset::<_, _, _, ()>(KEY, pattern, value).await {
        tracing::warn!(pattern, error = %e, "could not record resume support");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download_manager::domain_rules::DomainRule;

    fn rule(pattern: &str) -> DomainRule {
        DomainRule {
            pattern: Some(pattern.to_string()),
            max_concurrent: None,
            max_bytes_per_sec: None,
            description: None,
        }
    }

    #[test]
    fn a_wildcard_rule_is_the_record_key() {
        let rules = vec![rule("*.hath.network")];
        assert_eq!(
            pattern_for(&rules, "a1b2.hentaiathome.hath.network"),
            "*.hath.network"
        );
    }

    #[test]
    fn a_catch_all_rule_falls_back_to_the_exact_hostname() {
        let rules = vec![rule("*")];
        assert_eq!(pattern_for(&rules, "A.Example.COM"), "a.example.com");
    }

    #[test]
    fn an_exact_rule_wins_over_the_fallback() {
        let rules = vec![rule("*"), rule("cdn.example.com")];
        assert_eq!(pattern_for(&rules, "cdn.example.com"), "cdn.example.com");
    }
}
