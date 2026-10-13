//! Assembly and serialisation of the generated OpenAPI document.
//!
//! The document is the single artefact every rendered form of the API reference reads. Its shape
//! is fixed by `specs/011-api-docs-site/contracts/openapi-document.md`; this module's job is to
//! produce exactly that and nothing else.
//!
//! Determinism is a contract, not a nicety: paths, methods and JSON keys are all emitted in sorted
//! order, so two runs over unchanged sources are byte-identical. Without that, comparing a published
//! document against a locally generated one would be meaningless. (`serde_json`'s default map is a
//! `BTreeMap`; nothing in this workspace enables `preserve_order`.)

use serde_json::{json, Map, Value};

use crate::access_notes::{self, AccessNotes};
use crate::merge::Merged;
use crate::policy::Policy;
use crate::scan::HttpMethod;

pub const DEFAULT_SERVER_URL: &str = "http://localhost:3000";

const DOCUMENT_DESCRIPTION: &str = "\
The complete HTTP API of a LANrurugi instance.

This document is generated from the server's own route registrations, so every endpoint listed \
here is one the server actually serves, and every endpoint the server serves is listed. Endpoints \
that also exist in LANraragi carry that project's field-level documentation; the rest are listed \
with the handler's own notes and a pointer into the source.

To try a request, set the server address to your own instance (the default is the local port) and \
supply an API token from your instance's settings. Requests go from your browser straight to your \
instance.";

/// `/api` is the only prefix the auth middleware covers: `app.rs` nests every API router under it
/// and layers `require_api_key` *inside* the nest. A router merged outside it (`health`,
/// `opensearch`) never reaches that middleware, so it needs no credentials by construction — not by
/// policy. `scan::mount`'s guard is what keeps that a fact rather than a convention: any nest prefix
/// it does not already know about fails the scan.
const AUTHENTICATED_PREFIX: &str = "/api";

/// Build the complete document.
pub fn assemble(merged: &Merged, policy: &Policy, notes: &AccessNotes, version: &str) -> Value {
    let mut paths: Map<String, Value> = Map::new();
    for ((path, method), operation) in &merged.operations {
        let mut value = operation.value.clone();
        if let Some(object) = value.as_object_mut() {
            attach_security(object, path, *method, policy);
            attach_access_notes(object, path, *method, notes);
        }
        let entry = paths
            .entry(path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        if let Some(item) = entry.as_object_mut() {
            item.insert(method.as_str().to_string(), value);
        }
    }

    let tags: Vec<Value> = merged
        .tags
        .iter()
        .map(|name| match merged.tag_descriptions.get(name) {
            Some(description) => json!({ "name": name, "description": description }),
            None => json!({ "name": name }),
        })
        .collect();

    let mut components = Map::new();
    components.insert(
        "securitySchemes".to_string(),
        json!({
            "bearerAuth": {
                "type": "http",
                "scheme": "bearer",
                "description": "An API token created in your instance's settings. Send it as \
                                `Authorization: Bearer <token>`."
            },
            // The first-party web UI authenticates with a cookie, and 19 operations deliberately
            // refuse API tokens (see `attach_security`). Declaring only `bearerAuth` would tell every
            // consumer of this document that those 19 are unreachable.
            "sessionCookie": {
                "type": "apiKey",
                "in": "cookie",
                "name": "lanrurugi_session",
                "description": "The session cookie set by `POST /api/login`. Only this site's own \
                                frontend uses it; there is no way to obtain one from an API token, \
                                which is why the endpoints below accept nothing else."
            }
        }),
    );
    if !merged.schemas.is_empty() {
        components.insert("schemas".to_string(), Value::Object(merged.schemas.clone()));
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "LANrurugi API",
            "version": version,
            "license": { "name": "MIT", "identifier": "MIT" },
            "description": DOCUMENT_DESCRIPTION,
        },
        "servers": [
            {
                "url": DEFAULT_SERVER_URL,
                "description": "LANrurugi's default local address. Replace this with your own \
                                instance's address — see the API testing guide."
            }
        ],
        // The default for the 199 operations that accept either credential. Operations that accept
        // only one, or none at all, carry their own `security` (see `attach_security`).
        "security": [ { "bearerAuth": [] }, { "sessionCookie": [] } ],
        "tags": tags,
        "paths": Value::Object(paths),
        "components": Value::Object(components),
    })
}

