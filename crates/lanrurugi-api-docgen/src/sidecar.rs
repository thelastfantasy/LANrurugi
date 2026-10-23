//! Hand-written OpenAPI fragments: the layer a maintainer edits.
//!
//! This is where field-level detail goes for endpoints the upstream document does not cover, and
//! where a deliberate divergence from an endpoint it does cover is recorded. Fragments use the real
//! OpenAPI path-item shape rather than an invented flat key structure, so they validate against the
//! same tooling as any OpenAPI document and can carry a full schema later without a format change.
//!
//! Every violation is a hard failure. A fragment describing an endpoint that does not exist is the
//! precise rot this feature exists to prevent; silently ignoring it would recreate the problem in
//! a new form.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

use crate::scan::{HttpMethod, RouteRecord};

const NON_OPERATION_KEYS: &[&str] = &["$ref", "summary", "description", "servers", "parameters"];

#[derive(Debug, Default)]
pub struct SidecarLayer {
    pub operations: BTreeMap<(String, HttpMethod), Value>,
    pub schemas: Map<String, Value>,
    pub files_read: usize,
}

pub fn load(dir: &Path, routes: &[RouteRecord]) -> Result<SidecarLayer> {
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_yaml(dir, &mut files)?;
        files.sort();
    }

    let route_set: BTreeSet<(String, HttpMethod)> = routes
        .iter()
        .map(|r| (r.absolute_path.clone(), r.method))
        .collect();

    let mut layer = SidecarLayer::default();
    let mut problems: Vec<String> = Vec::new();

    for file in &files {
        let display = file.display().to_string();
        let text = std::fs::read_to_string(file).with_context(|| format!("reading {display}"))?;
        let value: Value =
            serde_saphyr::from_str(&text).with_context(|| format!("parsing {display}"))?;
        let Some(map) = value.as_object() else {
            bail!("{display}: a sidecar fragment must be a mapping of paths (and optionally `components`)");
        };
        layer.files_read += 1;

        for (key, item) in map {
            if key == "components" {
                merge_schemas(&mut layer.schemas, item, &display, &mut problems);
                continue;
            }
            if !key.starts_with('/') {
                problems.push(format!(
                    "{display}: `{key}` is not a path key — fragments hold absolute paths (with the \
                     `/api` prefix) plus an optional `components` key."
                ));
                continue;
            }
            let Some(operations) = item.as_object() else {
                problems.push(format!("{display}: `{key}` is not a path item object."));
                continue;
            };
            for (method_key, operation) in operations {
                if NON_OPERATION_KEYS.contains(&method_key.as_str()) {
                    continue;
                }
                let Some(method) = HttpMethod::parse(method_key) else {
                    problems.push(format!(
                        "{display}: `{key}` has method key `{method_key}`, which is not one of \
                         get/post/put/delete/patch/head/options."
                    ));
                    continue;
                };
                if !route_set.contains(&(key.clone(), method)) {
                    problems.push(format!(
                        "{display}: `{} {}` does not match any route the server registers. Either \
                         the route was removed or renamed (update or delete this entry), or the \
                         path is missing its `/api` prefix.",
                        method.as_str().to_uppercase(),
                        key
                    ));
                    continue;
                }
                if let Some(previous) = layer
                    .operations
                    .insert((key.clone(), method), operation.clone())
                {
                    let _ = previous;
                    problems.push(format!(
                        "{display}: `{} {}` is declared by more than one fragment; ambiguity \
                         resolved by file order is a bug waiting to happen.",
                        method.as_str().to_uppercase(),
                        key
                    ));
                }
            }
        }
    }

    if !problems.is_empty() {
        problems.sort();
        problems.dedup();
        bail!(
            "{} problem(s) in the hand-written sidecar:\n  - {}",
            problems.len(),
            problems.join("\n  - ")
        );
    }

    Ok(layer)
}

fn merge_schemas(
    into: &mut Map<String, Value>,
    components: &Value,
    display: &str,
    problems: &mut Vec<String>,
) {
    let Some(schemas) = components.get("schemas").and_then(Value::as_object) else {
        return;
    };
    for (name, schema) in schemas {
        if into.contains_key(name) {
            problems.push(format!(
                "{display}: schema `{name}` is defined by more than one fragment."
            ));
            continue;
        }
        into.insert(name.clone(), schema.clone());
    }
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_yaml(&path, out)?;
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("yaml") | Some("yml")
        ) {
            out.push(path);
        }
    }
    Ok(())
}
