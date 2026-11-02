//! Layering, precedence and coverage across the three description sources.
//!
//! Precedence is the part of this design most likely to be misread later — sidecar beats upstream
//! beats auto-derived, and replacement is whole-operation rather than field-by-field. Each case
//! below pins one direction of that, plus the gates that stop a hand-written fragment from
//! describing an endpoint that does not exist.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use lanrurugi_api_docgen::legacy::{self, LegacyLayer};
use lanrurugi_api_docgen::merge::{self, SchemaSource};
use lanrurugi_api_docgen::scan::handlers::FnLookup;
use lanrurugi_api_docgen::scan::{FnInfo, HttpMethod, RouteRecord};
use lanrurugi_api_docgen::sidecar::{self, SidecarLayer};
use serde_json::{json, Map, Value};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "lanrurugi-docgen-pipeline-{}-{}",
            std::process::id(),
            unique
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sidecar")).expect("create scratch dir");
        Self { root }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.path(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&path, body).expect("write scratch file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn route(path: &str, method: HttpMethod, handler: &str) -> RouteRecord {
    RouteRecord {
        method,
        absolute_path: path.to_string(),
        declared_path: path.trim_start_matches("/api").to_string(),
        factory: "thing::router".to_string(),
        handler: Some(handler.to_string()),
        module: "thing".to_string(),
        file: "crates/lanrurugi-api/src/thing.rs".to_string(),
        line: 12,
    }
}

fn empty_layers() -> (LegacyLayer, SidecarLayer) {
    (LegacyLayer::default(), SidecarLayer::default())
}

/// A policy that reaches everything the fixtures register under `/api`.
///
/// The gates below predate the policy layer and test layering, not authentication; they need *a*
/// policy that lets their fixtures through, not a meaningful one. Anything that tests the policy
/// behaviour itself builds its own (see the `policy_*` tests).
fn permissive_policy() -> lanrurugi_api_docgen::policy::Policy {
    lanrurugi_api_docgen::policy::Policy::parse("p, session, /api/*, *, allow\n").expect("policy")
}

fn no_fns() -> Vec<FnInfo> {
    Vec::new()
}

fn operation_ids(document: &merge::Merged) -> Vec<String> {
    document
        .operations
        .values()
        .filter_map(|op| op.value.get("operationId").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

// ── sidecar ─────────────────────────────────────────────────────────────────

#[test]
fn sidecar_entry_for_a_route_that_does_not_exist_fails() {
    let scratch = Scratch::new();
    scratch.write(
        "sidecar/ghost.yaml",
        r#"
"/api/ghost":
  get:
    summary: Not a real endpoint
    responses:
      "200": { description: ok }
"#,
    );

    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];
    let error = sidecar::load(&scratch.path("sidecar"), &routes)
        .expect_err("a stale sidecar entry must fail");
    let message = format!("{error:#}");

    assert!(
        message.contains("/api/ghost") && message.contains("does not match any route"),
        "message should name the offending entry and say why: {message}"
    );
}

#[test]
fn sidecar_unknown_method_key_fails() {
    let scratch = Scratch::new();
    scratch.write(
        "sidecar/thing.yaml",
        r#"
"/api/thing":
  gets:
    summary: typo
    responses:
      "200": { description: ok }
"#,
    );

    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];
    let error = sidecar::load(&scratch.path("sidecar"), &routes).expect_err("unknown method fails");
    let message = format!("{error:#}");
    assert!(
        message.contains("gets"),
        "message should name the key: {message}"
    );
}

#[test]
fn two_fragments_claiming_one_operation_fail() {
    let scratch = Scratch::new();
    for name in ["a.yaml", "b.yaml"] {
        scratch.write(
            &format!("sidecar/{name}"),
            r#"
"/api/thing":
  get:
    summary: From this file
    responses:
      "200": { description: ok }
"#,
        );
    }

    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];
    let error = sidecar::load(&scratch.path("sidecar"), &routes).expect_err("duplicate fails");
    let message = format!("{error:#}");
    assert!(
        message.contains("more than one fragment"),
        "message should explain the ambiguity: {message}"
    );
}

