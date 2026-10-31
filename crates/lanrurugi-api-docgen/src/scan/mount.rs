//! Where a router factory's declared paths actually live.
//!
//! Paths written inside a module's `router()` are relative to wherever that router is mounted, not
//! absolute. Getting this wrong is silent and plausible-looking: publishing the declared paths
//! unprefixed is wrong for the ~236 routes under `/api`, and publishing the two bare routers under
//! `/api` is wrong for the other two.
//!
//! The prefix cannot be derived from the declared paths, so it is declared here — and then asserted
//! against what the source actually does. The assertions are the point: the table is allowed to be
//! explicit precisely because it is checked. Any `.nest()`/`.nest_service()` prefix the table does
//! not already know about fails the build, so a new mount point cannot be introduced without
//! someone noticing.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};
use syn::visit::Visit;
use syn::Expr;

use super::ScanOutput;

/// Router factories served at the root rather than under `/api`.
///
/// Both were verified against `crates/lanrurugi-server/src/app.rs`, which merges them at the top
/// level after nesting everything else under `/api`. They are the only two.
const BARE_FACTORIES: &[&str] = &["health::router", "opensearch::router"];

/// Nest prefixes that are known to mount an API router. `/api` is the only one today.
const ALLOWED_NEST_PREFIXES: &[&str] = &["/api"];

/// Nest prefixes that mount static assets rather than an API router, and therefore contribute no
/// routes. Listed separately from the allowed set so that adding an *API* mount point and adding a
/// static one are visibly different acts.
const STATIC_NEST_PREFIXES: &[&str] = &["/docs"];

pub fn is_bare_factory(factory: &str) -> bool {
    BARE_FACTORIES.contains(&factory)
}

#[derive(Debug, Clone)]
pub struct NestSite {
    pub prefix: String,
    pub call: String,
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Default)]
pub struct MountFacts {
    pub nest_sites: Vec<NestSite>,
    pub merge_refs: BTreeSet<String>,
}

/// The resolved factory → prefix map, plus the facts used to check it.
#[derive(Debug)]
pub struct MountTable {
    prefixes: BTreeMap<String, String>,
    pub unreferenced_factories: Vec<String>,
}

impl MountTable {
    pub fn prefix_for(&self, factory: &str) -> &str {
        self.prefixes
            .get(factory)
            .map(String::as_str)
            .unwrap_or("/api")
    }

    /// Build the table and assert it against the source.
    pub fn resolve(scan: &ScanOutput) -> Result<Self> {
        let mut prefixes = BTreeMap::new();
        for factory in scan.factories.keys() {
            let prefix = if is_bare_factory(factory) { "" } else { "/api" };
            prefixes.insert(factory.clone(), prefix.to_string());
        }

        // Guard: every nesting prefix found anywhere in the scanned sources must be one the table
        // already accounts for. This is what stops a second mount point from being introduced
        // silently — the failure mode the table exists to prevent.
        let mut unknown: Vec<String> = Vec::new();
        for site in &scan.nest_sites {
            let known = ALLOWED_NEST_PREFIXES.contains(&site.prefix.as_str())
                || STATIC_NEST_PREFIXES.contains(&site.prefix.as_str());
            if !known {
                unknown.push(format!(
                    "{}:{}: `{}` mounts `{}`, a prefix this generator does not know about. Add it \
                     to mount.rs (as an API prefix or a static one) so routes under it are \
                     attributed correctly.",
                    site.file, site.line, site.call, site.prefix
                ));
            }
        }
        if !unknown.is_empty() {
            unknown.sort();
            bail!(
                "{} unrecognised mount point(s):\n  - {}",
                unknown.len(),
                unknown.join("\n  - ")
            );
        }

        // Guard: a router factory that is merged but was never discovered means the scanner missed
        // a definition — a macro-generated factory, or a file outside `--api-src`. Its routes would
        // then be absent from the document while the build still passed, which is the one failure
        // this whole feature exists to make impossible.
        let missing: Vec<String> = scan
            .merge_refs
            .iter()
            .filter(|factory| !scan.factories.contains_key(*factory))
            .cloned()
            .collect();
        if !missing.is_empty() {
            bail!(
                "{} router factory/factories are mounted but were not found in the scanned \
                 sources, so their routes cannot be documented:\n  - {}",
                missing.len(),
                missing.join("\n  - ")
            );
        }

        // Reported, not fatal: a factory that nothing merges is either dead code or mounted
        // through a path the scanner cannot see. Both are worth a maintainer's attention, and
        // neither can hide a route from the document.
        let mut unreferenced: Vec<String> = scan
            .factories
            .keys()
            .filter(|factory| !scan.merge_refs.contains(*factory))
            .cloned()
            .collect();
        unreferenced.sort();

        let table = Self {
            prefixes,
            unreferenced_factories: unreferenced,
        };

        // Cross-check the table against what the scan produced: every route's absolute path must
        // actually begin with the prefix the table claims for its factory. This is the assertion
        // that makes an explicit table as trustworthy as deriving the prefix would be — if the two
        // ever disagree, the build says so instead of publishing a wrong path.
        let mut mismatched: Vec<String> = Vec::new();
        for route in &scan.routes {
            let prefix = table.prefix_for(&route.factory);
            let expected = if prefix.is_empty() {
                route.declared_path.clone()
            } else if route.declared_path == "/" {
                prefix.to_string()
            } else {
                format!("{prefix}{}", route.declared_path)
            };
            if expected != route.absolute_path {
                mismatched.push(format!(
                    "{} {} (factory `{}`, declared `{}`) — the mount table says prefix `{prefix}`,                      which would make this `{expected}`",
                    route.method.as_str().to_uppercase(),
                    route.absolute_path,
                    route.factory,
                    route.declared_path
                ));
            }
        }
        if !mismatched.is_empty() {
            mismatched.sort();
            bail!(
                "{} route(s) disagree with the mount table:\n  - {}",
                mismatched.len(),
                mismatched.join("\n  - ")
            );
        }

        Ok(table)
    }
}

