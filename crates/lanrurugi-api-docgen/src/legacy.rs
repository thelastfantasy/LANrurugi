//! The vendored upstream LANraragi OpenAPI document, used as the base description layer.
//!
//! This is the layer that makes compatibility-surface documentation possible at all. The upstream
//! document is OpenAPI 3.1 — the same dialect this generator emits — and, at the revision vendored
//! for this feature, carries 64 paths / 87 operations with a JSON response schema on 84 of them,
//! plus parameters, constraints and prose. None of that is re-written here; it is reused.
//!
//! Two rewrites are applied on read, and only two:
//!
//! - **Path prefix.** Upstream declares paths relative to its own `servers[0].url`, which ends in
//!   `/api`. The prefix is read out of the document rather than hardcoded, so a future upstream
//!   change to that URL is a visible diff rather than a silent mis-prefixing.
//! - **Auth.** Upstream's `securitySchemes.api_key` and its per-operation `security` entries
//!   describe a scheme this server deliberately no longer uses. They are dropped rather than
//!   inherited.
//!
//! The document is vendored rather than fetched so that generation is offline and reproducible: an
//! upstream edit must not silently change what this project publishes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

use crate::scan::{HttpMethod, RouteRecord};

/// Keys that may appear in an OpenAPI path item alongside the operation verbs.
const NON_OPERATION_PATH_KEYS: &[&str] =
    &["$ref", "summary", "description", "servers", "parameters"];

#[derive(Debug, Default)]
pub struct LegacyLayer {
    pub operations: BTreeMap<(String, HttpMethod), Value>,
    pub schemas: Map<String, Value>,
    /// Upstream's own tag descriptions, by tag name. Carried so a tag this server also uses gets
    /// the upstream prose rather than appearing bare — but only tags an operation actually uses are
    /// emitted (see `merge`), since upstream declares tags for paths this server does not serve.
    pub tag_descriptions: BTreeMap<String, String>,
    /// Upstream paths this server does not register at all.
    pub upstream_only_paths: Vec<String>,
    /// Upstream operations whose path exists here but whose method does not.
    pub upstream_only_operations: Vec<String>,
    /// The prefix the upstream document's own `servers` entry implies.
    pub prefix: String,
}

pub fn load(path: &Path, routes: &[RouteRecord]) -> Result<LegacyLayer> {
    if !path.exists() {
        bail!(
            "vendored upstream document not found: {} (run `mise run docs-vendor-legacy` to fetch \
             it, or pass --legacy)",
            path.display()
        );
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let document: Value = serde_saphyr::from_str(&text)
        .with_context(|| format!("parsing {} as YAML", path.display()))?;

    let openapi = document
        .get("openapi")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !openapi.starts_with("3.1") {
        bail!(
            "{} declares `openapi: {openapi}`; this generator emits OpenAPI 3.1 and will not \
             translate a different dialect implicitly.",
            path.display()
        );
    }

    let prefix = prefix_from_servers(&document);
    let mut route_set: BTreeSet<(String, HttpMethod)> = BTreeSet::new();
    let mut route_paths: BTreeSet<String> = BTreeSet::new();
    for route in routes {
        route_set.insert((route.absolute_path.clone(), route.method));
        route_paths.insert(route.absolute_path.clone());
    }

    let mut layer = LegacyLayer {
        prefix: prefix.clone(),
        ..Default::default()
    };

    let Some(paths) = document.get("paths").and_then(Value::as_object) else {
        bail!("{} has no `paths` object", path.display());
    };

    for (declared, item) in paths {
        let absolute = join_path(&prefix, declared);
        let Some(operations) = item.as_object() else {
            continue;
        };

        for (key, operation) in operations {
            if NON_OPERATION_PATH_KEYS.contains(&key.as_str()) {
                continue;
            }
            let Some(method) = HttpMethod::parse(key) else {
                // `trace` and friends are not modelled; skipping is correct as long as it cannot
                // hide a route this server serves, and the route set is authoritative for that.
                continue;
            };
            if !route_set.contains(&(absolute.clone(), method)) {
                continue;
            }
            let mut operation = operation.clone();
            if let Some(map) = operation.as_object_mut() {
                map.remove("security");
                map.remove("x-mojo-to");
            }
            layer
                .operations
                .insert((absolute.clone(), method), operation);
        }

        if route_paths.contains(&absolute) {
            // The path exists here; report only the *methods* upstream documents that we do not.
            for key in operations.keys() {
                if NON_OPERATION_PATH_KEYS.contains(&key.as_str()) {
                    continue;
                }
                let Some(method) = HttpMethod::parse(key) else {
                    continue;
                };
                if !route_set.contains(&(absolute.clone(), method)) {
                    layer.upstream_only_operations.push(format!(
                        "{} {}",
                        method.as_str().to_uppercase(),
                        absolute
                    ));
                }
            }
        } else {
            layer.upstream_only_paths.push(absolute);
        }
    }

    layer.upstream_only_paths.sort();
    layer.upstream_only_operations.sort();

    if let Some(tags) = document.get("tags").and_then(Value::as_array) {
        for tag in tags {
            let Some(name) = tag.get("name").and_then(Value::as_str) else {
                continue;
            };
            if let Some(description) = tag.get("description").and_then(Value::as_str) {
                layer
                    .tag_descriptions
                    .insert(name.to_string(), description.to_string());
            }
        }
    }

    if let Some(schemas) = document
        .pointer("/components/schemas")
        .and_then(Value::as_object)
    {
        layer.schemas = schemas.clone();
    }

    Ok(layer)
}

/// Read the path component of the first `servers` entry, which is what upstream's declared paths
/// are relative to. Absent or relative-to-root means no prefix.
fn prefix_from_servers(document: &Value) -> String {
    let url = document
        .get("servers")
        .and_then(Value::as_array)
        .and_then(|s| s.first())
        .and_then(|s| s.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let after_scheme = match url.split_once("://") {
        Some((_, rest)) => rest,
        None => url,
    };
    let path = match after_scheme.find('/') {
        Some(idx) => &after_scheme[idx..],
        None => "",
    };
    path.trim_end_matches('/').to_string()
}

fn join_path(prefix: &str, declared: &str) -> String {
    if prefix.is_empty() {
        declared.to_string()
    } else if declared == "/" {
        prefix.to_string()
    } else {
        format!("{prefix}{declared}")
    }
}