#[test]
fn a_valid_fragment_loads_and_its_schemas_come_with_it() {
    let scratch = Scratch::new();
    scratch.write(
        "sidecar/thing.yaml",
        r##"
"/api/thing":
  get:
    summary: List things
    responses:
      "200":
        description: The things.
        content:
          application/json:
            schema:
              type: array
              items: { $ref: "#/components/schemas/Thing" }

components:
  schemas:
    Thing:
      type: object
      properties:
        id: { type: string }
"##,
    );

    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];
    let layer = sidecar::load(&scratch.path("sidecar"), &routes).expect("valid fragment loads");

    assert_eq!(layer.files_read, 1);
    assert!(layer
        .operations
        .contains_key(&("/api/thing".to_string(), HttpMethod::Get)));
    assert!(layer.schemas.contains_key("Thing"));
}

#[test]
fn sidecar_schema_colliding_with_an_upstream_schema_fails_at_merge() {
    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];

    let mut legacy = LegacyLayer::default();
    legacy
        .schemas
        .insert("Thing".to_string(), json!({ "type": "object" }));

    let mut sidecar = SidecarLayer::default();
    sidecar
        .schemas
        .insert("Thing".to_string(), json!({ "type": "string" }));

    let fns = no_fns();
    let error = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns))
        .expect_err("a silent schema override must fail");
    assert!(format!("{error:#}").contains("Thing"));
}

// ── precedence ──────────────────────────────────────────────────────────────

#[test]
fn a_fragment_replaces_an_upstream_operation_whole() {
    let routes = vec![route("/api/search", HttpMethod::Get, "search")];

    let mut legacy = LegacyLayer::default();
    legacy.operations.insert(
        ("/api/search".to_string(), HttpMethod::Get),
        json!({
            "summary": "Upstream's summary",
            "description": "Upstream's description",
            "security": [],
            "parameters": [ { "name": "category", "in": "query" } ],
            "responses": { "200": { "description": "from upstream" } },
        }),
    );

    let mut sidecar = SidecarLayer::default();
    sidecar.operations.insert(
        ("/api/search".to_string(), HttpMethod::Get),
        json!({
            "summary": "LANrurugi's summary",
            "responses": { "200": { "description": "from the sidecar" } },
        }),
    );

    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");
    let op = merged
        .operations
        .get(&("/api/search".to_string(), HttpMethod::Get))
        .expect("operation present");

    assert_eq!(op.source, SchemaSource::Sidecar);
    assert_eq!(op.value["summary"], json!("LANrurugi's summary"));
    // Whole-operation replacement: upstream's `parameters` are gone, not merged in.
    assert!(op.value.get("parameters").is_none());
    assert_eq!(merged.coverage.by_source.get("sidecar").copied(), Some(1));
    assert_eq!(merged.coverage.by_source.get("legacy").copied(), None);
}

#[test]
fn a_fragment_may_add_a_method_to_an_upstream_path_without_disturbing_the_others() {
    let routes = vec![
        route("/api/things", HttpMethod::Get, "list_things"),
        route("/api/things", HttpMethod::Post, "create_thing"),
    ];

    let mut legacy = LegacyLayer::default();
    legacy.operations.insert(
        ("/api/things".to_string(), HttpMethod::Get),
        json!({
            "summary": "List things",
            "responses": { "200": { "description": "from upstream" } },
        }),
    );

    let mut sidecar = SidecarLayer::default();
    sidecar.operations.insert(
        ("/api/things".to_string(), HttpMethod::Post),
        json!({
            "summary": "Create a thing",
            "responses": { "201": { "description": "created" } },
        }),
    );

    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");

    assert_eq!(
        merged
            .operations
            .get(&("/api/things".to_string(), HttpMethod::Get))
            .expect("get present")
            .source,
        SchemaSource::Legacy
    );
    assert_eq!(
        merged
            .operations
            .get(&("/api/things".to_string(), HttpMethod::Post))
            .expect("post present")
            .source,
        SchemaSource::Sidecar
    );
    assert_eq!(merged.coverage.by_source.get("legacy").copied(), Some(1));
    assert_eq!(merged.coverage.by_source.get("sidecar").copied(), Some(1));
}

