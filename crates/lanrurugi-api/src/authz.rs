//! Casbin-backed authorization (issue #91) — replaces the hand-rolled `if auth.is_token()`/
//! `is_guest_token()` checks and the four separate `require_session` route-level middleware
//! mounts (`activity.rs`, `api_tokens.rs`, `database.rs`, `settings.rs`) with two declarative
//! policy files loaded once at startup: `policy/route_policy.csv` (which roles may call which
//! route+method) and `policy/activity_policy.csv` (which roles may *see* which activity log
//! entries — a resource-level, not just route-level, rule).
//!
//! # Why two separate `Enforcer`s instead of one
//!
//! `route_model.conf` answers "is this request allowed at all" (RBAC: `sub` is a fixed role
//! string, `obj` is a route pattern, `act` is an HTTP method) — a yes/no gate that runs once per
//! request, mirroring what `require_session` used to do at the middleware layer.
//!
//! `activity_model.conf` answers a different question: given a *list* of already-fetched activity
//! entries, which ones may this caller see — an ABAC rule (`sub_id`/`obj_id` string equality) run
//! once per entry inside `list_activity`/`get_facets`, not at the router layer at all (Axum only
//! resolves route+method before a handler runs; it has no way to gate on the *content* of what a
//! handler is about to return). A single combined model conflating "may call this route" with
//! "may see this specific row" would need every route-level check to also thread an `obj_id`
//! through, real routes that don't operate on a single per-record `obj_id` at all.
//!
//! # Why policy lives in source, not Redis
//!
//! Both CSV files are `include_str!`-ed into the binary (see [`route_enforcer`]/
//! [`activity_enforcer`]) rather than read from disk or a `RedisAdapter` — these rules are the
//! same kind of fixed, code-reviewed invariant `DELETION_ACTION_TYPES`/`SETTINGS_FIELD_SECTIONS`
//! elsewhere in this codebase already are: changing "can a Guest token delete a token" is a
//! behavior change that belongs in a PR and a `git blame`, not a runtime-mutable value nobody
//! reviews. If a future feature needs runtime-adjustable policy, that's a distinct addition (a
//! `casbin::Adapter` backed by Redis) layered on top of this, not a reason to move the *current*
//! fixed rules there today.

use std::sync::Arc;

use casbin::{CoreApi, DefaultModel, Enforcer, StringAdapter};
use lanrurugi_storage::activity::ActorKind;
use lanrurugi_storage::api_tokens::TokenRole;

use crate::auth_context::{AuthContext, AuthMethod};

const ROUTE_MODEL: &str = include_str!("../policy/route_model.conf");
const ROUTE_POLICY: &str = include_str!("../policy/route_policy.csv");
const ACTIVITY_MODEL: &str = include_str!("../policy/activity_model.conf");
const ACTIVITY_POLICY: &str = include_str!("../policy/activity_policy.csv");

/// The fixed roles [`AuthContext`] ever resolves to — a `&'static str` (not a `TokenRole`
/// re-export) since Casbin's `enforce` takes plain strings and `Session`/`GuestVisitor` have no
/// `TokenRole` counterpart to reuse in the first place. `None` (no `AuthContext` at all) can no
/// longer occur as of 007-guest-restricted-access — password login is unconditional, and an
/// unauthenticated-but-guest-eligible request is now `AuthMethod::GuestVisitor`, a real
/// `AuthContext`, not the absence of one; `require_api_key` never reaches `check_route` at all for
/// a request that is neither authenticated nor guest-eligible (it rejects with `401` first). The
/// `"anonymous"` Casbin subject is reintroduced for public bootstrap routes (login/theme/info/
/// version): `require_api_key` now resolves an unauthenticated caller to `AuthMethod::Anonymous`
/// when guest-mode eligibility does not apply, and lets `route_policy.csv` decide whether that is
/// allowed. Protected routes stay deny-by-default for this subject.
fn subject_role(auth: Option<&AuthContext>) -> &'static str {
    match auth.map(|a| &a.method) {
        None => unreachable!(
            "require_api_key never calls check_route with auth: None as of \
             007-guest-restricted-access — an eligible unauthenticated request is \
             AuthMethod::GuestVisitor, and an ineligible one is rejected before check_route runs"
        ),
        Some(AuthMethod::Session) => "session",
        Some(AuthMethod::Token {
            role: TokenRole::Admin,
            ..
        }) => "token_admin",
        Some(AuthMethod::Token {
            role: TokenRole::Guest,
            ..
        }) => "token_guest",
        Some(AuthMethod::GuestVisitor) => "guest_visitor",
        Some(AuthMethod::Anonymous) => "anonymous",
    }
}

