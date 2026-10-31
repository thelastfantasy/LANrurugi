> **Revisited (2026-10-11).** The framework choice below (Starlight) stands; the *renderer*
> choice did not. Embedding Scalar's whole-page reference was measured against the reference
> documentation site and replaced by generated per-group pages — see
> [`contracts/deliverables.md`](./contracts/deliverables.md) for what shipped and why the embedded
> viewer was insufficient (no per-group pages, no real heading anchors for an outline, a sidebar of
> internal module names, and no answer to "what shape is this response?" — that last one is now the
> separate Swagger UI deliverable).

# Phase 0 Research: Developer Documentation Site with Generated API Reference

**Feature**: `011-api-docs-site` | **Date**: 2026-10-11

Every decision below was checked against live sources during this planning run — the repository's
own source and lockfile, the crate/npm registries, the upstream LANraragi repository, and the
installed toolchain — rather than against memory or an earlier discussion. Findings that
contradicted a prior assumption are called out explicitly, because two of them changed the design.

---

## 1. How the route table is obtained

**Decision**: A new build-time binary parses the Rust source of `crates/lanrurugi-api/src/**` and
`crates/lanrurugi-server/src/app.rs` with `syn`, extracting every `.route(<path>, <method-router>)`
call, and combines that with a small explicit mount table. Runtime introspection is not used.

**Rationale**:

- Axum 0.8.9's `Router` has **no route-enumeration API**. Verified directly against the vendored
  crate source (`~/.cargo/registry/src/.../axum-0.8.9/src/routing/mod.rs`): the only introspection
  method is `pub fn has_routes(&self) -> bool`. There is no `routes()`, no iterator, nothing that
  yields the registered paths. A runtime dumper is therefore not merely inconvenient — it does not
  exist.