/// Attach the per-role behaviour notes, if this route has any.
///
/// A distinct key rather than an addition to `description`: `merge` replaces whole operations
/// precisely so a reader can predict the result from either input, and appending to a field would
/// quietly reintroduce the field-level merging it rejects. See `access_notes` for why the note
/// cannot live in `security` either — OpenAPI has no per-role condition on parameters or responses.
fn attach_access_notes(
    object: &mut Map<String, Value>,
    path: &str,
    method: HttpMethod,
    notes: &AccessNotes,
) {
    let Some(entries) = notes.get(path, method) else {
        return;
    };
    object.insert(access_notes::FIELD.to_string(), json!(entries));
    // Appended to whatever the description already says rather than replacing it — the operation's
    // own prose stays first. See `DESCRIPTION_MARKER` for why this is duplicated into two fields.
    let block = access_notes::description_block(entries);
    let combined = match object.get("description").and_then(Value::as_str) {
        Some(existing) if !existing.trim().is_empty() => format!("{existing}\n\n{block}"),
        _ => block,
    };
    object.insert("description".to_string(), Value::String(combined));
}

/// State one operation's real authentication requirement.
///
/// The requirement is read from `policy` — the Casbin file the server enforces — rather than
/// declared here, because a hand-written claim is exactly what went wrong before: a single global
/// `security: [{bearerAuth: []}]` was wrong for 32 of 231 operations (19 that refuse tokens, 13
/// that need no credentials at all), and nothing could have caught it.
///
/// Only the exceptions are written. The 199 operations that accept either credential inherit the
/// document's global `security`, which is what keeps this from repeating one identical array 199
/// times.
fn attach_security(
    object: &mut Map<String, Value>,
    path: &str,
    method: HttpMethod,
    policy: &Policy,
) {
    if !path.starts_with(AUTHENTICATED_PREFIX) {
        object.insert("security".to_string(), json!([]));
        return;
    }

    let Some(access) = policy.access(method.as_str(), path) else {
        // `validate` gates this; reaching here means the gate was bypassed.
        return;
    };

    // State the role where a guest token is refused. 117 routes are administrator-only, and without
    // this they are indistinguishable from the 87 any token reaches — a reader holding a guest token
    // has no way to know which is which until the 403. Only the exception is marked, matching
    // `x-lanrurugi-guest-visible`: absent means "any token".
    if access.admin_token_only() {
        object.insert("x-lanrurugi-token-role".to_string(), json!("admin"));
    }

    if access.guest_visitor {
        // Not `security: []`: a guest visitor needs no credentials only while the operator has guest
        // access switched on, which is not something OpenAPI can express. The marker carries the
        // fact and the credential requirement stays stated.
        object.insert("x-lanrurugi-guest-visible".to_string(), json!(true));
    }

    if access.anonymous {
        // Reachable with no credentials, ever. An empty requirement array is OpenAPI's way of
        // removing the document-level default for this operation.
        object.insert("security".to_string(), json!([]));
    } else if access.is_session_only() {
        object.insert("security".to_string(), json!([ { "sessionCookie": [] } ]));
    } else if access.is_token_only() {
        object.insert("security".to_string(), json!([ { "bearerAuth": [] } ]));
    }
    // Otherwise both credentials work, which is the document-level default.
}

/// Serialise for writing. `to_string_pretty` plus the sorted maps above is the whole determinism
/// story; a trailing newline is added so the file is a well-formed text file.
pub fn to_json(document: &Value) -> String {
    let mut out = serde_json::to_string_pretty(document)
        .expect("the generated document is built from JSON values and cannot fail to serialise");
    out.push('\n');
    out
}
