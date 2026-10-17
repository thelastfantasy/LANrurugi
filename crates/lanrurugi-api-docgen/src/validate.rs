//! The gates.
//!
//! These run on every invocation, including `--check`, and each one either passes or aborts with a
//! message naming the offending route, file and line. A gate never "skips" a problem and never
//! downgrades one to a warning.
//!
//! The completeness gate deserves a note: because the route scan is applied last as the
//! authoritative layer, a route cannot be missing from the assembled document. The assertion is
//! kept anyway, because the alternative to checking an invariant that "cannot" be violated is
//! finding out years later that it could.

use std::collections::BTreeSet;

use anyhow::{bail, Result};
use serde_json::Value;

use crate::merge::{Coverage, Merged};
use crate::policy::{Policy, Role};

/// See `emit::AUTHENTICATED_PREFIX` — the same boundary, from the gate's side.
const AUTHENTICATED_PREFIX: &str = "/api";

pub fn run(merged: &Merged, policy: &Policy) -> Result<()> {
    let mut failures: Vec<String> = Vec::new();

    let coverage = &merged.coverage;
    if coverage.routes_described != coverage.routes_registered {
        failures.push(format!(
            "completeness: {} route(s) are registered but missing from the document, and {} are \
             documented without being registered. The merge layers cannot remove a route, so this \
             means a bug in the generator itself.",
            coverage
                .routes_registered
                .saturating_sub(coverage.routes_described),
            coverage
                .routes_described
                .saturating_sub(coverage.routes_registered)
        ));
    }

    let mut operation_ids: BTreeSet<String> = BTreeSet::new();
    for ((path, method), operation) in &merged.operations {
        let described = format!("{} {}", method.as_str().to_uppercase(), path);

        if !path.starts_with('/') {
            failures.push(format!(
                "integrity: `{path}` is not an absolute path; OpenAPI path keys must begin with `/`."
            ));
        }

        let Some(object) = operation.value.as_object() else {
            failures.push(format!(
                "integrity: {described} is not an operation object."
            ));
            continue;
        };

        let has_responses = object
            .get("responses")
            .and_then(Value::as_object)
            .is_some_and(|r| !r.is_empty());
        if !has_responses {
            failures.push(format!(
                "integrity: {described} has no responses; an operation with no responses is invalid \
                 OpenAPI."
            ));
        }

        match object.get("operationId").and_then(Value::as_str) {
            Some(id) if !id.is_empty() => {
                if !operation_ids.insert(id.to_string()) {
                    failures.push(format!(
                        "integrity: operationId `{id}` (on {described}) is used by more than one \
                         operation."
                    ));
                }
            }
            _ => failures.push(format!("integrity: {described} has no operationId.")),
        }
    }

    // Policy coverage.
    //
    // Deliberately *not* "every route must be mentioned by some rule". The policy is fail-open for
    // the authenticated roles — `p, session, /api/*, *, allow` plus a per-method wildcard for each
    // token role means a new `/api` route is reachable by default, and 174 of the 229 matched routes
    // rely on that wildcard alone. A "was it mentioned" gate therefore can never fire, which is
    // worse than no gate: it reads as protection while providing none. (Verified by injecting an
    // uncovered route: the document accepted it.)
    //
    // What a new route *can* silently break is the one invariant that is not default-allow: a guest
    // token is read-only. `token_guest` is granted `GET` by wildcard and every mutating route denies
    // it explicitly, so forgetting one deny hands a guest token a write it should never have. That
    // is checkable, true today (0 violations), and the actual property the deny list maintains.
    const MUTATING: [&str; 4] = ["POST", "PUT", "PATCH", "DELETE"];
    for (path, method) in merged.operations.keys() {
        if !path.starts_with(AUTHENTICATED_PREFIX) {
            continue;
        }
        let action = method.as_str().to_uppercase();

        // Applies to every route, not only the mutating ones: a route nothing matches is dead, and
        // a route every matching rule denies is dead too. Both are silent in production without
        // this — the first answers 403 to the admin token as well.
        match policy.access(method.as_str(), path) {
            None => failures.push(format!(
                "policy: {action} {path} is served but no rule matches it at all, so no credential \
                 class can reach it. Add a rule to `crates/lanrurugi-api/policy/route_policy.csv`."
            )),
            Some(access) if access.is_unreachable() => failures.push(format!(
                "policy: {action} {path} is served, and every rule that matches it denies. Either an \
                 intended deny-all (rare) or a pattern that does not match what it was meant to."
            )),
            Some(_) => {}
        }

        if MUTATING.contains(&action.as_str())
            && policy.allows(Role::TokenGuest, method.as_str(), path)
        {
            failures.push(format!(
                "policy: {action} {path} is reachable by a guest token. A guest token is read-only \
                 (`token_guest` is granted `GET` and every mutating route denies it explicitly), so \
                 add `p, token_guest, {path}, {action}, deny` — or, if a guest token genuinely should \
                 be able to call this, record that decision on purpose."
            ));
        }
    }

    // The reverse direction of the reachability gate above: a rule that matches no served route
    // governs nothing, which is what a rename or a removal leaves behind. Silent on the server, so
    // this is the only place it can surface.
    let served: Vec<(String, String)> = merged
        .operations
        .keys()
        .map(|(path, method)| (method.as_str().to_string(), path.to_string()))
        .collect();
    for (line, rule) in policy.unused_rules(&served) {
        failures.push(format!(
            "policy: line {line} of `route_policy.csv` (`{rule}`) matches no served route. A route \
             renamed or removed in the source must take its rules with it."
        ));
    }

    if !failures.is_empty() {
        failures.sort();
        bail!(
            "{} gate failure(s):\n  - {}",
            failures.len(),
            failures.join("\n  - ")
        );
    }
    Ok(())
}

