//! The route policy, read from the file that actually enforces it.
//!
//! `crates/lanrurugi-api/policy/route_policy.csv` is the authority for who may call what: it is
//! `include_str!`-ed into the server and consulted by `require_api_key` on every request. This
//! module reads the same file and answers the same question, so the document can state each
//! endpoint's real authentication requirement instead of one blanket claim.
//!
//! That distinction is not academic. The first version of the document declared a single global
//! `security: [{bearerAuth: []}]`, which is wrong for 32 of the 231 operations: 19 endpoints
//! deliberately reject API tokens (they are session-only, and are exactly the security-sensitive
//! ones — token and session management, password change, dropping the database), and 13 accept no
//! credentials at all. Swagger UI drew a padlock on all 231.
//!
//! Two things this must reproduce exactly, because it is answering a question the server answers
//! at runtime:
//!
//! - **The effect rule.** `route_model.conf` is
//!   `some(where (p.eft == allow)) && !some(where (p.eft == deny))` — an allow anywhere is not
//!   enough; a deny *anywhere in the matching set* wins. A "most specific pattern wins" model (the
//!   intuitive one, and what a hand analysis reaches for) gets the session-only endpoints wrong.
//! - **`keyMatch2`.** `:param` matches exactly one path segment; a trailing `*` matches the rest.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};

/// The Casbin subjects that appear in the route policy.
///
/// `GuestVisitor` is the restricted-guest subject from `007-guest-restricted-access`: an
/// unauthenticated visitor when guest mode is on, scoped to guest-visible categories. It is tracked
/// separately from `Anonymous` because the two mean different things to a reader — `Anonymous` is
/// "no credentials needed, ever", `GuestVisitor` is "no credentials needed *when the operator has
/// enabled guest access*".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Anonymous,
    GuestVisitor,
    Session,
    TokenAdmin,
    TokenGuest,
}

impl Role {
    /// The spelling `route_policy.csv` uses, so a gate's message can quote the file back.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Anonymous => "anonymous",
            Role::GuestVisitor => "guest_visitor",
            Role::Session => "session",
            Role::TokenAdmin => "token_admin",
            Role::TokenGuest => "token_guest",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            "anonymous" => Some(Role::Anonymous),
            "guest_visitor" => Some(Role::GuestVisitor),
            "session" => Some(Role::Session),
            "token_admin" => Some(Role::TokenAdmin),
            "token_guest" => Some(Role::TokenGuest),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    Allow,
    Deny,
}

#[derive(Debug)]
struct Rule {
    role: Role,
    object: String,
    action: String,
    effect: Effect,
    line: usize,
}

/// Who may reach one route.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Access {
    /// Callable with no credentials at all.
    pub anonymous: bool,
    /// Callable with no credentials when guest mode is enabled.
    pub guest_visitor: bool,
    /// Callable with a first-party session cookie.
    pub session: bool,
    /// Callable with an administrator API token.
    pub token_admin: bool,
    /// Callable with a guest (read-only) API token. Never true where `token_admin` is false — a
    /// guest token is a strict subset of an admin one in this policy.
    pub token_guest: bool,
}

impl Access {
    /// Any API token reaches this route.
    pub fn any_token(self) -> bool {
        self.token_admin || self.token_guest
    }

    /// Only an administrator token reaches it — a guest token is refused. 117 of this API's routes
    /// are in this state, which is why the document states it rather than leaving a reader with a
    /// guest token to discover it as a 403.
    pub fn admin_token_only(self) -> bool {
        self.token_admin && !self.token_guest
    }

    /// No credential class reaches this route. A route in this state is unreachable in production,
    /// which is a defect rather than a documentation choice — the caller gates on it.
    pub fn is_unreachable(self) -> bool {
        !self.anonymous && !self.guest_visitor && !self.session && !self.any_token()
    }

    /// Only an API token gets in, never a browser session.
    pub fn is_token_only(self) -> bool {
        self.any_token() && !self.session && !self.anonymous && !self.guest_visitor
    }

