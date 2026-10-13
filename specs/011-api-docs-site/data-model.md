> **Superseded in part (2026-10-11) — see [`contracts/deliverables.md`](./contracts/deliverables.md).**
> The rendering decisions recorded in this file were revisited after comparing the result against
> the reference documentation site: the whole-page Scalar viewer was replaced by generated
> per-group pages, and the API document now also feeds a standalone Swagger UI. Where this file
> still says `@scalar/starlight` or `docs/public/sdk/`, the deliverable contract is authoritative.

# Phase 1 Data Model: Developer Documentation Site with Generated API Reference

**Feature**: `011-api-docs-site` | **Date**: 2026-10-11

This feature persists nothing at runtime. The entities below are the in-memory and on-disk shapes the
build pipeline moves between its stages, plus the two artifacts that leave the repository (the
generated API document and the built site). They are specified because the coverage gate is defined
in terms of them, and because the merge precedence between three independent sources of description
is the part of this design most likely to be misread later.

---

## 1. `RouteRecord`

The atomic unit of completeness. One HTTP method on one absolute path.

| Field | Type | Notes |
|---|---|---|
| `method` | enum: `get`/`post`/`put`/`delete`/`patch`/`head`/`options` | Lowercase, matching OpenAPI's operation keys. |
| `absolute_path` | string | Always begins with `/`. Includes the mount prefix the router is nested under. |
| `declared_path` | string | The literal as written in the source, before the mount prefix. Kept for diagnostics. |
| `mount` | `MountPoint` | Which router factory and prefix this came from. |
| `handler` | string | The handler function's name as written, e.g. `list_jobs`. |
| `module` | string | The module path relative to `crates/lanrurugi-api/src`, e.g. `download_manager::queue`. Used as the default OpenAPI tag. |
| `source` | `SourceLocation` (`file`, `line`) | Where the `.route()` call is. Emitted as `x-lanrurugi-source`. |
| `doc` | optional string | The handler function's own `///` doc comment, verbatim, minus the leading `///` markers. |

**Identity**: `(method, absolute_path)` — unique within a run; a duplicate is a hard error, since two
handlers claiming one method+path means one of them is unreachable.

**Provenance rule**: a `RouteRecord` exists if and only if the server registers it. Nothing in the
documentation layers can create one, and nothing can remove one. This is what makes the completeness
claim structural rather than asserted.

**Normalisation**: the declared path is used as-is as the OpenAPI path template — axum 0.8's
`{param}` syntax and OpenAPI's `{param}` syntax are already identical, so no rewriting is applied.
(Verified against the upstream document, whose templates use the same spelling.)

---

## 2. `MountPoint`

Where a router factory's declared paths actually live.

| Field | Type | Notes |
|---|---|---|
| `factory` | string | `module::function`, e.g. `health::router`, `subscriptions::api::router`. |
| `prefix` | string | `"/api"` or `""` (served bare at the root). |
| `declared_in` | string | The file that mounts it, e.g. `crates/lanrurugi-server/src/app.rs`. |

**Invariant (checked, not assumed)**: the set of `MountPoint.factory` values equals the set of router
factory functions discovered by the scanner. A factory merged nowhere, or merged but absent from the
table, fails generation. This two-way check is the mechanism that prevents a new router from being
silently mounted somewhere the generator does not know about.

---

## 3. `LegacyDocument`

The vendored upstream LANraragi OpenAPI 3.1 document (`docs/api/legacy-openapi.yaml`), read but never
modified on disk.

| Field | Notes |
|---|---|
| `source_revision` | Recorded so a refresh is traceable. |
| `paths` | 64 path items at the revision vendored for this feature. |
| `servers[0].url` | Ends in `/api`; this is what makes the prefix rewrite a literal operation. |
| `components.securitySchemes` | Contains `api_key` only. Deliberately **not** carried into the output. |

**Derived during merge**:
- `shared` — paths that also exist in the route table (55 at this revision). Their operations are the
  base layer for those routes.
- `upstream_only` — paths the upstream document describes and this server does not register (9 at
  this revision). **Reported, never published.** These are the concrete evidence for the spec's
  FR-008: the first run of this mechanism already finds nine endpoints a reader of the upstream
  document would expect and this server does not serve.

**Rewrite applied on read**: every path key gains the `/api` prefix derived from the document's own
`servers[0].url`, and every operation's `security` field is dropped (see `openapi-document.md`).

---

## 4. `SidecarFragment`

A hand-written OpenAPI fragment file under `docs/api/sidecar/`, one per API group.

| Field | Notes |
|---|---|
| `path` | A full, absolute path key (with the `/api` prefix), as OpenAPI requires. |
| `operations` | A map of method → operation object, in standard OpenAPI form. |
| `source_file` | The fragment's own path, for error messages. |

**Validation rules** (each a hard failure):
- Every path key must correspond to at least one `RouteRecord`. A fragment describing a route that
  does not exist is the exact rot this feature exists to prevent, so it fails rather than being
  ignored.