/// Print the coverage report. Called on every non-quiet run, including successful ones — a gate
/// whose result nobody can see is not much of a gate.
pub fn report(
    merged: &Merged,
    policy: &Policy,
    legacy_prefix: &str,
    sidecar_files: usize,
    access_note_routes: usize,
    quiet: bool,
) {
    if quiet {
        return;
    }
    let c: &Coverage = &merged.coverage;
    println!("API reference coverage");
    println!("  routes registered:     {}", c.routes_registered);
    println!("  routes described:      {}", c.routes_described);
    for source in ["legacy", "sidecar", "none"] {
        let count = c.by_source.get(source).copied().unwrap_or(0);
        let label = match source {
            "legacy" => "from the vendored upstream document",
            "sidecar" => "hand-written in docs/api/sidecar",
            _ => "listed without schema detail (the backlog)",
        };
        println!("  {source:<8} {count:>5}  {label}");
    }
    println!(
        "  upstream paths not served here: {} (prefix `{legacy_prefix}`)",
        c.upstream_only_paths.len()
    );
    for path in &c.upstream_only_paths {
        println!("      {path}");
    }
    if !c.upstream_only_operations.is_empty() {
        println!(
            "  upstream operations not served here: {}",
            c.upstream_only_operations.len()
        );
        for op in &c.upstream_only_operations {
            println!("      {op}");
        }
    }
    if !c.unreferenced_factories.is_empty() {
        println!(
            "  router factories nothing merges: {:?}",
            c.unreferenced_factories
        );
    }
    // How the document now describes authentication, counted rather than asserted — the point of
    // deriving it from the policy is that these numbers are checkable rather than claimed.
    let (mut anonymous, mut session_only, mut token_only, mut either, mut outside) =
        (0, 0, 0, 0, 0);
    // Counted separately from `either`: "a token works" and "which token works" are different
    // questions, and the document now answers both.
    let mut admin_token_only = 0;
    for (path, method) in merged.operations.keys() {
        if !path.starts_with(AUTHENTICATED_PREFIX) {
            outside += 1;
            continue;
        }
        if policy
            .access(method.as_str(), path)
            .is_some_and(|access| access.admin_token_only())
        {
            admin_token_only += 1;
        }
        match policy.access(method.as_str(), path) {
            Some(access) if access.anonymous => anonymous += 1,
            Some(access) if access.is_session_only() => session_only += 1,
            Some(access) if access.is_token_only() => token_only += 1,
            Some(_) => either += 1,
            None => {}
        }
    }
    println!("  authentication declared:");
    println!("      either credential  {either:>5}");
    println!("      session only       {session_only:>5}");
    println!("      bearer only        {token_only:>5}");
    println!("      no credentials     {anonymous:>5}");
    println!("      outside /api       {outside:>5}");
    println!("      administrator token only {admin_token_only:>5}");
    println!("  sidecar fragments read: {sidecar_files}");
    println!("  routes with access notes: {access_note_routes}");
    println!(
        "  document: {} path(s), {} operation(s)",
        distinct_paths(merged),
        merged.operations.len()
    );
}

fn distinct_paths(merged: &Merged) -> usize {
    merged
        .operations
        .keys()
        .map(|(path, _)| path.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}