    /// Only a browser session gets in; every token role is refused.
    pub fn is_session_only(self) -> bool {
        self.session && !self.any_token() && !self.anonymous && !self.guest_visitor
    }

    /// Reachable without credentials.
    pub fn is_public(self) -> bool {
        self.anonymous || self.guest_visitor
    }
}

#[derive(Debug, Default)]
pub struct Policy {
    rules: Vec<Rule>,
}

impl Policy {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the route policy at {}", path.display()))?;
        Self::parse(&text)
            .with_context(|| format!("parsing the route policy at {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut rules = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let fields: Vec<&str> = line.split(',').map(str::trim).collect();
            if fields.first() != Some(&"p") {
                // Not a policy line. The activity policy shares this syntax but not this file; a
                // line that is neither a comment nor a `p` entry is a file this module should not
                // silently skim past.
                anyhow::bail!(
                    "line {} is neither a comment nor a `p` rule: {line}",
                    index + 1
                );
            }
            if fields.len() != 5 {
                anyhow::bail!(
                    "line {} has {} fields, expected 5 (p, sub, obj, act, eft): {line}",
                    index + 1,
                    fields.len()
                );
            }
            let role = Role::parse(fields[1]).with_context(|| {
                format!(
                    "line {} names the unknown subject {:?}. A new subject must be added to `Role` \
                     here as well, or every route it governs will be reported as unreachable.",
                    index + 1,
                    fields[1]
                )
            })?;
            let effect = match fields[4] {
                "allow" => Effect::Allow,
                "deny" => Effect::Deny,
                other => anyhow::bail!(
                    "line {} has effect {other:?}, expected allow or deny",
                    index + 1
                ),
            };
            rules.push(Rule {
                role,
                object: normalise(fields[2]),
                action: fields[3].to_string(),
                effect,
                line: index + 1,
            });
        }
        if rules.is_empty() {
            anyhow::bail!("the route policy contained no rules");
        }
        Ok(Self { rules })
    }

    /// Number of rules read. Used by the coverage report to make "the file was actually read" a
    /// checkable number rather than an assumption.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Whether one role reaches `method path`, by the same `some(allow) && !some(deny)` rule the
    /// enforcer applies. Exposed because a gate needs to ask about a *specific* role — the shape of
    /// this policy makes the general question ("can anybody reach this?") uninteresting.
    pub fn allows(&self, role: Role, method: &str, path: &str) -> bool {
        let object = normalise(path);
        let action = method.to_ascii_uppercase();
        let mut allowed = false;
        let mut denied = false;
        for rule in &self.rules {
            if rule.role != role || (rule.action != "*" && rule.action != action) {
                continue;
            }
            if !key_match2(&rule.object, &object) {
                continue;
            }
            match rule.effect {
                Effect::Allow => allowed = true,
                Effect::Deny => denied = true,
            }
        }
        allowed && !denied
    }

    /// Who may reach `method path`.
    ///
    /// `None` means no rule in the file mentions this route at all — distinct from a route every
    /// rule denies. The first is a missing policy (a route reachable by nobody, which the caller
    /// gates on); the second is a deliberate denial.
    pub fn access(&self, method: &str, path: &str) -> Option<Access> {
        let object = normalise(path);
        let action = method.to_ascii_uppercase();
        let mut mentioned = false;
        let mut allowed = BTreeSet::new();
        let mut denied = BTreeSet::new();
        for rule in &self.rules {
            if rule.action != "*" && rule.action != action {
                continue;
            }
            if !key_match2(&rule.object, object.as_str()) {
                continue;
            }
            mentioned = true;
            match rule.effect {
                Effect::Allow => {
                    allowed.insert(rule.role);
                }
                Effect::Deny => {
                    denied.insert(rule.role);
                }
            }
        }
        if !mentioned {
            return None;
        }
        // `some(allow) && !some(deny)`, per `route_model.conf`.
        let reachable = |role: Role| allowed.contains(&role) && !denied.contains(&role);
        Some(Access {
            anonymous: reachable(Role::Anonymous),
            guest_visitor: reachable(Role::GuestVisitor),
            session: reachable(Role::Session),
            token_admin: reachable(Role::TokenAdmin),
            token_guest: reachable(Role::TokenGuest),
        })
    }

