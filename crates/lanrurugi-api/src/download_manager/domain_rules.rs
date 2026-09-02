//! Domain-pattern matching for per-domain concurrency/rate-limit rules
//! (`specs/005-download-plugin-progress/data-model.md`'s `Domain Rule`, spec FR-007/FR-009).

use serde::{Deserialize, Serialize};

/// A single rule pairing a domain pattern with a concurrency cap and/or rate-limit cap — mirrors
/// `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`'s `DomainRule` field-for-field (both a
/// plugin's own `pluginOptions()`-declared default and a user's persisted override use this same
/// shape — see `specs/005-download-plugin-progress/contracts/download-settings-api.md`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DomainRule {
    /// An exact hostname (`"example.com"`), a wildcard covering subdomains (`"*.example.com"`),
    /// or absent/`"*"` for the general, non-domain-specific fallback rule. Case-insensitive; no
    /// port/scheme component (spec Assumptions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_bytes_per_sec: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// The concurrency/rate-limit caps that apply to one specific download target, after resolving
/// every candidate `DomainRule` against its hostname (see [`resolve`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResolvedLimits {
    pub max_concurrent: Option<u32>,
    pub max_bytes_per_sec: Option<u64>,
}

/// Resolves the effective limits for `hostname` against `rules`, applying exact-hostname-beats-
/// wildcard-beats-general-fallback precedence (spec FR-007/FR-009) — independently for
/// concurrency and rate limiting, since a target could in principle get its concurrency cap from
/// one rule and its rate limit from a different one if that's how the plugin/user configured the
/// two lists (data-model.md's Relationships section).
///
/// `hostname` is matched case-insensitively; callers are expected to have already stripped any
/// port/scheme (spec Assumptions — "domain" means the hostname portion only).
pub fn resolve(rules: &[DomainRule], hostname: &str) -> ResolvedLimits {
    let hostname = hostname.to_ascii_lowercase();

    let mut exact: Option<&DomainRule> = None;
    let mut wildcard: Option<&DomainRule> = None;
    let mut fallback: Option<&DomainRule> = None;

    for rule in rules {
        match rule.pattern.as_deref() {
            None => fallback = fallback.or(Some(rule)),
            Some("*") => fallback = fallback.or(Some(rule)),
            Some(p) => {
                let p = p.to_ascii_lowercase();
                if let Some(suffix) = p.strip_prefix("*.") {
                    if hostname == suffix || hostname.ends_with(&format!(".{suffix}")) {
                        wildcard = wildcard.or(Some(rule));
                    }
                } else if p == hostname {
                    exact = exact.or(Some(rule));
                }
            }
        }
    }

    // Resolved independently per field: an exact-hostname rule that declares only
    // `max_concurrent` (say) doesn't block a wildcard/fallback rule's own `max_bytes_per_sec` from
    // applying — each field falls back to the next-lower-precedence rule that actually declares
    // it, rather than an all-or-nothing "this whole rule wins or loses."
    let max_concurrent = [exact, wildcard, fallback]
        .into_iter()
        .flatten()
        .find_map(|r| r.max_concurrent);
    let max_bytes_per_sec = [exact, wildcard, fallback]
        .into_iter()
        .flatten()
        .find_map(|r| r.max_bytes_per_sec);

    ResolvedLimits {
        max_concurrent,
        max_bytes_per_sec,
    }
}