- Every method key must be one of the eight operation verbs.
- Two fragments may not declare the same `(path, method)`; ambiguity fails rather than being resolved
  by file order.

**Precedence**: a sidecar operation **replaces** the upstream layer's operation for the same
`(path, method)` as a whole (not a field-by-field patch). Field-level merging across two documents
that independently describe the same endpoint produces results nobody can predict from reading
either file; whole-operation replacement is the rule a maintainer can hold in their head. Where a
maintainer wants to keep an upstream field, they copy it into the fragment — which also makes the
divergence visible in review.

---

## 5. `OperationEntry`

One operation in the generated document, after the three layers have been applied.

| Field | Source | Notes |
|---|---|---|
| `operationId` | derived | From the handler name; disambiguated with the path when two handlers share a name. Must be unique across the document. |
| `tags` | derived | `[module]`, unless the layer that supplied the operation set its own. |
| `summary` | layer, else derived | First sentence of the handler doc comment, or the handler name when there is no doc comment. |
| `description` | layer, else derived | Remaining handler doc comment, plus a note when the operation is auto-derived. |
| `parameters`, `requestBody`, `responses` | layer (upstream or sidecar) | Present only when the supplying layer provided them. Never invented. |
| `security` | document default | Never per-operation; see `openapi-document.md`. |
| `x-lanrurugi-source` | derived | `file:line` of the `.route()` registration. |
| `x-lanrurugi-schema-source` | derived | `legacy` \| `sidecar` \| `none` — which layer supplied request/response detail. |
| `x-lanrurugi-undocumented` | derived | `true` when `x-lanrurugi-schema-source` is `none`, so a reader (and the coverage report) can tell an auto-listed endpoint from a described one at a glance. |

**State transitions**: none — this is a build-time value, produced once per run.

---

## 6. `CoverageReport`

The per-build accounting. Printed on every run, including successful ones; a gate that fails silently
is not a gate.

| Figure | Meaning | Gate behaviour |
|---|---|---|
| `routes_registered` | Count of distinct `(method, absolute_path)` in the route table. | — |
| `routes_described` | Count present in the output. | Must equal `routes_registered`; a mismatch is a hard failure (it should be impossible, which is why it is asserted). |
| `by_schema_source.legacy` | Operations whose detail came from the upstream document. | — |
| `by_schema_source.sidecar` | Operations with hand-written detail. | — |
| `by_schema_source.none` | Operations currently auto-listed without request/response detail. | Reported; not a failure. This is the visible backlog. |
| `upstream_only_paths` | Upstream paths this server does not register. | Reported as a warning list; not a failure (some may be deliberate). |
| `unresolved_routes` | Routes the scanner could not turn into a `RouteRecord`. | Hard failure, naming file and line. |
| `stale_sidecar_entries` | Sidecar `(path, method)` pairs with no matching route. | Hard failure, naming the fragment file. |

---

## 7. `GeneratedApiDocument`

The single artifact every rendered form of the API reference is a view of.

- **Shape**: defined in `contracts/openapi-document.md`.
- **Written to**: `docs/public/openapi.json` (gitignored). CI additionally uploads it as a build
  artifact so the docs job and any future consumer use the same bytes.
- **Determinism**: the emitter sorts paths and methods, so two runs over unchanged sources produce
  byte-identical output. Without this, every CI run would produce a different artifact and the
  "same revision" guarantee would be unverifiable by inspection.

---

## 8. `SdkArtifacts`

Generated from the plugin dispatcher's TypeScript sources by Deno's own tooling.

| Artifact | Command | Written to | Path normalisation |
|---|---|---|---|
| Browsable reference | `deno doc --html` | `docs/dist-sdk/` | Not required — verified to contain no absolute paths. |
| Machine-readable reference | `deno doc --json` | `docs/dist-sdk/plugin-sdk.json` | Required — verified to embed `file:///<absolute path>` node keys. |

Both are gitignored build outputs. The HTML tree is self-contained (no network fetch at page-view
time), which is what allows the same site to be read offline from a running instance.

---

## 9. `DocsSiteBuild`

| Field | Notes |
|---|---|
| `base` | `/` for the public deployment, `/docs/` for the copy shipped inside the server image. From an environment variable; one source, two builds. |
| `source` | `docs/src/content/docs/**` (hand-written prose) plus the generated `docs/public/**`. |
| `artifacts_consumed` | `docs/public/openapi.json` — produced by an earlier stage, never by the site build itself. (`docs/dist-sdk/**` and `docs/dist-swagger/**` are the other two deliverables, built by their own tasks; the site reaches the SDK through `DOCS_SDK_URL`.) |
| `output` | `docs/dist/` (gitignored). In the image, this plus the SDK and Swagger outputs are composed under `LANRURUGI_DOCS_DIR`; on Cloudflare Pages the three are separate projects. |
| `excluded` | `docs/api/**` (generator inputs, not site content) and `specs/**` (the project's own design record — FR-028). |

**Invariant**: the site build never generates documentation content. It only renders what earlier
stages produced, which is what keeps the published site and the offline copy from being two
independently maintained documents (FR-001).
