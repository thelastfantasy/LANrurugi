//! Route-table recovery from the server's own source.
//!
//! axum's `Router` cannot be asked what was registered — 0.8.9 exposes `has_routes()` and nothing
//! else — so the route table is recovered by parsing the crate's source with `syn`. That is a
//! deliberate choice over a text scan: the naive version was tried first and produced phantom
//! endpoints from `.route(` appearing inside ordinary comments.
//!
//! Two exclusions are load-bearing rather than defensive. Only `router`/`public_router` factory
//! functions are scanned, and `#[cfg(test)]` modules / `#[test]`-annotated functions are skipped.
//! Several files build throwaway `Router`s inside their test modules, some with paths computed at
//! runtime (`&format!("/{filename}")`) — a scanner without those exclusions would either publish
//! nonsense routes or refuse to run on the repository as it stands.

pub mod handlers;
pub mod mount;
pub mod routes;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use mount::MountTable;

/// The HTTP verbs the route scanner understands. Kept as an enum rather than a string so an
/// unrecognised verb cannot reach the emitted document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
    Head,
    Options,
}

impl HttpMethod {
    /// Lowercase, matching OpenAPI's operation keys.
    pub fn as_str(self) -> &'static str {
        match self {
            HttpMethod::Get => "get",
            HttpMethod::Post => "post",
            HttpMethod::Put => "put",
            HttpMethod::Delete => "delete",
            HttpMethod::Patch => "patch",
            HttpMethod::Head => "head",
            HttpMethod::Options => "options",
        }
    }

    /// Parse an OpenAPI operation key. `any` is deliberately absent: it registers every method and
    /// the scanner refuses to model that rather than guess.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "get" => Some(HttpMethod::Get),
            "post" => Some(HttpMethod::Post),
            "put" => Some(HttpMethod::Put),
            "delete" => Some(HttpMethod::Delete),
            "patch" => Some(HttpMethod::Patch),
            "head" => Some(HttpMethod::Head),
            "options" => Some(HttpMethod::Options),
            _ => None,
        }
    }
}

/// One HTTP method on one absolute path — the unit of completeness for the whole feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteRecord {
    pub method: HttpMethod,
    pub absolute_path: String,
    pub declared_path: String,
    pub factory: String,
    pub handler: Option<String>,
    pub module: String,
    pub file: String,
    pub line: usize,
}

/// A free function found while scanning, with its own doc comment. Handler documentation is taken
/// from here rather than being re-written: the comment is text a maintainer already maintains, so
/// surfacing it cannot drift the way a copy would.
#[derive(Debug, Clone)]
pub struct FnInfo {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub doc: Option<String>,
}

#[derive(Debug, Default)]
pub struct ScanOutput {
    pub routes: Vec<RouteRecord>,
    pub fns: Vec<FnInfo>,
    /// factory key -> repository-relative file it was found in.
    pub factories: BTreeMap<String, String>,
    /// Every `.nest()`/`.nest_service()` prefix seen in the scanned sources, with where.
    pub nest_sites: Vec<mount::NestSite>,
    /// Every router factory referenced by a `.merge(..)`, wherever it lives.
    pub merge_refs: BTreeSet<String>,
}

impl ScanOutput {
    pub fn fn_lookup(&self) -> handlers::FnLookup<'_> {
        handlers::FnLookup::new(&self.fns)
    }
}

/// Turn a path relative to the crate's `src/` into its module path.
///
/// `activity.rs` -> `activity`; `plugin_wizard/mod.rs` -> `plugin_wizard`;
/// `subscriptions/api.rs` -> `subscriptions::api`; `lib.rs` -> `""`.
fn module_path_for(rel: &Path) -> String {
    let mut parts: Vec<String> = rel
        .parent()
        .map(|p| {
            p.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    let stem = rel
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if stem != "mod" && stem != "lib" && stem != "main" {
        parts.push(stem);
    }
    parts.join("::")
}

fn is_router_factory(name: &str) -> bool {
    name == "router" || name == "public_router"
}

fn attrs_say_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let mut found = false;
        // `#[cfg(test)]` parses as a `Meta::List`; anything unparseable is simply not a match.
        let _ = a.parse_nested_meta(|meta| {
            if meta.path.is_ident("test") {
                found = true;
            }
            // Consume any value the meta might carry so parsing does not stop early.
            if meta.input.peek(syn::Token![=]) {
                let _: syn::Expr = meta.value()?.parse()?;
            }
            Ok(())
        });
        found
    })
}

fn attrs_say_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("test") || a.path().segments.last().is_some_and(|s| s.ident == "test")
    })
}

struct Walker<'a> {
    out: &'a mut ScanOutput,
    errors: Vec<String>,
}