/// Loads [`ROUTE_MODEL`]/[`ROUTE_POLICY`] into a fresh `Enforcer` — called once at startup
/// ([`crate::state::AppState`] holds the result behind an `Arc`, cheap to clone/share across
/// every request the same way every other repository handle there already is). A build failure
/// here means the checked-in policy/model files themselves are malformed (a real programmer
/// error caught by `mise run check`'s own server-boot smoke test, not a runtime condition any
/// caller could recover from) — `expect`, matching this codebase's own convention for
/// startup-only invariants (`now_secs`'s `SystemTime` unwrap elsewhere in this crate).
pub async fn route_enforcer() -> Enforcer {
    let model = DefaultModel::from_str(ROUTE_MODEL)
        .await
        .expect("route_model.conf is malformed");
    let adapter = StringAdapter::new(ROUTE_POLICY);
    Enforcer::new(model, adapter)
        .await
        .expect("route_policy.csv is malformed")
}

/// Loads [`ACTIVITY_MODEL`]/[`ACTIVITY_POLICY`] — see [`route_enforcer`]'s own docs for why this
/// is a second, separate `Enforcer` rather than folded into the first.
pub async fn activity_enforcer() -> Enforcer {
    let model = DefaultModel::from_str(ACTIVITY_MODEL)
        .await
        .expect("activity_model.conf is malformed");
    let adapter = StringAdapter::new(ACTIVITY_POLICY);
    Enforcer::new(model, adapter)
        .await
        .expect("activity_policy.csv is malformed")
}

/// Route-level check — backs `require_api_key`'s own single enforcement point (issue #91; see
/// `procedure.rs`'s own module docs for the merge from two separate middlewares into this one).
/// `obj` is expected to be axum's own `MatchedPath` as read from inside `require_api_key`
/// (translated to Casbin's `:param` `keyMatch2` syntax by [`axum_path_to_casbin`] first) — which,
/// because `require_api_key` is `.layer()`-ed *inside* the `.nest("/api", ...)` boundary
/// (`app.rs::build_app`), always carries the `/api` prefix (`/api/database/drop`, not
/// `/database/drop`), confirmed live and now what every `route_policy.csv` rule is written
/// against. Never the bare path a `router()` function's own `.route(...)` call declares, and never
/// the concrete request URI either (which would need every numeric/hash id spelled out in the
/// policy file itself).
pub fn check_route(
    enforcer: &Enforcer,
    auth: Option<&AuthContext>,
    obj: &str,
    method: &str,
) -> bool {
    let sub = subject_role(auth);
    enforcer.enforce((sub, obj, method)).unwrap_or_else(|e| {
        tracing::error!(error = %e, sub, obj, method, "casbin route enforce failed, denying");
        false
    })
}

/// Translates Axum's own `{param}` route-capture syntax to Casbin's `keyMatch2` `:param` syntax —
/// kept as one shared function (called at each route's own definition site when building the
/// `obj` string passed to [`check_route`]) rather than hand-writing the `:id` spelling at every
/// call site, so a route's path and its policy-check path can never silently drift apart from a
/// hand-copied typo.
pub fn axum_path_to_casbin(path: &str) -> String {
    path.replace('{', ":").replace('}', "")
}