#[test]
fn a_route_no_layer_describes_is_still_emitted_and_marked() {
    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];

    let fns = vec![FnInfo {
        name: "list_things".to_string(),
        file: "crates/lanrurugi-api/src/thing.rs".to_string(),
        line: 40,
        doc: Some("Every thing, newest first.\n\nMore detail follows.".to_string()),
    }];

    let (legacy, sidecar) = empty_layers();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");
    let op = merged
        .operations
        .get(&("/api/thing".to_string(), HttpMethod::Get))
        .expect("operation present");

    assert_eq!(op.source, SchemaSource::None);
    assert_eq!(op.value["summary"], json!("Every thing, newest first"));
    let description = op.value["description"].as_str().expect("description");
    assert!(
        description.contains("Not yet described in this reference"),
        "an auto-derived entry must say so plainly: {description}"
    );
    assert!(
        description.contains("thing.rs:40"),
        "the handler's own location should be named: {description}"
    );
    assert_eq!(op.value["x-lanrurugi-undocumented"], json!(true));
    assert_eq!(op.value["x-lanrurugi-schema-source"], json!("none"));
}

// ── upstream layer ──────────────────────────────────────────────────────────

#[test]
fn upstream_layer_prefixes_paths_from_its_own_servers_entry_and_reports_the_rest() {
    let scratch = Scratch::new();
    let legacy_path = scratch.write(
        "legacy.yaml",
        r#"
openapi: 3.1.0
info:
  title: Whatever
  version: 0.0.0
  license: { name: MIT, identifier: MIT }
servers:
  - url: https://example.invalid/some-prefix
tags:
  - name: search
    description: Perform searches.
paths:
  "/search":
    get:
      summary: Search
      security: []
      x-mojo-to: api-search#handle_api
      operationId: searchArchives
      responses:
        "200": { description: results }
  "/gone":
    get:
      summary: Only upstream has this
      responses:
        "200": { description: gone }
components:
  schemas:
    ServerInfo:
      type: object
"#,
    );

    let routes = vec![route("/some-prefix/search", HttpMethod::Get, "search")];
    let layer = legacy::load(&legacy_path, &routes).expect("load");

    assert_eq!(layer.prefix, "/some-prefix");
    let op = layer
        .operations
        .get(&("/some-prefix/search".to_string(), HttpMethod::Get))
        .expect("shared path carried over under the derived prefix");
    assert!(
        op.get("security").is_none(),
        "per-operation security is dropped"
    );
    assert!(
        op.get("x-mojo-to").is_none(),
        "upstream controller names are dropped"
    );
    assert_eq!(op["summary"], json!("Search"));

    assert_eq!(
        layer.upstream_only_paths,
        vec!["/some-prefix/gone".to_string()]
    );
    assert!(layer.schemas.contains_key("ServerInfo"));
    // Upstream's tag prose is carried, but only for tags an operation actually uses — the upstream
    // document declares tags for paths this server does not serve, and emitting those would render
    // empty groups in the reference.
    assert_eq!(
        layer.tag_descriptions.get("search").map(String::as_str),
        Some("Perform searches.")
    );
}

#[test]
fn upstream_document_in_a_different_dialect_is_refused() {
    let scratch = Scratch::new();
    let path = scratch.write(
        "legacy.yaml",
        r#"
openapi: 3.0.0
info: { title: Old, version: 1.0.0 }
paths: {}
"#,
    );

    let error = legacy::load(&path, &[]).expect_err("a different dialect must not be translated");
    assert!(format!("{error:#}").contains("3.0.0"));
}