impl Walker<'_> {
    fn walk_items(&mut self, items: &[syn::Item], module: &str, in_test: bool, rel: &Path) {
        for item in items {
            match item {
                syn::Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        let nested_test = in_test || attrs_say_cfg_test(&m.attrs);
                        let nested_module = if module.is_empty() {
                            m.ident.to_string()
                        } else {
                            format!("{module}::{}", m.ident)
                        };
                        self.walk_items(inner, &nested_module, nested_test, rel);
                    }
                }
                syn::Item::Fn(f) => {
                    if in_test || attrs_say_test(&f.attrs) {
                        continue;
                    }
                    let name = f.sig.ident.to_string();
                    let line = f.sig.ident.span().start().line;
                    self.out.fns.push(FnInfo {
                        name: name.clone(),
                        file: rel.display().to_string(),
                        line,
                        doc: handlers::doc_comment(&f.attrs),
                    });
                    if is_router_factory(&name) {
                        let factory = if module.is_empty() {
                            name.clone()
                        } else {
                            format!("{module}::{name}")
                        };
                        self.out
                            .factories
                            .insert(factory.clone(), rel.display().to_string());
                        routes::collect_routes(
                            &f.block,
                            &factory,
                            module,
                            rel,
                            &mut self.out.routes,
                            &mut self.errors,
                        );
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse every `.rs` file under `api_src` and recover the route table.
pub fn scan(api_src: &Path, extra_sources: &[PathBuf], repo_root: &Path) -> Result<ScanOutput> {
    if !api_src.is_dir() {
        bail!(
            "api source directory not found: {} (run the generator from the repository root, or pass --api-src)",
            api_src.display()
        );
    }

    let mut files = Vec::new();
    collect_rs_files(api_src, &mut files)?;
    files.sort();

    let mut out = ScanOutput::default();
    let mut errors = Vec::new();

    for path in &files {
        let rel = path
            .strip_prefix(api_src)
            .with_context(|| format!("path outside --api-src: {}", path.display()))?;
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let parsed = syn::parse_file(&text)
            .with_context(|| format!("parsing {} as Rust source", path.display()))?;

        let module = module_path_for(rel);
        let display = display_path(path, repo_root);
        let mut walker = Walker {
            out: &mut out,
            errors: Vec::new(),
        };
        walker.walk_items(&parsed.items, &module, false, Path::new(&display));
        errors.extend(walker.errors);

        let mut facts = mount::MountFacts::default();
        mount::collect_mount_facts(&parsed, &display, &mut facts);
        out.nest_sites.extend(facts.nest_sites);
        out.merge_refs.extend(facts.merge_refs);
    }

    // Mount points: `.nest()` in sibling crates (the server assembly) can also introduce a prefix,
    // so those files get the same nest guard applied even though they contain no route factories.
    for path in extra_sources {
        if !path.exists() {
            bail!("source file not found: {}", path.display());
        }
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let parsed = syn::parse_file(&text)
            .with_context(|| format!("parsing {} as Rust source", path.display()))?;
        let display = display_path(path, repo_root);
        let mut facts = mount::MountFacts::default();
        mount::collect_mount_facts(&parsed, &display, &mut facts);
        out.nest_sites.extend(facts.nest_sites);
        out.merge_refs.extend(facts.merge_refs);
    }

    if !errors.is_empty() {
        errors.sort();
        errors.dedup();
        bail!(
            "{} route registration(s) could not be resolved:\n  - {}",
            errors.len(),
            errors.join("\n  - ")
        );
    }

    out.routes.sort_by(|a, b| {
        a.absolute_path
            .cmp(&b.absolute_path)
            .then(a.method.cmp(&b.method))
    });

    let mut seen: BTreeMap<(String, HttpMethod), &RouteRecord> = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    for r in &out.routes {
        let key = (r.absolute_path.clone(), r.method);
        if seen.insert(key.clone(), r).is_some() {
            duplicates.insert(key);
        }
    }
    if !duplicates.is_empty() {
        let list: Vec<String> = duplicates
            .iter()
            .map(|(p, m)| format!("{} {}", m.as_str().to_uppercase(), p))
            .collect();
        bail!(
            "two registrations claim the same method and path (one of them is unreachable):\n  - {}",
            list.join("\n  - ")
        );
    }

    Ok(out)
}

fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_rs_files(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Repository-relative path, so no emitted artefact can carry a machine-specific absolute path.
pub fn display_path(path: &Path, repo_root: &Path) -> String {
    path.strip_prefix(repo_root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Resolve the mount prefix for every discovered router factory, asserting the explicit table
/// against what the source actually does.
pub fn resolve_mounts(out: &ScanOutput) -> Result<MountTable> {
    MountTable::resolve(out)
}