- The scan must be **AST-based, not textual**. The naive approach was tested first and produced
  false positives: `grep`-style matching finds `.route(` inside ordinary `//` comments (four such
  comments exist today, e.g. `plugins.rs`'s explanation of a moved path parameter), which a regex
  scanner would turn into phantom endpoints.
- The scan must **exclude test code**. This is not hypothetical: `plugins.rs` contains two routes
  whose path is built at runtime (`&format!("/{first_filename}")`, `&format!("/{second_filename}")`)
  inside a `#[tokio::test]`-local `axum::Router` used as a fake file server. A scanner that ignored
  test modules would either report those as real endpoints or fail on them as unresolvable. Both
  outcomes are wrong. **This finding contradicts a premise carried over from the earlier design
  discussion** ("all 130 `.route()` calls are string literals, no dynamic concatenation"): that was
  true of the production code then and is still true of the production code now, but it was never
  true of the file as a whole, and the scanner has to know the difference.
- Every *production* route in the repository is currently a string literal, so the parser's
  non-literal branch is a guard rail rather than a routine path — which is exactly what the spec's
  FR-004 wants it to be.

**Alternatives considered**:

- *Runtime introspection* — rejected: no such API exists in axum 0.8.9 (verified above).
- *Regex/text scanning* — rejected: produces phantom routes from comments (verified) and cannot
  reliably pair a path with its method chain across multi-line calls.
- *Making the route table data rather than code* (a static declaration list that both the router and
  the generator read) — rejected: it is a rewrite of ~238 route registrations across 41 files, and it
  moves the source of truth away from the code that actually serves the request, which is the
  opposite of the feature's intent. It would also make it possible for the two to disagree, which is
  the failure mode being eliminated.
- *A `tower`/`Router` seam that records registrations as they are added* (a feature-gated wrapper
  around `.route()`) — rejected: it requires touching every router file, and a wrapper only records
  what the wrapper is called for; a route registered some other way would silently vanish, which is
  precisely the silent-omission failure the spec forbids.

## 2. Mount points: why a table, and what makes it safe

**Decision**: The generator carries an explicit table mapping each discovered router factory
function to its mount prefix, and asserts that the table exactly matches what the source actually
does — in both directions.

**Rationale**: Paths declared inside module `router()` functions are relative to where they are
mounted, not absolute. Verified from the source: `app.rs` builds
`Router::new().nest("/api", api).merge(health::router()).merge(opensearch::router())`, where `api`
is `login::router()` merged with `settings::public_router()`, `misc::public_router()`,
`version::public_router()` and `lanrurugi_api::router()` (which itself merges 30 further module
routers). So every route in those 34 routers is served at `/api/<declared path>`, while
`health::router()` (`/health`) and `opensearch::router()` (`/opensearch.xml`) are served bare.
Publishing the declared paths unprefixed would be wrong for ~236 of ~238 routes; publishing the bare
two under `/api` would be wrong for the other two, and both are silent, plausible-looking errors.

The two consistency assertions are what keep the table from rotting: if a new router factory is
merged in `app.rs` or `lib.rs` but is missing from the table, generation fails; if the table names a
router factory that is not merged anywhere, generation fails too. A new endpoint therefore cannot
be added to the server without either landing under a known prefix or breaking the build loudly.

**Alternatives considered**:

- *Fully resolving the merge/nest expression graph* — considered and deferred: it is more machinery
  than the problem needs (there is exactly one nesting level and a flat merge structure), and the
  table plus two-way assertions already guarantee no route is unaccounted for. If a second nesting
  level ever appears, the resolver can be added without changing the emitted document.
- *Assuming a blanket `/api` prefix for the whole crate* — rejected: it would silently mis-prefix
  `health.rs` and `opensearch.rs`, which live in the same crate but are mounted bare.
- *Reading prefixes out of `policy/route_policy.csv`* — rejected: that file is authorization policy,
  not a route inventory. It contains 57 unique paths including wildcards and explicit `deny` rules,
  and it does not enumerate every route; using it as the source of truth would fail the completeness
  requirement outright.

## 3. YAML parsing dependency

**Decision**: `serde-saphyr` 1.3.0.

**Rationale**: The generator must read two YAML inputs — the vendored upstream OpenAPI document and
the hand-written sidecar fragments. All candidates were checked live on crates.io:

| Crate | Latest | Last published | Assessment |
|---|---|---|---|
| `serde_yaml` | 0.9.34+deprecated | 2024-03 | Deprecated in its own version string. Rejected. |
| `serde_yml` | 0.0.13 | 2026-05 | Its own description says it is **unmaintained** and forwards to another crate. Rejected. |
| `serde_norway` | 0.9.42 | 2024-12 | A stable fork of `serde_yaml`, but untouched for ~22 months. Viable, not preferred. |
| `noyalib` | 0.0.57 | 2026-10 | Actively developed and serde-integrated, but `0.0.x` with 57 patch releases — high API-churn risk in a pinned build tool. |
| `yaml-rust2` / `saphyr` | 0.13.0 / 0.1.0 | 2026-09 | Actively maintained and YAML 1.2 compliant, but expose their own value model and ship **no serde integration** — adopting them means hand-writing a value-tree converter. |
| **`serde-saphyr`** | **1.3.0** | **2026-09** | Built on `saphyr` (YAML 1.2 compliant), serde-integrated, past `1.0` so the API is settled, and actively released. **Chosen.** |

YAML 1.2 compliance matters here specifically because OpenAPI 3.1's schemas are JSON Schema-shaped
and the upstream document is authored in YAML; a YAML 1.1 resolver's extra boolean/octal aliases are
a known source of silent value corruption in exactly this kind of document.

**Alternatives considered**: converting the vendored document to JSON once and committing the JSON —
rejected because it makes refreshing the vendored copy depend on an unpinned external conversion
tool, and because it destroys the ability to diff the vendored file against upstream verbatim
(provenance matters for an MIT-licensed third-party artefact that is being redistributed).

## 4. The upstream document is more useful than assumed

**Decision**: Vendor the upstream LANraragi `tools/openapi.yaml` as the first documentation layer,
rewriting only its path prefix and its auth section.

**Rationale**: Checked live against `https://raw.githubusercontent.com/Difegue/LANraragi/dev/tools/openapi.yaml`:

- `openapi: 3.1.0` — the same major version this feature emits, so no dialect translation is needed.
- **64 paths / 87 operations**, with **84 of 87 operations carrying a JSON response schema**, plus
  parameters, constraints, summaries and descriptions.
- `servers: [{url: "https://lrr.tvc-16.science/api"}]` — paths are declared *relative to an `/api`
  prefix*, which matches this server's mount point exactly, so prefixing is a literal string
  operation rather than an inference.
- `info.license: {name: MIT, identifier: MIT}`, corroborated by the GitHub API (`spdx_id: mit`), so
  redistributing a vendored copy inside this MIT project is unencumbered.
- Its `components.securitySchemes` contains only `api_key`, and operations carry per-operation
  `security` entries (including `security: []` on `/search`) — these are **wrong for this server**
  and must be replaced rather than inherited (see Constitution Check, Principle II).

Measured overlap against the current route table: **55 paths shared**, **9 upstream-only**
(`/categories/bookmark_link`, `/categories/bookmark_link/{id}`, `/plugins/install`,
`/plugins/installed/{plugin_namespace}`, `/registries`, `/registries/default_registry`,
`/registries/default_registry/{id}`, `/registries/{id}`, `/registries/{id}/refresh` — all verified
absent from the router source), and **~138 LANrurugi-only**. So 55 of ~193 paths get complete
field-level documentation for free, and the 9 upstream-only paths become a real, reportable finding
rather than a guess.

**Alternatives considered**: fetching the upstream document at generation time — rejected per the
spec's reproducibility constraint (offline builds, and an upstream edit must not silently change
what this project publishes). Note also that a prior measurement of this file recorded 57 paths;
the current revision has 64, which is itself an argument for vendoring at a known revision rather
than tracking a moving URL.

