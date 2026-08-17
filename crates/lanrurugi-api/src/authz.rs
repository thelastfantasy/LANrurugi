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

/// The three fixed roles [`AuthContext`] ever resolves to — a `&'static str` (not a `TokenRole`
/// re-export) since Casbin's `enforce` takes plain strings and `Session`/`Anonymous` have no
/// `TokenRole` counterpart to reuse in the first place.
fn subject_role(auth: Option<&AuthContext>) -> &'static str {
    match auth.map(|a| &a.method) {
        None => "anonymous",
        Some(AuthMethod::Session) => "session",
        Some(AuthMethod::Token {
            role: TokenRole::Admin,
            ..
        }) => "token_admin",
        Some(AuthMethod::Token {
            role: TokenRole::Guest,
            ..
        }) => "token_guest",
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

/// Route-level check — replaces the four separate `.route_layer(require_session)` mounts and
/// `settings.rs::put_settings`'s own inline `is_token()` check with one call each handler makes
/// against the shared policy file instead of hand-rolling its own condition. `obj` is the route's
/// own declared path pattern exactly as written in its `Router::route(...)` call (e.g.
/// `"/activity/{id}"` — Axum's own `{id}` capture syntax, translated to Casbin's `:id` `keyMatch2`
/// syntax by [`axum_path_to_casbin`] before this is called), not the concrete request URI (which
/// would need every numeric/hash id in the policy file itself).
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

    #[tokio::test]
    async fn session_may_call_every_session_only_route() {
        let e = route_enforcer().await;
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/activity",
            "DELETE"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/activity/abc",
            "DELETE"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/activity/retention",
            "PUT"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/database/drop",
            "POST"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/settings/password",
            "POST"
        ));
        assert!(check_route(&e, Some(&session_auth()), "/tokens", "GET"));
        assert!(check_route(&e, Some(&session_auth()), "/tokens", "POST"));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/tokens/abc",
            "PATCH"
        ));
        assert!(check_route(
            &e,
            Some(&session_auth()),
            "/tokens/abc",
            "DELETE"
        ));
    }

    #[tokio::test]
    async fn admin_token_may_not_call_any_session_only_route() {
        let e = route_enforcer().await;
        let admin = token_auth(TokenRole::Admin);
        assert!(!check_route(&e, Some(&admin), "/activity", "DELETE"));
        assert!(!check_route(&e, Some(&admin), "/activity/abc", "DELETE"));
        assert!(!check_route(&e, Some(&admin), "/activity/retention", "PUT"));
        assert!(!check_route(&e, Some(&admin), "/database/drop", "POST"));
        assert!(!check_route(&e, Some(&admin), "/settings/password", "POST"));
        assert!(!check_route(&e, Some(&admin), "/tokens", "GET"));
        assert!(!check_route(&e, Some(&admin), "/tokens", "POST"));
        assert!(!check_route(&e, Some(&admin), "/tokens/abc", "PATCH"));
        assert!(!check_route(&e, Some(&admin), "/tokens/abc", "DELETE"));
    }

    #[tokio::test]
    async fn admin_token_may_call_every_ordinary_route_and_method() {
        let e = route_enforcer().await;
        let admin = token_auth(TokenRole::Admin);
        assert!(check_route(&e, Some(&admin), "/archives", "GET"));
        assert!(check_route(&e, Some(&admin), "/archives/abc", "PUT"));
        assert!(check_route(&e, Some(&admin), "/archives/abc", "DELETE"));
        assert!(check_route(&e, Some(&admin), "/activity", "GET"));
        assert!(check_route(&e, Some(&admin), "/activity/facets", "GET"));
    }

    #[tokio::test]
    async fn guest_token_is_read_only_everywhere() {
        let e = route_enforcer().await;
        let guest = token_auth(TokenRole::Guest);
        assert!(check_route(&e, Some(&guest), "/archives", "GET"));
        assert!(check_route(&e, Some(&guest), "/activity", "GET"));
        assert!(!check_route(&e, Some(&guest), "/archives/abc", "PUT"));
        assert!(!check_route(&e, Some(&guest), "/archives/abc", "DELETE"));
        assert!(!check_route(&e, Some(&guest), "/tokens", "GET"));
    }

    #[tokio::test]
    async fn anonymous_open_instance_may_call_ordinary_routes_but_not_session_only_ones() {
        // `require_api_key` only ever short-circuits (no `AuthContext` inserted at all) when
        // `enable_pass=false` — the four session-only routes still individually gate through
        // `require_session`/`check_route` in that case, `auth: None`.
        let e = route_enforcer().await;
        assert!(check_route(&e, None, "/archives", "GET"));
        assert!(!check_route(&e, None, "/database/drop", "POST"));
        assert!(!check_route(&e, None, "/tokens", "GET"));
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