#[test]
fn upstream_method_we_do_not_serve_is_reported_separately() {
    let scratch = Scratch::new();
    let path = scratch.write(
        "legacy.yaml",
        r#"
openapi: 3.1.0
info: { title: Whatever, version: 0.0.0 }
servers: [ { url: https://example.invalid/api } ]
paths:
  "/thing":
    get:
      responses: { "200": { description: ok } }
    delete:
      responses: { "200": { description: ok } }
"#,
    );

    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];
    let layer = legacy::load(&path, &routes).expect("load");

    assert!(layer.upstream_only_paths.is_empty());
    assert_eq!(
        layer.upstream_only_operations,
        vec!["DELETE /api/thing".to_string()]
    );
    assert_eq!(layer.operations.len(), 1);
}

// ── coverage and integrity ──────────────────────────────────────────────────

#[test]
fn coverage_accounts_for_every_route_exactly_once() {
    let routes = vec![
        route("/api/from-legacy", HttpMethod::Get, "a"),
        route("/api/from-sidecar", HttpMethod::Get, "b"),
        route("/api/from-nowhere", HttpMethod::Get, "c"),
    ];

    let mut legacy = LegacyLayer::default();
    legacy.operations.insert(
        ("/api/from-legacy".to_string(), HttpMethod::Get),
        json!({ "responses": { "200": { "description": "ok" } } }),
    );

    let mut sidecar = SidecarLayer::default();
    sidecar.operations.insert(
        ("/api/from-sidecar".to_string(), HttpMethod::Get),
        json!({ "responses": { "200": { "description": "ok" } } }),
    );

    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");

    assert_eq!(merged.coverage.routes_registered, 3);
    assert_eq!(merged.coverage.routes_described, 3);
    assert_eq!(merged.coverage.by_source.get("legacy").copied(), Some(1));
    assert_eq!(merged.coverage.by_source.get("sidecar").copied(), Some(1));
    assert_eq!(merged.coverage.by_source.get("none").copied(), Some(1));

    lanrurugi_api_docgen::validate::run(&merged, &permissive_policy()).expect("gates pass");
}

#[test]
fn operation_ids_are_unique_even_when_handlers_share_a_name() {
    // Two modules, one handler name — a real shape in this codebase (`router` aside, several
    // modules define a `search` or `list` helper).
    let mut a = route("/api/a", HttpMethod::Get, "list");
    a.module = "a".to_string();
    let mut b = route("/api/b", HttpMethod::Get, "list");
    b.module = "b".to_string();

    let routes = vec![a, b];
    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");

    let ids = operation_ids(&merged);
    assert_eq!(ids.len(), 2);
    let unique: std::collections::BTreeSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), 2, "operationIds must be unique: {ids:?}");
    lanrurugi_api_docgen::validate::run(&merged, &permissive_policy()).expect("gates pass");
}

#[test]
fn tags_default_to_the_module_and_the_document_collects_them() {
    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];
    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");

    assert!(merged.tags.contains("thing"));
}

#[test]
fn an_operation_that_arrives_without_responses_gets_a_valid_placeholder() {
    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];

    let mut legacy = LegacyLayer::default();
    legacy.operations.insert(
        ("/api/thing".to_string(), HttpMethod::Get),
        json!({ "summary": "No responses declared" }),
    );

    let sidecar = SidecarLayer::default();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");
    let op = merged
        .operations
        .get(&("/api/thing".to_string(), HttpMethod::Get))
        .expect("operation present");

    let responses: &Map<String, Value> = op.value["responses"].as_object().expect("responses");
    assert!(!responses.is_empty());
    lanrurugi_api_docgen::validate::run(&merged, &permissive_policy()).expect("gates pass");
}

// ── Authentication: derived from the policy, not declared ────────────────────────────────────────
//
// The document used to state one global `security: [{bearerAuth: []}]` for every operation, which
// was wrong for 38 of 231 (25 that refuse API tokens, 11 that need no credentials, 2 served outside
// the auth middleware). These pin the derivation that replaced it.

/// Build a document from `routes` under `policy_text`, returning it as JSON.
fn assemble_with(routes: Vec<RouteRecord>, policy_text: &str) -> Value {
    use lanrurugi_api_docgen::access_notes::AccessNotes;
    use lanrurugi_api_docgen::policy::Policy;
    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");
    let policy = Policy::parse(policy_text).expect("policy");
    lanrurugi_api_docgen::emit::assemble(&merged, &policy, &AccessNotes::default(), "0.0.0")
}

fn security_of(document: &Value, path: &str, method: &str) -> Option<Value> {
    document["paths"][path][method].get("security").cloned()
}

#[test]
fn both_schemes_are_declared_and_the_default_accepts_either() {
    let document = assemble_with(
        vec![route("/api/thing", HttpMethod::Get, "list_things")],
        "p, session, /api/*, *, allow\np, token_admin, /api/*, GET, allow\n",
    );
    let schemes = document["components"]["securitySchemes"]
        .as_object()
        .expect("schemes");
    assert!(schemes.contains_key("bearerAuth"));
    assert!(schemes.contains_key("sessionCookie"));
    assert_eq!(schemes["sessionCookie"]["in"], "cookie");
    assert_eq!(schemes["sessionCookie"]["name"], "lanrurugi_session");
    assert_eq!(
        document["security"],
        json!([{ "bearerAuth": [] }, { "sessionCookie": [] }])
    );
    // Reachable by both, so it carries no override and inherits that default.
    assert!(security_of(&document, "/api/thing", "get").is_none());
}