    /// Rules that match no served route, as `(line, "sub obj act")`.
    ///
    /// The opposite direction of the reachability gate: that one catches a route nobody can call,
    /// this one catches a rule that governs nothing — a route renamed or removed without its rule
    /// following it. Harmless to the server (nothing matches, so it grants nothing) and therefore
    /// silent forever without this, while the file slowly stops describing the routes it is cited
    /// for. Wide rules are exempt: `/api/*` is deliberately broad, not stale.
    pub fn unused_rules(&self, served: &[(String, String)]) -> Vec<(usize, String)> {
        let mut unused = Vec::new();
        for rule in &self.rules {
            if rule.object == "*" || rule.object.ends_with("/*") {
                continue;
            }
            let matched = served.iter().any(|(method, path)| {
                (rule.action == "*" || rule.action == method.to_ascii_uppercase())
                    && key_match2(&rule.object, &normalise(path))
            });
            if !matched {
                unused.push((
                    rule.line,
                    format!("{} {} {}", rule.role.as_str(), rule.object, rule.action),
                ));
            }
        }
        unused
    }

    /// The line numbers of every rule that matches, for diagnostics.
    pub fn matching_lines(&self, method: &str, path: &str) -> Vec<usize> {
        let object = normalise(path);
        let action = method.to_ascii_uppercase();
        self.rules
            .iter()
            .filter(|rule| {
                (rule.action == "*" || rule.action == action) && key_match2(&rule.object, &object)
            })
            .map(|rule| rule.line)
            .collect()
    }
}

/// The document spells parameters `{id}`; the policy spells them `:id`. Compare one spelling.
fn normalise(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut in_brace = false;
    for ch in path.chars() {
        match ch {
            '{' => {
                in_brace = true;
                out.push(':');
            }
            '}' => in_brace = false,
            _ if in_brace => {}
            _ => out.push(ch),
        }
    }
    out.trim_end_matches('/').to_string()
}