/// Deterministic string key identifying which `Arc<Semaphore>`/rate-limiter instance a resolved
/// set of rules should share (`download_manager::mod`'s concurrency/rate-limit maps are keyed by
/// this, not the raw hostname, so e.g. two different subdomains matching the same wildcard rule
/// correctly share one limit — spec US2 Acceptance Scenario 2). Returns the *matching rule's own
/// pattern* (exact hostname, wildcard, or `"*"` for the fallback) — two different hostnames that
/// resolve to the same rule always get the same key.
pub fn resolved_key(rules: &[DomainRule], hostname: &str) -> String {
    let hostname = hostname.to_ascii_lowercase();
    for rule in rules {
        match rule.pattern.as_deref() {
            Some(p) if p != "*" => {
                let p_lower = p.to_ascii_lowercase();
                if let Some(suffix) = p_lower.strip_prefix("*.") {
                    if hostname == suffix || hostname.ends_with(&format!(".{suffix}")) {
                        return p_lower;
                    }
                } else if p_lower == hostname {
                    return p_lower;
                }
            }
            _ => {}
        }
    }
    "*".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(
        pattern: Option<&str>,
        max_concurrent: Option<u32>,
        max_bytes_per_sec: Option<u64>,
    ) -> DomainRule {
        DomainRule {
            pattern: pattern.map(str::to_string),
            max_concurrent,
            max_bytes_per_sec,
            description: None,
        }
    }

    #[test]
    fn exact_hostname_beats_wildcard() {
        let rules = vec![
            rule(Some("*.example.com"), Some(1), None),
            rule(Some("cdn.example.com"), Some(5), None),
        ];
        let resolved = resolve(&rules, "cdn.example.com");
        assert_eq!(resolved.max_concurrent, Some(5));
    }

    #[test]
    fn wildcard_covers_any_matching_subdomain() {
        let rules = vec![rule(Some("*.example.com"), Some(2), None)];
        assert_eq!(resolve(&rules, "a.example.com").max_concurrent, Some(2));
        assert_eq!(resolve(&rules, "b.example.com").max_concurrent, Some(2));
        assert_eq!(resolve(&rules, "example.com").max_concurrent, Some(2));
        assert_eq!(resolve(&rules, "other.com").max_concurrent, None);
    }

    #[test]
    fn wildcard_beats_general_fallback() {
        let rules = vec![
            rule(None, Some(1), None),
            rule(Some("*.example.com"), Some(3), None),
        ];
        assert_eq!(
            resolve(&rules, "a.example.com").max_concurrent,
            Some(3),
            "wildcard rule outranks the pattern-less fallback"
        );
        assert_eq!(
            resolve(&rules, "unrelated.com").max_concurrent,
            Some(1),
            "fallback still applies to a domain no other rule matches"
        );
    }

    #[test]
    fn no_matching_rule_and_no_fallback_is_unmanaged() {
        let rules = vec![rule(Some("example.com"), Some(2), None)];
        let resolved = resolve(&rules, "unrelated.com");
        assert_eq!(resolved.max_concurrent, None);
        assert_eq!(resolved.max_bytes_per_sec, None);
    }

    #[test]
    fn concurrency_and_rate_limit_resolve_independently() {
        let rules = vec![
            rule(Some("cdn.example.com"), Some(5), None),
            rule(Some("*.example.com"), None, Some(1_048_576)),
        ];
        let resolved = resolve(&rules, "cdn.example.com");
        assert_eq!(
            resolved.max_concurrent,
            Some(5),
            "exact rule's own concurrency applies"
        );
        assert_eq!(
            resolved.max_bytes_per_sec,
            Some(1_048_576),
            "wildcard rule's rate limit still applies since the exact rule didn't declare one"
        );
    }

    #[test]
    fn matching_is_case_insensitive() {
        let rules = vec![rule(Some("*.Example.COM"), Some(2), None)];
        assert_eq!(resolve(&rules, "CDN.example.com").max_concurrent, Some(2));
    }

    #[test]
    fn resolved_key_groups_matching_subdomains_together() {
        let rules = vec![rule(Some("*.example.com"), Some(2), None)];
        assert_eq!(resolved_key(&rules, "a.example.com"), "*.example.com");
        assert_eq!(resolved_key(&rules, "b.example.com"), "*.example.com");
    }

    #[test]
    fn resolved_key_falls_back_to_asterisk_when_unmanaged() {
        let rules = vec![rule(Some("example.com"), Some(2), None)];
        assert_eq!(resolved_key(&rules, "unrelated.com"), "*");
    }
}