#[test]
fn a_route_that_refuses_tokens_declares_the_cookie_scheme_only() {
    let document = assemble_with(
        vec![route("/api/tokens", HttpMethod::Get, "list_tokens")],
        "p, session, /api/*, *, allow\np, token_admin, /api/*, GET, allow\n\
         p, token_guest, /api/*, GET, allow\np, token_admin, /api/tokens, GET, deny\n\
         p, token_guest, /api/tokens, GET, deny\n",
    );
    assert_eq!(
        security_of(&document, "/api/tokens", "get"),
        Some(json!([{ "sessionCookie": [] }])),
        "a token cannot call this, so the document must not say bearer works"
    );
}

#[test]
fn an_anonymous_route_declares_no_requirement_at_all() {
    let document = assemble_with(
        vec![route("/api/login", HttpMethod::Post, "login")],
        "p, anonymous, /api/login, POST, allow\np, session, /api/*, *, allow\n",
    );
    assert_eq!(
        security_of(&document, "/api/login", "post"),
        Some(json!([])),
        "an empty requirement array is how an operation opts out of the document default"
    );
}

#[test]
fn a_route_outside_the_authenticated_prefix_needs_no_credentials() {
    // `/health` is merged outside the `/api` nest in `app.rs`, so it never reaches
    // `require_api_key`. The policy says nothing about it and must not have to.
    let document = assemble_with(
        vec![route("/health", HttpMethod::Get, "health")],
        "p, session, /api/*, *, allow\n",
    );
    assert_eq!(security_of(&document, "/health", "get"), Some(json!([])));
}

#[test]
fn a_guest_visible_route_is_marked_rather_than_called_public() {
    // `guest_visitor` means "no credentials while guest mode is on", which OpenAPI cannot express as
    // a security requirement. It must not be flattened into `security: []`, which would claim the
    // endpoint is public unconditionally.
    let document = assemble_with(
        vec![route("/api/search", HttpMethod::Get, "search")],
        "p, session, /api/*, *, allow\np, token_admin, /api/*, GET, allow\n\
         p, guest_visitor, /api/search, GET, allow\n",
    );
    assert!(security_of(&document, "/api/search", "get").is_none());
    assert_eq!(
        document["paths"]["/api/search"]["get"]["x-lanrurugi-guest-visible"],
        json!(true)
    );
}

#[test]
fn a_route_the_policy_never_mentions_fails_the_gate() {
    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(
        &[route("/api/forgotten", HttpMethod::Get, "forgotten")],
        legacy,
        sidecar,
        &FnLookup::new(&fns),
    )
    .expect("merge");
    // Deliberately not `/api/*`: a wildcard would match the route and the gate would be right to
    // pass. "Never mentions it" means no pattern matches at all.
    let policy = lanrurugi_api_docgen::policy::Policy::parse("p, session, /api/known, *, allow\n")
        .expect("policy");

    let error = lanrurugi_api_docgen::validate::run(&merged, &policy)
        .expect_err("a served route with no policy must not pass");
    let message = format!("{error:#}");
    assert!(message.contains("/api/forgotten"), "{message}");
    assert!(message.contains("no rule matches it at all"), "{message}");
}

#[test]
fn a_route_only_denied_fails_the_gate() {
    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(
        &[route("/api/locked", HttpMethod::Get, "locked")],
        legacy,
        sidecar,
        &FnLookup::new(&fns),
    )
    .expect("merge");
    let policy = lanrurugi_api_docgen::policy::Policy::parse(
        "p, session, /api/*, *, allow\np, session, /api/locked, GET, deny\n",
    )
    .expect("policy");

    let error = lanrurugi_api_docgen::validate::run(&merged, &policy)
        .expect_err("a route nobody can reach must not pass");
    assert!(format!("{error:#}").contains("every rule that matches it denies"));
}