/// Collect `.nest()`/`.nest_service()` prefixes and `.merge(..)` factory references from a file.
pub fn collect_mount_facts(parsed: &syn::File, display: &str, facts: &mut MountFacts) {
    let mut visitor = MountVisitor {
        file: display,
        nest_sites: &mut facts.nest_sites,
        merge_refs: &mut facts.merge_refs,
    };
    visitor.visit_file(parsed);
}

struct MountVisitor<'a> {
    file: &'a str,
    nest_sites: &'a mut Vec<NestSite>,
    merge_refs: &'a mut BTreeSet<String>,
}

impl<'ast> Visit<'ast> for MountVisitor<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let method = node.method.to_string();
        if method == "nest" || method == "nest_service" {
            if let Some(Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(prefix),
                ..
            })) = node.args.first().map(unparen)
            {
                self.nest_sites.push(NestSite {
                    prefix: prefix.value(),
                    call: method,
                    file: self.file.to_string(),
                    line: node.method.span().start().line,
                });
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let Expr::Path(path) = &*node.func {
            let segments: Vec<String> = path
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            if let Some(key) = factory_key(&segments) {
                self.merge_refs.insert(key);
            }
        }
        syn::visit::visit_expr_call(self, node);
    }
}

/// Turn a path's segments into a factory key, or `None` when it is not a router factory call.
///
/// `activity::router` -> `activity::router`; `crate::auth_bridge::router` ->
/// `auth_bridge::router`; `lanrurugi_api::login::router` -> `login::router`;
/// `lanrurugi_api::router` -> `router` (the crate root aggregator).
fn factory_key(segments: &[String]) -> Option<String> {
    let last = segments.last()?;
    if last != "router" && last != "public_router" {
        return None;
    }
    if segments.len() < 2 {
        return None;
    }
    let head = segments[..segments.len() - 1].to_vec();
    let head = match head.first().map(String::as_str) {
        Some("crate") | Some("self") | Some("super") | Some("lanrurugi_api") => head[1..].to_vec(),
        _ => head,
    };
    if head.is_empty() {
        // `lanrurugi_api::router` — the crate root's own aggregator, keyed as `router`.
        return Some("router".to_string());
    }
    Some(format!("{}::{}", head.join("::"), last))
}

fn unparen(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => unparen(&p.expr),
        Expr::Group(g) => unparen(&g.expr),
        other => other,
    }
}