/// Casbin's `keyMatch2`, reproduced without a regex engine.
///
/// `:param` matches exactly one segment (any value, including empty); a `*` segment matches the
/// remaining segments and must be last. Casbin implements this by rewriting the pattern to a regex
/// (`/:x` → `/[^/]+`, `/*` → `/.*`) and anchoring it; segment-wise comparison is the same relation
/// and does not need the dependency.
fn key_match2(pattern: &str, path: &str) -> bool {
    let mut pattern_segments = pattern.split('/');
    let mut path_segments = path.split('/');
    loop {
        match (pattern_segments.next(), path_segments.next()) {
            (None, None) => return true,
            (Some("*"), Some(_)) => return true,
            (Some(segment), Some(candidate)) => {
                if !segment.starts_with(':') && segment != candidate {
                    return false;
                }
            }
            // A `*` needs at least the segment it replaces, matching the regex Casbin builds
            // (`/api/*` becomes `/api/.*`, which does not match a bare `/api`).
            _ => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shaped like the real file: `session` is the only subject with a wildcard action, and every
    // deny is written for *both* token roles. Both details matter — the first version of this
    // sample gave `token_admin` a wildcard action (so `PATCH` was legitimately reachable) and
    // denied `/api/tokens` for `token_admin` only (so a guest token could still reach it). The
    // implementation was right in both cases; the fixture was not.
    const SAMPLE: &str = "\
# comment
p, session, /api/*, *, allow
p, token_admin, /api/*, GET, allow
p, token_guest, /api/*, GET, allow
p, anonymous, /api/login, POST, allow
p, token_admin, /api/tokens, GET, deny
p, token_guest, /api/tokens, GET, deny
p, token_guest, /api/archives, DELETE, deny
";

    #[test]
    fn key_match2_handles_parameters_and_wildcards() {
        assert!(key_match2("/api/*", "/api/archives/abc/metadata"));
        assert!(key_match2("/api/archives/:id", "/api/archives/abc"));
        assert!(!key_match2(
            "/api/archives/:id",
            "/api/archives/abc/metadata"
        ));
        assert!(!key_match2("/api/archives/:id", "/api/archives"));
        // `/api/*` is `/api/.*`, which does not match `/api` itself.
        assert!(!key_match2("/api/*", "/api"));
        assert!(key_match2("/api/login", "/api/login"));
        assert!(!key_match2("/api/login", "/api/logout"));
    }

    #[test]
    fn a_deny_anywhere_wins_over_an_allow_elsewhere() {
        let policy = Policy::parse(SAMPLE).unwrap();
        // `session` reaches everything; `token_admin` is allowed `/api/*` but denied `/api/tokens`
        // GET specifically.
        let tokens = policy.access("GET", "/api/tokens").unwrap();
        assert!(tokens.session);
        assert!(
            !tokens.any_token(),
            "the specific deny must beat the wildcard allow"
        );
        let activity = policy.access("GET", "/api/activity").unwrap();
        assert!(activity.any_token(), "no deny covers this one");
        // Both roles reach it here: the sample grants each of them `GET` on `/api/*`.
        assert!(activity.token_admin && activity.token_guest);
    }

    #[test]
    fn anonymous_and_the_action_wildcard_are_handled() {
        let policy = Policy::parse(SAMPLE).unwrap();
        assert!(policy.access("POST", "/api/login").unwrap().anonymous);
        assert!(!policy.access("GET", "/api/login").unwrap().anonymous);
        // `session` has `*` for its action, so it reaches a method the others do not.
        assert!(policy.access("PATCH", "/api/anything").unwrap().session);
        assert!(!policy.access("PATCH", "/api/anything").unwrap().any_token());
    }

    #[test]
    fn a_route_no_rule_mentions_is_none_not_denied() {
        let policy = Policy::parse(SAMPLE).unwrap();
        assert!(policy.access("GET", "/health").is_none());
        assert!(policy.access("GET", "/api/nonexistent").is_some());
    }

    #[test]
    fn the_document_and_policy_spellings_agree() {
        let policy = Policy::parse("p, session, /api/archives/:id/metadata, GET, allow\n").unwrap();
        assert!(
            policy
                .access("GET", "/api/archives/{id}/metadata")
                .unwrap()
                .session
        );
    }

    #[test]
    fn an_unknown_subject_is_an_error_rather_than_a_silent_deny() {
        let error = Policy::parse("p, root, /api/*, *, allow\n")
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown subject"), "{error}");
    }

    #[test]
    fn unused_rules_reports_only_rules_no_served_route_can_match() {
        // One wildcard (never stale by definition), one rule for a route that is still served,
        // one for a route that is not, and one whose *method* is no longer served.
        let policy = Policy::parse(
            "p, session, /api/*, *, allow
p, anonymous, /api/login, POST, allow
p, session, /api/old-thing, GET, allow
p, session, /api/login, DELETE, allow
",
        )
        .unwrap();
        let served = [
            ("POST".to_string(), "/api/login".to_string()),
            ("GET".to_string(), "/api/archives/{id}".to_string()),
        ];
        let unused: Vec<String> = policy
            .unused_rules(&served)
            .into_iter()
            .map(|(line, rule)| format!("{line}: {rule}"))
            .collect();
        assert_eq!(
            unused,
            vec![
                "3: session /api/old-thing GET".to_string(),
                "4: session /api/login DELETE".to_string()
            ],
            "the wildcard is exempt, the served rule is kept, and both the removed route and the \
             removed method are reported with their file line"
        );
    }
}