#[test]
fn a_mutating_route_reachable_by_a_guest_token_fails_the_gate() {
    // The one invariant in this policy that is not default-allow. `token_guest` gets `GET` by
    // wildcard, so every mutating route must deny it explicitly; the previous version of this gate
    // asked a question the wildcards made unanswerable, and could never fire.
    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(
        &[route("/api/thing", HttpMethod::Post, "make_thing")],
        legacy,
        sidecar,
        &FnLookup::new(&fns),
    )
    .expect("merge");
    // A `*` action for the guest token is exactly the mistake this gate exists to catch: with the
    // real file's `GET`-only wildcard the route is safe, and the test would prove nothing.
    let policy = lanrurugi_api_docgen::policy::Policy::parse(
        "p, session, /api/*, *, allow\np, token_guest, /api/*, *, allow\n",
    )
    .expect("policy");

    let error = lanrurugi_api_docgen::validate::run(&merged, &policy)
        .expect_err("a guest token must not reach a POST");
    let message = format!("{error:#}");
    assert!(message.contains("reachable by a guest token"), "{message}");
    assert!(message.contains("/api/thing"), "{message}");
}

#[test]
fn a_guest_token_denied_the_mutating_method_passes() {
    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(
        &[route("/api/thing", HttpMethod::Post, "make_thing")],
        legacy,
        sidecar,
        &FnLookup::new(&fns),
    )
    .expect("merge");
    let policy = lanrurugi_api_docgen::policy::Policy::parse(
        "p, session, /api/*, *, allow\np, token_guest, /api/*, GET, allow\n\
         p, token_guest, /api/thing, POST, deny\n",
    )
    .expect("policy");

    lanrurugi_api_docgen::validate::run(&merged, &policy).expect("gates pass");
}

#[test]
fn the_upstream_key_marker_is_stripped_from_summaries() {
    // Upstream prefixes "requires an API key" with 🔑. This document states the requirement in
    // `security`, derived from the route policy — and for `POST /api/database/drop` the policy
    // refuses every token role, so the marker was not merely redundant there but false.
    let routes = vec![route(
        "/api/database/drop",
        HttpMethod::Post,
        "drop_database",
    )];
    let mut legacy = LegacyLayer::default();
    legacy.operations.insert(
        ("/api/database/drop".to_string(), HttpMethod::Post),
        json!({ "summary": "\u{1f511} Drop the Database" }),
    );
    let sidecar = SidecarLayer::default();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");

    let summary = merged
        .operations
        .get(&("/api/database/drop".to_string(), HttpMethod::Post))
        .and_then(|op| op.value.get("summary"))
        .and_then(Value::as_str);
    assert_eq!(summary, Some("Drop the Database"));
}

#[test]
fn a_summary_without_the_marker_is_left_alone() {
    let routes = vec![route("/api/thing", HttpMethod::Get, "list_things")];
    let mut legacy = LegacyLayer::default();
    legacy.operations.insert(
        ("/api/thing".to_string(), HttpMethod::Get),
        json!({ "summary": "List  things" }),
    );
    let sidecar = SidecarLayer::default();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");
    let summary = merged
        .operations
        .get(&("/api/thing".to_string(), HttpMethod::Get))
        .and_then(|op| op.value.get("summary"))
        .and_then(Value::as_str);
    assert_eq!(summary, Some("List  things"));
}

#[test]
fn an_administrator_only_route_says_so() {
    // 117 routes in this API accept an administrator token and refuse a guest one. Without a marker
    // they are indistinguishable from the 87 that accept either, and a reader holding a guest token
    // only finds out from a 403.
    let document = assemble_with(
        vec![route(
            "/api/archives/{id}",
            HttpMethod::Delete,
            "delete_archive",
        )],
        "p, session, /api/*, *, allow\np, token_admin, /api/*, DELETE, allow\n\
         p, token_guest, /api/*, GET, allow\n",
    );
    assert_eq!(
        document["paths"]["/api/archives/{id}"]["delete"]["x-lanrurugi-token-role"],
        json!("admin")
    );
}

#[test]
fn a_route_both_token_roles_reach_carries_no_role_marker() {
    // The convention, matching `x-lanrurugi-guest-visible`: only the exception is marked, so an
    // absent marker means "any token".
    let document = assemble_with(
        vec![route("/api/archives", HttpMethod::Get, "list_archives")],
        "p, session, /api/*, *, allow\np, token_admin, /api/*, GET, allow\n\
         p, token_guest, /api/*, GET, allow\n",
    );
    assert!(document["paths"]["/api/archives"]["get"]
        .get("x-lanrurugi-token-role")
        .is_none());
}