/// One activity entry's own actor, reduced to the same `(role, id)` shape [`subject_role`]
/// produces for the *requester* — the two are compared inside [`can_view_activity_entry`]'s own
/// `enforce` call via `activity_model.conf`'s `"self"` policy target (`r.sub_id == r.obj_id &&
/// r.sub_role == r.obj_role`). `System`/`Anonymous` actors have no token role of their own; they
/// map to fixed labels no real requester role ever equals, which is intentional — nobody's
/// `subject_role` is ever literally `"system"` or `"anonymous_actor"`, so a `"self"` policy line
/// can never accidentally match a system-generated entry as if some caller "owned" it.
fn actor_role(kind: ActorKind, token_role: Option<TokenRole>) -> &'static str {
    match kind {
        ActorKind::Session => "session",
        ActorKind::System => "system",
        ActorKind::Anonymous => "anonymous",
        ActorKind::Token => match token_role {
            Some(TokenRole::Admin) => "token_admin",
            Some(TokenRole::Guest) => "token_guest",
            // A since-revoked token's role can no longer be looked up (see
            // `activity.rs::describe_actor_facet`'s own docs on the same lookup-can-fail
            // situation) — conservatively treated as neither role's "self", visible only to
            // `session` (whose `"all"` policy target doesn't check role/id at all) until proven
            // otherwise. Never widened to `"token_admin"`/`"token_guest"` by guessing.
            None => "token_revoked",
        },
    }
}

/// Whether `requester` (the caller of `GET /activity`/`GET /activity/facets`) may see an entry
/// whose actor is `(entry_actor_kind, entry_actor_id, entry_actor_token_role)` — see
/// [`crate::activity::filter_visible_entries`] for the actual per-entry loop this backs.
/// `entry_actor_token_role` must be looked up by the caller (`state.api_tokens.get(id).await`)
/// before calling this — this function itself does no I/O, so a whole page of entries can be
/// filtered without re-fetching the *requester's own* role on every iteration.
#[allow(clippy::too_many_arguments)]
pub fn can_view_activity_entry(
    enforcer: &Enforcer,
    requester_role: &str,
    requester_id: Option<&str>,
    entry_actor_kind: ActorKind,
    entry_actor_id: Option<&str>,
    entry_actor_token_role: Option<TokenRole>,
) -> bool {
    let obj_role = actor_role(entry_actor_kind, entry_actor_token_role);
    let sub_id = requester_id.unwrap_or("");
    let obj_id = entry_actor_id.unwrap_or("");
    enforcer
        .enforce((requester_role, sub_id, obj_role, obj_id))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, requester_role, obj_role, "casbin activity enforce failed, denying");
            false
        })
}

/// [`subject_role`], exposed for [`crate::activity`]'s own per-entry filtering loop (which needs
/// the *requester's* role once per request, not once per entry — kept as a public wrapper rather
/// than making `subject_role` itself `pub` to keep this module's own role-string spelling
/// ("token_admin", not "TokenAdmin"/"admin") in exactly one place).
pub fn requester_role(auth: Option<&AuthContext>) -> &'static str {
    subject_role(auth)
}

/// The requester's own token id (`AuthMethod::Token { id, .. }`) — `None` for `Session`/
/// `Anonymous`, matching `Actor::id`'s own "no per-identity id for these kinds" convention
/// ([`lanrurugi_storage::activity::Actor`]'s own docs).
pub fn requester_id(auth: Option<&AuthContext>) -> Option<&str> {
    match auth.map(|a| &a.method) {
        Some(AuthMethod::Token { id, .. }) => Some(id.as_str()),
        _ => None,
    }
}

/// Shared, cheaply-cloneable handle to both enforcers. A process-global `OnceLock`, not an
/// `AppState` field — unlike everything else on `AppState` (Redis-backed repository handles, all
/// legitimately per-connection-pool state), the loaded policy is a fixed, code-reviewed ruleset
/// that never changes for the lifetime of the process (see this module's own top-level docs on
/// why policy lives in source, not Redis) and axum's `from_fn`-based middleware
/// (`procedure::require_session`) is registered at `Router<AppState>` build time — *before*
/// `AppState` itself exists to extract a field from — so a `State<AppState>` extractor on that
/// middleware can't resolve `authz` even if the field existed. A global avoids that ordering
/// problem entirely rather than working around it.
#[derive(Clone)]
pub struct Authz {
    pub route: Arc<Enforcer>,
    pub activity: Arc<Enforcer>,
}

static AUTHZ: tokio::sync::OnceCell<Authz> = tokio::sync::OnceCell::const_new();