## 5. What the second layer (sidecar) is, and what the third layer contributes

**Decision**: The sidecar is a set of hand-written **OpenAPI fragments** (`"/path": {get: {...}}`
maps), one file per API group. The third layer — the route scan — contributes, for any operation the
first two layers did not describe, an auto-derived entry built from facts already in the source: the
handler's own `///` doc comment, the module name as a tag, and a `file:line` source pointer.

**Rationale**: Using the real fragment format rather than an invented flat key structure means the
sidecar is validated by the same tooling as any OpenAPI document, gets editor completion, and can
carry a full schema later without a format change.

The third layer's auto-derived content is the part worth justifying. Handler functions in this
codebase reliably carry `///` doc comments that describe real behaviour — verified examples include
`jobs.rs`'s `list_jobs` (`/// \`GET /jobs\` — every tracked job, most-recently-created first ...`,
including the note about which fields are omitted when absent) and `activity.rs`'s constants'
explanatory comments. Surfacing those comments costs nothing, cannot drift (they are the same text
the maintainer already maintains), and turns what would otherwise be several hundred bare
`GET /path` stubs into entries that actually tell a reader something. This is the difference between
the skeleton being a placeholder and being a useful first draft.

Auto-derived operations are marked distinctly from hand-written and upstream-sourced ones (via
`x-lanrurugi-schema-source` and a `x-lanrurugi-undocumented` flag), so the coverage report is honest
about which endpoints still lack request/response detail.

**Alternatives considered**: requiring a hand-written entry for every one of the ~138
LANrurugi-only paths before the gate passes — rejected as both unrealistic (inventing response
schemas for ~138 handlers that return anonymous `json!({...})` values would be fabrication, not
documentation) and contrary to the spec, which requires completeness of *listing* and only requires
detail to be *addable*.

## 6. Documentation site framework

**Decision**: Astro Starlight + `@scalar/starlight`.

**Rationale**: The reference UI the user asked to match (the LANraragi GitBook page) is not built by
GitBook at all — it is **GitBook embedding Scalar**. Verified by fetching and inspecting that page:
it carries `scalarRuntimeURL: .../scalar/scalar-api-client@…mjs`, a `<button
class="scalar-activate-button">Test it</button>`, and per-operation Scalar extensions
(`x-hideTryItPanel`, `x-enable-proxy`). So embedding Scalar produces the same API block by
construction, and the remaining work is only the surrounding site chrome.

Scalar publishes official integrations for Starlight and Docusaurus (and plain Astro/React/Vue/HTML),
but **not** for VitePress — checked against the integration directory of the Scalar repository.
Between the two:

- Starlight's default layout (left sidebar with groups, top nav with search, right "on this page"
  outline, dark mode, built-in i18n, Pagefind search) is the closer match to the reference site, and
  its plugin is a one-line addition.
- Docusaurus's advantages are built-in document versioning and more mature i18n. The reference page
  does show a version dropdown, but this project has no released version to pin yet, and the
  constitution's own advice is to prefer the lighter option until the need is real.

Versioned docs can be added later with the community `starlight-versions` plugin if a second release
line ever exists.

**Alternatives considered**: *GitBook SaaS* — pixel-perfect but consumes a spec by URL or Git sync,
which cannot participate in this repository's same-revision generation and gate, and adds a
third-party account to the critical path; rejected. *Docusaurus* — viable, rejected only on weight
and proximity of the default theme. *VitePress* — rejected: no official Scalar integration.