#[test]
fn a_route_no_token_reaches_carries_no_role_marker() {
    // A session-only route must not claim a role at all: neither token works there.
    let document = assemble_with(
        vec![route("/api/tokens", HttpMethod::Get, "list_tokens")],
        "p, session, /api/*, *, allow\np, token_admin, /api/*, GET, allow\n\
         p, token_guest, /api/*, GET, allow\np, token_admin, /api/tokens, GET, deny\n\
         p, token_guest, /api/tokens, GET, deny\n",
    );
    let operation = &document["paths"]["/api/tokens"]["get"];
    assert!(operation.get("x-lanrurugi-token-role").is_none());
    assert_eq!(operation["security"], json!([{ "sessionCookie": [] }]));
}

#[test]
fn access_notes_reach_the_document_as_their_own_field() {
    // Appended as a distinct key rather than folded into `description`: `merge` replaces whole
    // operations so the result stays predictable from either input, and appending to a field would
    // reintroduce the field-level merging it rejects. OpenAPI cannot express per-role behaviour any
    // other way — there is no per-role condition on parameters or responses.
    use lanrurugi_api_docgen::access_notes;
    use lanrurugi_api_docgen::policy::Policy;

    let notes_path = std::env::temp_dir().join("lanrurugi-access-notes-emit.yaml");
    std::fs::write(
        &notes_path,
        "\"GET /api/activity\":\n  - A session sees every entry.\n",
    )
    .expect("write fixture");

    let routes = vec![route("/api/activity", HttpMethod::Get, "list_activity")];
    let notes = access_notes::load(&notes_path, &routes).expect("load notes");
    let _ = std::fs::remove_file(&notes_path);

    let (legacy, sidecar) = empty_layers();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");
    let policy =
        Policy::parse("p, session, /api/*, *, allow\np, token_admin, /api/*, GET, allow\n")
            .expect("policy");
    let document = lanrurugi_api_docgen::emit::assemble(&merged, &policy, &notes, "0.0.0");

    assert_eq!(
        document["paths"]["/api/activity"]["get"]["x-lanrurugi-access-notes"],
        json!(["A session sees every entry."])
    );
}

#[test]
fn access_notes_also_reach_the_description_as_prose() {
    // Swagger UI renders `description` under the endpoint title and has no other placement for a
    // vendor extension — its Extensions table sits at the very bottom (measured: 1391px into a
    // 1570px operation). The notes are therefore emitted twice on purpose: prose for humans there,
    // and the structured field for anything that branches on it. The blank lines are load-bearing —
    // with the marker directly above the heading, Swagger UI's markdown renderer treated the block
    // as raw text and printed the asterisks.
    use lanrurugi_api_docgen::access_notes;
    use lanrurugi_api_docgen::policy::Policy;

    let notes_path = std::env::temp_dir().join("lanrurugi-access-notes-prose.yaml");
    std::fs::write(
        &notes_path,
        "\"GET /api/activity\":\n  - A guest token sees only its own entries.\n",
    )
    .expect("write fixture");

    let routes = vec![route("/api/activity", HttpMethod::Get, "list_activity")];
    let notes = access_notes::load(&notes_path, &routes).expect("load notes");
    let _ = std::fs::remove_file(&notes_path);

    let mut legacy = LegacyLayer::default();
    legacy.operations.insert(
        ("/api/activity".to_string(), HttpMethod::Get),
        json!({ "description": "A page of entries." }),
    );
    let sidecar = SidecarLayer::default();
    let fns = no_fns();
    let merged = merge::merge(&routes, legacy, sidecar, &FnLookup::new(&fns)).expect("merge");
    let policy = Policy::parse("p, session, /api/*, *, allow\n").expect("policy");
    let document = lanrurugi_api_docgen::emit::assemble(&merged, &policy, &notes, "0.0.0");

    let description = document["paths"]["/api/activity"]["get"]["description"]
        .as_str()
        .expect("description");
    assert!(
        description.starts_with("A page of entries."),
        "the operation's own prose stays first: {description}"
    );
    assert!(
        description.contains("<!-- lanrurugi:access-notes -->\n\n**Access differs by role**\n\n- A guest token sees only its own entries."),
        "the block must carry the blank lines that make it parse as markdown: {description:?}"
    );
}
