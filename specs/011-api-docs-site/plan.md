> **Superseded in part (2026-10-11) — see [`contracts/deliverables.md`](./contracts/deliverables.md).**
> The rendering decisions recorded in this file were revisited after comparing the result against
> the reference documentation site: the whole-page Scalar viewer was replaced by generated
> per-group pages, and the API document now also feeds a standalone Swagger UI. Where this file
> still says `@scalar/starlight` or `docs/public/sdk/`, the deliverable contract is authoritative.

# Implementation Plan: Developer Documentation Site with Generated API Reference

**Branch**: `011-api-docs-site` | **Date**: 2026-10-11 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/011-api-docs-site/spec.md`

## Summary

Two deliverables, one pipeline. First, a new workspace binary `lanrurugi-api-docgen` that reads the
Axum route registrations directly out of `crates/lanrurugi-api/src/**` and
`crates/lanrurugi-server/src/app.rs`, merges them with (a) the vendored upstream LANraragi OpenAPI
3.1 document and (b) a hand-written sidecar of OpenAPI fragments, and emits one OpenAPI 3.1 JSON
document covering every route the server registers. It fails the build on any route it cannot
resolve and on any sidecar entry that names a route that does not exist. Second, a static
documentation site under `docs/` (Astro Starlight) that renders that JSON as generated
per-group pages with a built-in request panel,
and generated code samples (including cURL), plus a `deno doc`-generated plugin SDK reference and
hand-written guide pages. The site is published to Cloudflare Pages from CI and shipped inside the
Docker image at `/docs` for offline reading. One incidental defect fix rides along: the `enablecors`
setting's `Access-Control-Allow-Headers` omits `Content-Type`, so a browser preflight for any
JSON-bodied request is rejected and the setting does not actually work.

## Technical Context

**Language/Version**: Rust (workspace toolchain, pinned in `.mise.toml`) for the generator;
TypeScript/Node 24.18.0 + pnpm 11.10.0 for the docs site; Deno 2.9.1 for the plugin SDK reference.

**Primary Dependencies**:
- Generator: `syn` (with `full` + `visit` features) and `proc-macro2` for AST parsing,
  `serde`/`serde_json` for the emitted document, `serde-saphyr` 1.x for reading the vendored YAML
  and the sidecar, `clap` for the CLI. *No* dependency on `lanrurugi-api` itself — the generator is
  a text/AST consumer, so it links nothing from the product and builds in seconds.
- Docs site: `astro` 7.3.8, `@astrojs/starlight` 0.42.6. Swagger UI is a separate deliverable
  (`swagger-ui-dist`); the prose API pages are generated from the document rather than handed to an
  embedded viewer.

**Storage**: None. The generator reads repository files and writes one JSON file. The docs site is
static output. No Redis or database involvement anywhere in this feature.

**Testing**: `cargo test -p lanrurugi-api-docgen` — unit tests over embedded source fixtures for the
AST scanner (literal path, multi-method chain, `axum::routing::` qualified form, test-module
exclusion, non-literal path rejection, unrecognised method-router rejection) and integration tests
over the merge pipeline (upstream-path prefixing, sidecar override precedence, stale-entry
detection, coverage accounting). Plus the binary's own `--check` run against the real repository as
the end-to-end gate in CI, and `docs/`'s `astro build` as the site's own build check.

**Target Platform**: Linux, both CI (GitHub Actions, ubuntu-latest) and the existing Debian-slim
Docker runtime image. The docs site additionally targets Cloudflare Pages' static host.

**Project Type**: Web application (existing monorepo). This feature adds one Cargo workspace member
(a build-time binary, never linked into the server) and one pnpm workspace package (a static site,
never shipped to the browser as part of the product SPA).

**Performance Goals**: The generator is a build-time tool, not a request-path component: it must
complete in a few seconds over ~80 source files and ~1,000 lines of YAML, because it runs on every
CI run and on every local `mise run docs-api`. No runtime performance characteristic of the server
changes — the single `cors.rs` edit adds one header to responses only when `enablecors` is on.

**Constraints**:
- Must not require the Rust workspace to link `lanrurugi-api` (no Redis, no `AppState`, no model
  assets) — the gate has to be runnable cheaply and independently of the full build.
- Must not fetch anything at generation time: the upstream document is vendored, so generation and
  the gate work offline and are reproducible. Refreshing the vendored copy is a separate, deliberate
  task.
- Must not introduce a second product frontend or a second deployable: the docs site is static
  output built from the same repository.
- Must not proxy or observe readers' API requests; no server-side component exists in the try-it
  path at all.
- The site must build correctly both at a domain root (Cloudflare Pages) and under the `/docs`
  sub-path the server already mounts (`LANRURUGI_DOCS_DIR`).
- Hand-written spec-kit artifacts (`specs/**`) must not be published.

**Scale/Scope**: ~193 distinct production paths / ~238 (path, method) pairs currently registered,
across 41 router files; 64 paths in the vendored upstream document, of which 55 are shared and 9 are
upstream-only (must be reported). Roughly 30–40 hand-written sidecar entries in this tranche; the
remainder are emitted as auto-derived operations carrying the handler's own doc comment and a source
pointer. Docs site: ~6–8 hand-written pages plus the generated API and SDK references.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Assessment |
|---|---|
| **I. Legacy Data & User-Trust Compatibility** | Not engaged. Nothing in this feature reads, writes, or migrates persisted data, changes the archive-identity algorithm, or touches anything Redis-stored. |
| **II. API Contract Fidelity (Phase 1)** | **Engaged and satisfied.** This feature is deliberately additive: it changes no endpoint's path, method, request shape, or response shape, and the generated document is a *description* of the current surface rather than a change to it. The one behavioural edit (`cors.rs`) alters a response header on an existing opt-in setting; the setting's default stays off, and no endpoint's behaviour changes when it is off. Two points deserve explicit sign-off per the constitution's "compatibility-affecting changes require explicit sign-off" rule: (a) the generated document marks every `/api/*` operation as Bearer-authenticated rather than reproducing upstream's per-operation `security` entries — correct, because the project has already documented a deliberate pre-release break from legacy auth semantics (constitution Principle II's own note), and reproducing upstream's `security: []` on `/search` would actively misdescribe this server; (b) nine upstream paths are not served by this server and are therefore reported as drift rather than published — this is a finding for maintainers, not a change in behaviour. |
| **III. Resource-Conscious, Genuinely Concurrent Single-Process Architecture** | Satisfied by construction. The generator is a separate build-time binary that links nothing from the product; it introduces no runtime process, no background task, and no in-server work. The docs site is static. No CPU-bound parallel work is added, so the rayon-capping rule has nothing to apply to. The only runtime artefact is the static docs directory copied into the image, replacing the smaller SDK-only directory already there. |
| **IV. Sandboxed, Language-Agnostic Plugin Extensibility** | Not engaged; no plugin-protocol change. The SDK reference is generated from the same TypeScript sources the dispatcher already loads, by Deno's own tooling. |
| **V. Secrets & Network Trust Boundaries** | **Engaged and kept.** API tokens entered in the reference stay in the reader's browser and are sent only to the instance address the reader typed. No project-run proxy exists, so no token or internal address ever reaches project infrastructure. The generated document carries no secrets. The Cloudflare Pages deployment credential is a CI secret, used only by the deploy step, and the step is skipped when it is absent. |
| **VI. Phased Scope Discipline** | Satisfied. No Phase 2 concern is pulled forward: the translation endpoints that already exist are documented by the same mechanism as everything else, and nothing here blocks or gates Phase 1 delivery. Documenting existing endpoints is not new Phase 2 scope. |
| **VII. Frontend Engineering Discipline & Legacy UI Fidelity** | Not engaged in the "ported legacy UI" sense — the docs site is a new site, not a reproduction of a legacy LANraragi view, so the computed-style verification procedure has no reference to diff against. The docs site is also not the product frontend, so the `index.tsx`-composition and breakpoint-CSS rules do not apply to it; its own conventions follow the Starlight framework. The site does reproduce a *third-party* interaction pattern (GitBook's Scalar-backed API block), and does so by embedding the same underlying component rather than by imitating its markup. |
| **Repo layout: single monorepo, one frontend app** | **Engaged — see Complexity Tracking.** The docs site is a static site in the same repository, under `docs/`, and is a pnpm workspace package. It is not a second instance of the product's React app and shares no code with it. |
| **Data store: Redis only** | Satisfied; nothing is persisted. |
| **Dependencies: latest stable, verified at implementation time** | Applied and recorded in `research.md`: every version above was read from crates.io/npm during this planning run, and `serde-saphyr` was selected over the deprecated `serde_yaml`/`serde_yml` only after checking every candidate YAML crate live. |
| **Quality gates: `cargo check` + fmt, eslint, CI as the authoritative gate** | Satisfied: the new crate is a workspace member, so `cargo check --workspace`/`clippy`/`fmt` cover it; the docs package is added to the frontend job's scope for lint/type-check; the new `docs-generate` CI job is the enforcement point for the coverage gate. |
| **Agent communication language** | Chat replies in Chinese; this and every other artifact in `specs/011-api-docs-site/` in English, per the constitution's own carve-out. |

**Gate result: PASS.** One entry in Complexity Tracking (the `docs/` static site versus the "one
frontend app" layout constraint). No principle is overridden and no amendment is required.

## Project Structure

### Documentation (this feature)

```text
specs/011-api-docs-site/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
│   ├── docgen-cli.md            # the generator's CLI + exit-code contract
│   ├── openapi-document.md      # the shape of the emitted document
│   ├── sidecar-format.md        # the hand-written fragment format + merge precedence
│   └── docs-site.md             # the site's build/deploy contract (paths, base, artifacts)
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-api/                     # existing — READ-ONLY input to the generator
│   └── src/**.rs                      #   route registrations live here
├── lanrurugi-server/
│   └── src/app.rs                     # existing — READ-ONLY input (nest/merge mount points)
└── lanrurugi-api-docgen/              # NEW workspace member (build-time binary)
    ├── Cargo.toml
    ├── src/
    │   ├── main.rs                    # CLI, orchestration, exit codes
    │   ├── scan/
    │   │   ├── mod.rs                 # AST walk → RouteRecord set
    │   │   ├── routes.rs              # .route() extraction + method-router decoding
    │   │   ├── handlers.rs            # handler fn doc comments + source locations
    │   │   └── mount.rs               # router-fn → mount-prefix table + consistency assertions
    │   ├── legacy.rs                  # vendored upstream document: parse, prefix, filter, report
    │   ├── sidecar.rs                 # fragment loading + validation
    │   ├── merge.rs                   # three-layer merge + coverage accounting
    │   ├── emit.rs                    # OpenAPI 3.1 document assembly + JSON output
    │   └── validate.rs                # the --check gates
    └── tests/
        ├── scan_fixtures.rs           # scanner behaviour over fixture sources
        └── merge_pipeline.rs          # layering/precedence/coverage

docs/                                  # NEW pnpm workspace package `lanrurugi-docs`
├── package.json                       # astro + starlight (+ swagger-ui-dist for the Swagger deliverable)
├── astro.config.mjs                   # base/site/sdk-url from env; generated sidebar
├── tsconfig.json
├── api/                               # generator INPUTS (never part of the built site)
│   ├── legacy-openapi.yaml            # vendored upstream (MIT), refreshed deliberately
│   └── sidecar/                       # one file per API group
├── public/
│   ├── openapi.json                   # GENERATED (gitignored)
│   └── sdk/                           # GENERATED (gitignored): HTML reference + normalised JSON
├── src/
│   ├── content/docs/                  # hand-written Chinese prose pages
│   └── styles/custom.css
└── dist/                              # build output (gitignored)

scripts/
└── normalize-sdk-doc-json.mjs         # strips absolute paths from `deno doc --json` output

.github/workflows/ci.yml               # + docs-generate (Rust gate) and docs-build (site + deploy)
Dockerfile                             # rust-builder emits openapi.json; docs-builder builds the site
.mise.toml                             # + docs-api / docs-sdk / docs-build / docs-dev tasks
pnpm-workspace.yaml                    # + "docs"
.gitignore                             # + docs/public/openapi.json, docs/dist/, docs/dist-swagger/, docs/dist-sdk/
```

**Structure Decision**: The generator is a new Cargo workspace member rather than a script, because
it must be covered by the same `cargo check`/`clippy`/`fmt`/`test` gates as the rest of the
workspace and because it is the enforcement point for a constitution-level guarantee (documentation
truthfulness). It deliberately does *not* depend on `lanrurugi-api`: it consumes that crate's
*source text* rather than its symbols, which is what lets the gate run without linking Redis, model
assets, or the rest of the product — and is also what keeps it honest, since it sees what is
actually registered rather than what a refactor intended.

The docs site lives under `docs/` rather than `apps/docs/`: `docs/` is the constitution's designated
documentation location and its listing describes no deployable application. It is a pnpm workspace
package (so `pnpm install --frozen-lockfile` and `--filter` work exactly as the existing frontend
job already does) but deliberately not an `apps/*` entry, so it cannot be mistaken for a second
product app.

The three-layer merge (upstream document → sidecar → route scan) puts the route scan *last* and
treats it as the completeness authority. That ordering is the whole design: a layer that can only
add or enrich entries can never remove an endpoint from the output, so "the reference lists
something the server does not serve" and "the server serves something the reference does not list"
both become structurally impossible, and the gates only need to catch the residue — unparseable
routes, stale hand-written entries, and drift against the upstream document.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|-----------|------------|-------------------------------------|
| A second pnpm workspace package and a second static-site build target, alongside the "one frontend app" layout constraint (Technology Stack Constraints) | The feature's entire deliverable is a public documentation site. A documentation site is not a product frontend: it shares no code with `apps/frontend`, ships no product UI, adds no runtime dependency to the server, and is never loaded as part of the application. Serving prose documentation out of the product SPA would mean building a docs framework into the React app — strictly more coupling, not less. | (a) *Put the docs inside `apps/frontend`* — rejected: Starlight/Astro and the React SPA are different build systems; hosting one inside the other means maintaining a second pipeline inside the app and pulling Astro into the product's dependency graph. (b) *A separate repository* — rejected: the constitution's repository-layout constraint forbids it outright, and the coverage gate's whole value depends on the generator and the route source being validated at the same revision in the same pipeline. (c) *Publish the upstream OpenAPI document as-is* — rejected: it covers 64 paths against a server that registers ~193, and it does not describe this server's auth semantics; publishing it would be documentation that is wrong in a way that looks authoritative. |

**Post-Phase-1 re-evaluation**: unchanged. Adding the sidecar format, the CLI contract, and the
site's build contract introduced no new dependency on the product's runtime, no persisted state, and
no Phase 2 concern. One addition worth recording: the docs site's build is now reachable from the
Docker image build as well as CI. That is a build-graph edge, not a runtime one, and it reuses a
toolchain the image already installs (Node for the frontend, Deno for plugins).
