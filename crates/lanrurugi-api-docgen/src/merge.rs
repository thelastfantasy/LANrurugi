//! Merging the three description sources into one document.
//!
//! Precedence is sidecar → upstream → auto-derived, and the route scan is applied *last* as the
//! completeness authority. That ordering is the design: a layer that can only add or enrich an
//! entry can never remove a route from the output, so "the reference lists something the server
//! does not serve" and "the server serves something the reference does not list" are both
//! structurally impossible rather than merely checked.
//!
//! Replacement is whole-operation, never field-by-field. Merging two documents field by field
//! produces a result nobody can predict from reading either one; whole-operation replacement is a
//! rule a maintainer can hold in their head.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

use crate::legacy::LegacyLayer;
use crate::scan::handlers::{self, FnLookup};
use crate::scan::{HttpMethod, RouteRecord};
use crate::sidecar::SidecarLayer;

/// Which layer supplied an operation's request/response detail. Reported per operation so a reader
/// can tell a fully described endpoint from a listed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaSource {
    Legacy,
    Sidecar,
    None,
}

impl SchemaSource {
    pub fn as_str(self) -> &'static str {
        match self {
            SchemaSource::Legacy => "legacy",
            SchemaSource::Sidecar => "sidecar",
            SchemaSource::None => "none",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MergedOperation {
    pub value: Value,
    pub source: SchemaSource,
}

#[derive(Debug, Default)]
pub struct Merged {
    /// Keyed by (absolute path, method). A `BTreeMap` rather than a `HashMap` because the emitted
    /// document's ordering is part of its contract: two runs over unchanged sources must be
    /// byte-identical, or comparing a published artefact against a locally generated one is
    /// meaningless.
    pub operations: BTreeMap<(String, HttpMethod), MergedOperation>,
    pub schemas: Map<String, Value>,
    /// Only the tags an operation actually carries. Upstream declares tags for paths this server
    /// does not serve, and emitting those would render empty groups in the reference.
    pub tags: BTreeSet<String>,
    /// Upstream's prose for a tag, where it has any.
    pub tag_descriptions: BTreeMap<String, String>,
    pub coverage: Coverage,
}

#[derive(Debug, Default)]
pub struct Coverage {
    pub routes_registered: usize,
    pub routes_described: usize,
    pub by_source: BTreeMap<&'static str, usize>,
    /// Upstream paths this server does not register at all.
    pub upstream_only_paths: Vec<String>,
    /// Upstream operations on a path this server *does* register, but under a method it does not.
    pub upstream_only_operations: Vec<String>,
    /// Router factories nothing merges — dead code, or a mount the scanner cannot see.
    pub unreferenced_factories: Vec<String>,
}

pub fn merge(
    routes: &[RouteRecord],
    legacy: LegacyLayer,
    sidecar: SidecarLayer,
    fns: &FnLookup<'_>,
) -> Result<Merged> {
    let mut merged = Merged {
        schemas: legacy.schemas.clone(),
        ..Default::default()
    };
    merged.tag_descriptions.extend(
        legacy
            .tag_descriptions
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    for (name, schema) in &sidecar.schemas {
        if merged.schemas.contains_key(name) {
            bail!(
                "sidecar defines schema `{name}`, which the vendored upstream document already \
                 defines. Use a distinct name and reference it explicitly, so the override is \
                 visible in review."
            );
        }
        merged.schemas.insert(name.clone(), schema.clone());
    }

    let legacy_ops = legacy.operations;
    let sidecar_ops = sidecar.operations;

    let mut taken_operation_ids: BTreeSet<String> = BTreeSet::new();

    for route in routes {
        let key = (route.absolute_path.clone(), route.method);

        let (mut operation, source) = if let Some(value) = sidecar_ops.get(&key) {
            (value.clone(), SchemaSource::Sidecar)
        } else if let Some(value) = legacy_ops.get(&key) {
            (value.clone(), SchemaSource::Legacy)
        } else {
            (derive_operation(route, fns), SchemaSource::None)
        };

        strip_upstream_key_marker(&mut operation);
        finish_operation(&mut operation, route, source, &mut merged.tags);
        assign_operation_id(&mut operation, route, &mut taken_operation_ids);

        merged.operations.insert(
            key,
            MergedOperation {
                value: operation,
                source,
            },
        );
    }

    let coverage = Coverage {
        routes_registered: routes.len(),
        routes_described: merged.operations.len(),
        by_source: {
            let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
            for op in merged.operations.values() {
                *counts.entry(op.source.as_str()).or_default() += 1;
            }
            counts
        },
        upstream_only_paths: legacy.upstream_only_paths,
        upstream_only_operations: legacy.upstream_only_operations,
        unreferenced_factories: Vec::new(),
    };
    merged.coverage = coverage;

    Ok(merged)
}

/// The tag a route lands under when no layer supplies one: its module's *first* segment.
///
/// Not the whole module path — `subscriptions::api` would publish a Rust path expression as a
/// heading in the reference — and not the last segment either, which would file 25 subscription
/// endpoints under a heading called `api`. The top-level module is what names a feature area.
fn default_tag(module: &str) -> String {
    module.split("::").next().unwrap_or(module).to_string()
}

/// Build an operation for a route no layer describes, from facts already in the source.
///
/// The handler's own `///` comment is quoted when it has one. That is not a placeholder: those
/// comments are the text the maintainer already writes to explain the endpoint, so quoting them
/// means the document and the code cannot disagree. They are also written *for maintainers* — they
/// reference internal task identifiers and module paths — so the description says plainly whose
/// words these are and that the endpoint is not yet described. A reader must never mistake an
/// internal note for a curated API description.
///
/// Nothing here invents a schema. An endpoint whose request/response shapes are unknown says so,
/// which is far more useful than a plausible guess.
/// Remove the upstream document's `🔑` marker from an operation's summary.
///
/// The vendored LANraragi document prefixes "requires an API key" with a key emoji, because it had
/// no usable per-operation `security` to say so. This document does — derived from the server's own
/// route policy — which makes the marker redundant where it is right and false where it is not:
/// `POST /api/database/drop` carries it upstream, and the policy denies *both* token roles on that
/// route outright, so a key cannot call it at all. A machine-readable artefact should not keep a
/// decoration that contradicts its own machine-readable statement.
///
/// Applied to every operation rather than only the upstream-sourced ones: the sidecar has none
/// today, and a rule with an exception for "the layer that happens not to use it yet" is how the
/// exception gets forgotten.
fn strip_upstream_key_marker(operation: &mut Value) {
    const MARKER: &str = "\u{1f511}";
    let Some(summary) = operation.get("summary").and_then(Value::as_str) else {
        return;
    };
    if !summary.contains(MARKER) {
        return;
    }
    let cleaned = summary
        .replace(MARKER, "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(object) = operation.as_object_mut() {
        object.insert("summary".to_string(), Value::String(cleaned));
    }
}

fn derive_operation(route: &RouteRecord, fns: &FnLookup<'_>) -> Value {
    let handler = route.handler.as_deref();
    let info = handler.and_then(|h| fns.resolve(&route.file, h));
    let doc = info.and_then(|i| i.doc.as_deref());

    let summary = match doc {
        Some(doc) => handlers::split_summary(doc).0,
        None => handler.unwrap_or("endpoint").to_string(),
    };

    let location = format!("{}:{}", route.file, route.line);
    let handler_location = info
        .map(|i| format!("{}:{}", i.file, i.line))
        .unwrap_or_else(|| location.clone());
    let mut description = String::from(
        "Not yet described in this reference. This endpoint is listed because the server registers \
         it, but its request and response shapes have not been written down yet.",
    );
    match doc {
        Some(doc) => description.push_str(&format!(
            "\n\nThe note below is the handler's own documentation comment ({handler_location}), \
             quoted verbatim. It is written for maintainers of this codebase and may reference \
             internal identifiers.\n\n---\n\n{doc}"
        )),
        None => description.push_str(&format!(
            "\n\nThe handler is `{}` in {handler_location}.",
            handler.unwrap_or("unnamed")
        )),
    }

    json!({
        "summary": summary,
        "description": description,
        "responses": {
            "200": {
                "description": format!(
                    "The endpoint responded. Its exact shape is not documented yet — see the \
                     handler at {location}."
                )
            }
        }
    })
}

/// Apply the fields every operation carries regardless of which layer described it, and normalise
/// the ones a layer is allowed to be wrong about.
fn finish_operation(
    operation: &mut Value,
    route: &RouteRecord,
    source: SchemaSource,
    tags: &mut BTreeSet<String>,
) {
    let Some(map) = operation.as_object_mut() else {
        // A non-object operation cannot be salvaged; the integrity gate reports it.
        return;
    };

    // Per-operation `security` is dropped rather than inherited: upstream declares a fixed
    // `api_key` scheme and marks some operations `security: []`, and both statements are false for
    // this server, which replaced that scheme and gates every `/api/*` route through one
    // middleware. The document-level `bearerAuth` applies instead.
    map.remove("security");
    // Upstream's `x-mojo-to` names Perl controller methods that do not exist here; carrying it
    // would send a reader to a dead end.
    map.remove("x-mojo-to");

    let tag = map
        .get("tags")
        .and_then(Value::as_array)
        .and_then(|t| t.first())
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| default_tag(&route.module));
    map.insert("tags".to_string(), json!([tag.clone()]));
    tags.insert(tag);

    map.insert(
        "x-lanrurugi-source".to_string(),
        json!(format!("{}:{}", route.file, route.line)),
    );
    map.insert(
        "x-lanrurugi-schema-source".to_string(),
        json!(source.as_str()),
    );
    if source == SchemaSource::None {
        map.insert("x-lanrurugi-undocumented".to_string(), json!(true));
    } else {
        map.remove("x-lanrurugi-undocumented");
    }

    // Every operation must have at least one response or the document is invalid OpenAPI; a layer
    // that omitted them gets the same honest placeholder the derived layer uses.
    let has_responses = map
        .get("responses")
        .and_then(Value::as_object)
        .is_some_and(|r| !r.is_empty());
    if !has_responses {
        map.insert(
            "responses".to_string(),
            json!({ "200": { "description": "The endpoint responded." } }),
        );
    }
}

/// Give the operation a document-unique `operationId`, derived from the handler name.
fn assign_operation_id(
    operation: &mut Value,
    route: &RouteRecord,
    taken: &mut BTreeSet<String>,
) -> String {
    let Some(map) = operation.as_object_mut() else {
        return String::new();
    };

    let existing = map
        .get("operationId")
        .and_then(Value::as_str)
        .map(str::to_string);

    let base = match existing {
        Some(id) if !id.is_empty() => id,
        _ => match route.handler.as_deref() {
            Some(handler) => handlers::operation_id_from_handler(handler),
            None => handlers::operation_id_from_handler(&route.absolute_path),
        },
    };

    let mut candidate = base.clone();
    if taken.contains(&candidate) {
        // Deterministic rather than a counter: the same route always gets the same suffix.
        let suffix = route
            .absolute_path
            .trim_start_matches('/')
            .replace(['/', '{', '}', '-'], "_");
        candidate = format!("{base}_{}", suffix.trim_matches('_'));
    }
    let mut bump = 2;
    while taken.contains(&candidate) {
        candidate = format!("{base}_{bump}");
        bump += 1;
    }
    taken.insert(candidate.clone());
    map.insert("operationId".to_string(), json!(candidate.clone()));
    candidate
}