Versions pinned (read live from npm): `astro` 7.3.8, `@astrojs/starlight` 0.42.6,
`@scalar/starlight` 0.2.13.

## 7. What the API reference must do to satisfy "test it"

**Decision**: Rely on Scalar's built-in UI for the instance address, the token, and the code
samples; configure the document with a `bearerAuth` security scheme and a `localhost` default
server; do not configure a proxy; document the CORS and mixed-content preconditions in a prose page.

**Rationale**, each point verified rather than assumed:

- *In-page server input.* Scalar's API client ships a `serverDropdown` with `addServer` /
  `updateServers` actions — confirmed by reading the published package's own localisation files,
  which include the Chinese strings "服务器：", "添加服务器", "更新服务器". The reader types an address;
  nothing is edited on disk. This satisfies FR-011 directly.
- *In-page token.* The same localisation set carries `requestAuthTab` / `bearerToken` labels, i.e.
  an auth panel that takes a Bearer token.
- *Copy-as-cURL.* `@scalar/api-reference` depends on `@scalar/snippetz`, Scalar's code-sample
  generator, which is what produces the multi-language sample panel (including cURL). This is the
  fallback FR-014 requires, and it is generated client-side from the operation, so it works even
  when the request itself cannot be sent.
- *No external fetch at page-view time.* `@scalar/starlight` depends on `@scalar/astro` and
  `@scalar/client-side-rendering` and renders `<ScalarComponent renderMode="client">` from the
  installed packages — not from a CDN `<script src>`. This is what keeps FR-023 (offline reading)
  true. It also handles the site's `base` prefix internally (it strips `import.meta.env.BASE_URL`
  before resolving the reference route), which is what makes the `/docs` sub-path deployment work.
- *Default server.* The server's default bind is `0.0.0.0:3000` (verified in
  `crates/lanrurugi-server/src/main.rs`, `LANRURUGI_BIND`), so `http://localhost:3000` is the honest
  default. It is also the only default that can work from an HTTPS page without further setup:
  browsers treat `http://localhost` as a trustworthy origin and exempt it from mixed-content
  blocking, while `http://192.168.x.x` gets blocked outright. Documenting that distinction is part of
  FR-015.
- *No proxy.* The reference page's own `Test it` works because GitBook proxies the request through
  its own servers (`open-2c.gitbook.com/~scalar/proxy?allowed_origin=lrr.tvc-16.science/api`). That
  is defensible for one fixed public demo instance; it is not defensible for arbitrary self-hosted
  instances, where it would route readers' tokens and internal addresses through project
  infrastructure. Scalar's `proxyUrl` is therefore left unset, and `servers` carries only the
  localhost default (FR-013, FR-024).

**Alternatives considered**: pre-filling `servers` with a public demo address — rejected by explicit
product decision (spec Assumptions); operating our own proxy — rejected on Principle V grounds.

## 8. The one real defect this feature fixes

**Decision**: Add `Content-Type` to `Access-Control-Allow-Headers` in
`crates/lanrurugi-api/src/cors.rs`.

**Rationale**: Verified in the source: `const ALLOW_HEADERS: &str = "Authorization";` is the entire
value. `Content-Type` is a CORS-safelisted request header only for three values
(`application/x-www-form-urlencoded`, `multipart/form-data`, `text/plain`); `application/json` is
not among them, so any cross-origin `POST`/`PUT` with a JSON body triggers a preflight whose
`Access-Control-Request-Headers` includes `content-type`, and the browser refuses to send the real
request because the response does not allow it. The setting therefore only ever worked for
JSON-body-free requests. This is a genuine bug (the feature's whole premise, and every
integration-shaped client, depends on JSON bodies) and it is fixed here because the API reference's
try-it panel is the first consumer that makes it visible.

The fix is deliberately minimal — the setting's default stays off, no other header is added, and
nothing changes when the setting is off. `Accept` is not added: it is a safelisted header and is
never named in a preflight.

**Alternatives considered**: adding a broad `*` wildcard — rejected: it would allow arbitrary
headers on any origin that `Access-Control-Allow-Origin: *` already admits, which is a wider change
than the reported problem justifies.

## 9. Plugin SDK reference: HTML rather than JSON-only