impl Authz {
    async fn load() -> Self {
        Self {
            route: Arc::new(route_enforcer().await),
            activity: Arc::new(activity_enforcer().await),
        }
    }

    /// Loads both enforcers on first call (subsequent calls, including from every request's own
    /// `require_session` invocation, just clone the already-loaded `Arc`s — no repeated file
    /// parsing). Safe to call from anywhere; `lanrurugi-server`'s own `main.rs` calls it once
    /// eagerly at startup so a malformed policy file (see [`route_enforcer`]'s own docs on the
    /// `expect` inside it) fails fast at boot rather than on the first request that happens to
    /// need it.
    pub async fn get() -> &'static Authz {
        AUTHZ.get_or_init(Authz::load).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Each test builds its own `Enforcer` (not `Authz::get()`'s shared global) — these are pure
    // in-memory policy checks, no Redis/network involved, so there's no reason to serialize tests
    // through one process-wide instance the way an integration test touching real storage would
    // need to.

    // Every `obj` string below carries the `/api` prefix — `route_policy.csv`'s own top-level
    // comment explains why (`MatchedPath`, read by `require_api_key`, reflects the full post-
    // `.nest("/api", ...)` path, not the bare path each `router()` function's own `.route(...)`
    // calls declare). Omitting the prefix here would make these unit tests pass against a
    // `route_policy.csv` that doesn't actually match what `require_api_key` sees in production —
    // exactly the gap that let a real Admin-role token through `/database/drop` once already
    // this session, only caught by a real end-to-end HTTP test
    // (`lanrurugi-server/tests/auth_flow.rs::session_only_route_rejects_a_real_admin_token_but_accepts_a_real_session`),
    // not by these pure in-memory ones.

    #[tokio::test]
    async fn session_may_call_every_session_only_route() {
        let e = route_enforcer().await;
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/activity",
            "DELETE"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/activity/abc",
            "DELETE"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/activity/retention",
            "PUT"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/database/drop",
            "POST"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/settings/password",
            "POST"
        ));
        assert!(check_route(&e, Some(&session_auth()), "/api/tokens", "GET"));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/tokens",
            "POST"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/tokens/abc",
            "PATCH"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/api/tokens/abc",
            "DELETE"
        ));
    }

    #[tokio::test]
    async fn anonymous_may_reach_only_public_bootstrap_routes() {
        let e = route_enforcer().await;
        let anon = anonymous_auth();
        assert!(check_route(&e, Some(&anon), "/api/login", "POST"));
        assert!(check_route(&e, Some(&anon), "/api/login/status", "GET"));
        assert!(check_route(&e, Some(&anon), "/api/logout", "POST"));
        assert!(check_route(&e, Some(&anon), "/api/token/refresh", "POST"));
        assert!(check_route(&e, Some(&anon), "/api/theme", "GET"));
        assert!(check_route(&e, Some(&anon), "/api/info", "GET"));
        assert!(check_route(&e, Some(&anon), "/api/version", "GET"));
        assert!(!check_route(&e, Some(&anon), "/api/archives", "GET"));
        assert!(!check_route(&e, Some(&anon), "/api/settings", "GET"));
        assert!(!check_route(&e, Some(&anon), "/api/activity", "GET"));
    }

    #[tokio::test]
    async fn admin_token_may_not_call_any_session_only_route() {
        let e = route_enforcer().await;
        let admin = token_auth(TokenRole::Admin);
        assert!(!check_route(&e, Some(&admin), "/api/activity", "DELETE"));
        assert!(!check_route(
            &e,
            Some(&admin),
            "/api/activity/abc",
            "DELETE"
        ));
        assert!(!check_route(
            &e,
            Some(&admin),
            "/api/activity/retention",
            "PUT"
        ));
        assert!(!check_route(&e, Some(&admin), "/api/database/drop", "POST"));
        assert!(!check_route(
            &e,
            Some(&admin),
            "/api/settings/password",
            "POST"
        ));
        assert!(!check_route(&e, Some(&admin), "/api/tokens", "GET"));
        assert!(!check_route(&e, Some(&admin), "/api/tokens", "POST"));
        assert!(!check_route(&e, Some(&admin), "/api/tokens/abc", "PATCH"));
        assert!(!check_route(&e, Some(&admin), "/api/tokens/abc", "DELETE"));
    }

    #[tokio::test]
    async fn admin_token_may_call_every_ordinary_route_and_method() {
        let e = route_enforcer().await;
        let admin = token_auth(TokenRole::Admin);
        assert!(check_route(&e, Some(&admin), "/api/archives", "GET"));
        assert!(check_route(&e, Some(&admin), "/api/archives/abc", "PUT"));
        assert!(check_route(&e, Some(&admin), "/api/archives/abc", "DELETE"));
        assert!(check_route(&e, Some(&admin), "/api/activity", "GET"));
        assert!(check_route(&e, Some(&admin), "/api/activity/facets", "GET"));
    }

    #[tokio::test]
    async fn guest_token_is_read_only_everywhere() {
        let e = route_enforcer().await;
        let guest = token_auth(TokenRole::Guest);
        assert!(check_route(&e, Some(&guest), "/api/archives", "GET"));
        assert!(check_route(&e, Some(&guest), "/api/activity", "GET"));
        assert!(!check_route(&e, Some(&guest), "/api/archives/abc", "PUT"));
        assert!(!check_route(
            &e,
            Some(&guest),
            "/api/archives/abc",
            "DELETE"
        ));
        assert!(!check_route(&e, Some(&guest), "/api/tokens", "GET"));
    }

    /// 007-guest-restricted-access: `guest_visitor` gets an explicit whitelist (route_policy.csv's
    /// own docs on why this replaced the earlier `token_guest`-copied blanket-GET-allow shape),
    /// not a denylist — this test exercises both directions: routes actually on the whitelist
    /// (`/archives/:id/metadata`/`/archives/:id/page`/`/search`, all real requests the frontend's
    /// two guest-reachable routes, Library and Reader, issue) stay reachable, while
    /// `/archives/:id/download` (FR-009's bulk-export exclusion, also absent from the whitelist)
    /// and `/archives` (the bare list-all endpoint — real, but only ever called from
    /// admin-only-gated UI no guest can reach, so deliberately left off the whitelist too) both
    /// stay unreachable, same as every method/route this role was never given an `allow` for at
    /// all.
    #[tokio::test]
    async fn guest_visitor_is_read_only_and_cannot_download_raw_archives() {
        let e = route_enforcer().await;
        let guest = guest_visitor_auth();
        assert!(check_route(
            &e,
            Some(&guest),
            "/api/archives/abc/metadata",
            "GET"
        ));
        assert!(check_route(
            &e,
            Some(&guest),
            "/api/archives/abc/page",
            "GET"
        ));
        assert!(check_route(&e, Some(&guest), "/api/search", "GET"));
        assert!(!check_route(&e, Some(&guest), "/api/archives", "GET"));
        assert!(!check_route(
            &e,
            Some(&guest),
            "/api/archives/abc/download",
            "GET"
        ));
        assert!(!check_route(&e, Some(&guest), "/api/archives/abc", "PUT"));
        assert!(!check_route(
            &e,
            Some(&guest),
            "/api/archives/abc",
            "DELETE"
        ));
        assert!(!check_route(&e, Some(&guest), "/api/tokens", "GET"));
        assert!(!check_route(&e, Some(&guest), "/api/database/drop", "POST"));
    }

    fn session_auth() -> AuthContext {
        AuthContext {
            method: AuthMethod::Session,
            client_ip: None,
        }
    }

    fn token_auth(role: TokenRole) -> AuthContext {
        AuthContext {
            method: AuthMethod::Token {
                id: "tok1".to_string(),
                role,
            },
            client_ip: None,
        }
    }

    fn guest_visitor_auth() -> AuthContext {
        AuthContext {
            method: AuthMethod::GuestVisitor,
            client_ip: None,
        }
    }

    fn anonymous_auth() -> AuthContext {
        AuthContext {
            method: AuthMethod::Anonymous,
            client_ip: None,
        }
    }

    // Activity visibility matrix (issue #91, confirmed with the user): session sees everything;
    // an admin-role token sees its own entries plus every guest-role token's, but not another
    // admin token's or a real session's; a guest-role token sees only its own.

    #[tokio::test]
    async fn session_sees_every_actor_kind() {
        let e = activity_enforcer().await;
        assert!(can_view_activity_entry(
            &e,
            "session",
            None,
            ActorKind::Session,
            None,
            None
        ));
        assert!(can_view_activity_entry(
            &e,
            "session",
            None,
            ActorKind::Token,
            Some("any-token"),
            Some(TokenRole::Admin)
        ));
        assert!(can_view_activity_entry(
            &e,
            "session",
            None,
            ActorKind::Token,
            Some("any-token"),
            Some(TokenRole::Guest)
        ));
        assert!(can_view_activity_entry(
            &e,
            "session",
            None,
            ActorKind::System,
            Some("scanner"),
            None
        ));
    }

    #[tokio::test]
    async fn admin_token_sees_itself_and_every_guest_but_not_session_or_other_admins() {
        let e = activity_enforcer().await;
        // Itself.
        assert!(can_view_activity_entry(
            &e,
            "token_admin",
            Some("admin-1"),
            ActorKind::Token,
            Some("admin-1"),
            Some(TokenRole::Admin)
        ));
        // Any guest.
        assert!(can_view_activity_entry(
            &e,
            "token_admin",
            Some("admin-1"),
            ActorKind::Token,
            Some("guest-1"),
            Some(TokenRole::Guest)
        ));
        // A *different* admin token — must NOT be visible.
        assert!(!can_view_activity_entry(
            &e,
            "token_admin",
            Some("admin-1"),
            ActorKind::Token,
            Some("admin-2"),
            Some(TokenRole::Admin)
        ));
        // A real session's own entry — must NOT be visible.
        assert!(!can_view_activity_entry(
            &e,
            "token_admin",
            Some("admin-1"),
            ActorKind::Session,
            None,
            None
        ));
        // A system-generated entry — must NOT be visible (not "self", not "guest").
        assert!(!can_view_activity_entry(
            &e,
            "token_admin",
            Some("admin-1"),
            ActorKind::System,
            Some("scanner"),
            None
        ));
    }

    #[tokio::test]
    async fn guest_token_sees_only_itself() {
        let e = activity_enforcer().await;
        assert!(can_view_activity_entry(
            &e,
            "token_guest",
            Some("guest-1"),
            ActorKind::Token,
            Some("guest-1"),
            Some(TokenRole::Guest)
        ));
        // A different guest token — must NOT be visible.
        assert!(!can_view_activity_entry(
            &e,
            "token_guest",
            Some("guest-1"),
            ActorKind::Token,
            Some("guest-2"),
            Some(TokenRole::Guest)
        ));
        // An admin token, session, or system entry — none visible to a guest.
        assert!(!can_view_activity_entry(
            &e,
            "token_guest",
            Some("guest-1"),
            ActorKind::Token,
            Some("admin-1"),
            Some(TokenRole::Admin)
        ));
        assert!(!can_view_activity_entry(
            &e,
            "token_guest",
            Some("guest-1"),
            ActorKind::Session,
            None,
            None
        ));
        assert!(!can_view_activity_entry(
            &e,
            "token_guest",
            Some("guest-1"),
            ActorKind::System,
            Some("scanner"),
            None
        ));
    }

    #[tokio::test]
    async fn revoked_token_actor_is_visible_only_to_session() {
        // A `Token`-kind entry whose token id no longer resolves to a live record (revoked) — the
        // caller passes `token_role: None` for it, which `actor_role` maps to the conservative
        // `"token_revoked"` bucket, matching neither `"self"` nor `"guest"` for any requester.
        let e = activity_enforcer().await;
        assert!(can_view_activity_entry(
            &e,
            "session",
            None,
            ActorKind::Token,
            Some("long-gone"),
            None
        ));
        assert!(!can_view_activity_entry(
            &e,
            "token_admin",
            Some("admin-1"),
            ActorKind::Token,
            Some("long-gone"),
            None
        ));
        assert!(!can_view_activity_entry(
            &e,
            "token_guest",
            Some("guest-1"),
            ActorKind::Token,
            Some("long-gone"),
            None
        ));
    }
}