**Decision**: Generate both, from the same sources, by the same command: `deno doc --html` (the
browsable reference, served as a static sub-path of the site) and `deno doc --json` (a
machine-readable artifact, path-normalised before it is written).

**Rationale**: Both outputs were produced live from the real SDK sources during this run.

- The **HTML** output is self-contained (its only external URL is the SVG XML namespace, not a
  network fetch) and — importantly — contains **no absolute filesystem paths at all**. Verified by
  grepping the generated tree for the workspace path: zero occurrences.
- The **JSON** output, by contrast, keys its `nodes` map on `file:///…` URLs and embeds absolute
  paths in its module documentation. Verified live: `file:///home/<user>/LANrurugi/crates/…`
  appears as a node key. So the JSON needs the normalisation step the earlier discussion called for,
  while the HTML does not.

This inverts part of the earlier plan, which had assumed the JSON artifact was the primary output
and needed scrubbing. Both are kept — the HTML is what a plugin author reads, the JSON is the
machine-readable artifact for tooling — but the normalisation is applied only where it is actually
required, and the HTML is used directly rather than being re-rendered through the docs framework.
Re-rendering would mean writing a `deno doc` JSON renderer, which is a substantial piece of UI work
that delivers nothing the tool's own output does not already deliver.

**Alternatives considered**: rendering the SDK reference as a native Starlight page from the
normalised JSON — rejected for this tranche as disproportionate; keeping only the HTML — rejected
because a machine-readable SDK artifact is the explicit ask in the upstream issue.

## 10. CI shape, publishing, and offline delivery

**Decision**: Two new CI jobs. `docs-generate` (Rust only) runs the generator in `--check` mode and
uploads the generated document plus the SDK artifacts. `docs-build` (Node) downloads them, builds the
site, and deploys to Cloudflare Pages on pushes to the release branch when the deployment secrets
are present. The same site build is also wired into the Docker image, under the existing
`LANRURUGI_DOCS_DIR` mount at `/docs`.

**Rationale**:

- Splitting the jobs keeps the *gate* (a cheap Rust-only run) separate from the *site build* (a Node
  build), so a failure tells a maintainer immediately which half broke, and so the gate can be the
  required check without dragging the Node toolchain into it.
- Deployment is credential-driven and skipped when the secrets are absent, so forks and PRs are
  unaffected and no CI run can fail merely because a third-party account is not configured. The spec
  deliberately scopes the deliverable to the pipeline rather than to a live URL.
- The image already has the pieces: a Node stage (`frontend-builder`) for a pnpm/Astro build and a
  Deno binary stage for the SDK reference. The existing `docs-builder` stage currently runs
  `deno doc --html` only and produces a small directory; it becomes a Node-based stage that also
  copies the Deno binary in, produces the SDK artifacts, consumes the generated `openapi.json` from
  the Rust builder stage, and emits the full static site. The runtime image still receives only
  static files, so nothing about the running server's footprint changes.
- The site needs two `base` values (`/` for Cloudflare Pages, `/docs/` for the in-image copy). Both
  are `astro build` invocations of the same source against the same document, driven by an
  environment variable — not two documents.

**Alternatives considered**: letting Cloudflare Pages run the build in its own container — rejected:
it cannot see the Rust job's generated artifact or the repository's pinned toolchain, and rebuilding
there would mean either committing the generated document or minting a second, divergent source of
truth. A single combined CI job — viable, and marginally less machinery, but it loses the
gate/build distinction above and makes the required-check story muddier.

## 11. Scale, and what is deliberately not done in this tranche

**Decision**: The first tranche ships the mechanism plus a bounded set of hand-written sidecar
entries (the API groups that a client most plausibly needs beyond the compatibility surface), with
the remainder emitted as auto-derived operations. The coverage report is what makes the remaining
work measurable, and completing it is a follow-on task rather than a gate.

**Rationale**: ~138 paths are LANrurugi-only. Most return anonymous `json!({...})` values, so writing
response schemas for them would mean inventing shapes rather than documenting them. The spec
requires that detail be *addable* (FR-009) and that the listing be *complete* (FR-003); it does not
require that every endpoint's schema be authored on day one. Shipping an honest, complete listing
with a visible marker on the endpoints that still lack detail is strictly better than either
publishing nothing or publishing invented schemas that look authoritative.

**Measured scale, for sizing**: 41 router files; ~193 distinct production paths; ~238 (path, method)
pairs; 64 upstream paths of which 55 are shared; 9 upstream-only paths to report.
