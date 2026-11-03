---

description: "Task list for 011-api-docs-site"
---

# Tasks: Developer Documentation Site with Generated API Reference

**Input**: Design documents from `/specs/011-api-docs-site/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Unit and integration tests for the generator are included (the scanner is pure logic over
parsed source and is exactly what breaks silently if untested; the project's constitution also
requires `cargo test` to be meaningful for every workspace member). No end-to-end browser test suite
is added — `quickstart.md`'s scenarios are manual, and the site's own `astro build` is its build
check.

**Organization**: Tasks are grouped by user story so each can be implemented, verified, and
demonstrated on its own.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: `US1`–`US6`, mapping to the user stories in `spec.md`
- Every task names the exact file path it touches

## Path Conventions

Repository root is the Cargo workspace and the pnpm workspace. Paths below are repository-relative.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Create the two new packages and the ignore rules, so nothing downstream has to invent
structure.

- [x] T001 Add `crates/lanrurugi-api-docgen` to `members` in `Cargo.toml` and create
      `crates/lanrurugi-api-docgen/Cargo.toml` declaring `syn` (features `full`, `visit`),
      `proc-macro2`, `serde`, `serde_json`, `serde-saphyr`, `clap` (derive), `anyhow`. Pin each to
      the latest stable release read live from crates.io at implementation time (constitution:
      dependencies verified at implementation time, not copied from this document). No dependency on
      `lanrurugi-api`.
- [x] T002 [P] Create the docgen module skeleton — `crates/lanrurugi-api-docgen/src/main.rs` plus
      `src/scan/{mod.rs,routes.rs,handlers.rs,mount.rs}`, `src/{legacy.rs,sidecar.rs,merge.rs,emit.rs,validate.rs}`
      — with type definitions and `todo!()`-free stubs so `cargo check -p lanrurugi-api-docgen`
      passes before any logic lands.
- [x] T003 [P] Add `"docs"` to `packages` in `pnpm-workspace.yaml` and create
      `docs/package.json` (name `lanrurugi-docs`, private) depending on `astro`,
      `@astrojs/starlight`, `@scalar/starlight` at the latest stable versions read live from npm.
      Run `pnpm install` from the repository root to regenerate `pnpm-lock.yaml` — required before
      any Docker build or CI run, since `pnpm install --frozen-lockfile` fails outright otherwise.
- [x] T004 [P] Update `.gitignore` for the generated artefacts: `docs/dist/`, `docs/.astro/`,
      `docs/node_modules/`, `docs/public/openapi.json`, `docs/dist-sdk/`, `docs/dist-swagger/`.
- [x] T005 [P] Create `docs/api/sidecar/` with a `.gitkeep` and `docs/api/legacy-openapi.yaml`'s
      target directory, so the generator's default input paths exist from the first run.

**Checkpoint**: `cargo check --workspace` passes with the new (empty) crate; `pnpm install` succeeds
with the new (empty) package.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The route scanner. Every user story reads its output, and none of them can be built
without it.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [x] T006 Implement route extraction in `crates/lanrurugi-api-docgen/src/scan/routes.rs`: walk each
      parsed file with a `syn::visit` visitor, and for every method call named `route`, take
      argument 0 as a path literal (rejecting anything else by file:line) and decode argument 1 into
      a method set by recognising `get`/`post`/`put`/`delete`/`patch`/`head`/`options` either bare or
      `axum::routing::`-qualified, including chained forms like `get(a).delete(b)`. Reject an
      expression that yields no recognisable verb, and reject `.nest(...)`/`.nest_service(...)`
      inside a scanned router factory.
- [x] T007 Implement test-code exclusion in `crates/lanrurugi-api-docgen/src/scan/mod.rs`: skip any
      item inside a module annotated `#[cfg(test)]` and any function annotated `#[test]` or
      `#[tokio::test]`. This is not defensive padding — `crates/lanrurugi-api/src/plugins.rs`
      contains a test-local `axum::Router` with runtime-computed paths, and including it would make
      the tool fail on the repository as it stands.
- [x] T008 [P] Implement handler metadata collection in
      `crates/lanrurugi-api-docgen/src/scan/handlers.rs`: for every handler name referenced by a
      route registration, find its function item in the same crate and capture its `///` doc comment
      (verbatim, markers stripped) and its own source location. A handler that cannot be found is a
      hard error naming the route.
- [x] T009 Implement the mount table and its assertions in
      `crates/lanrurugi-api-docgen/src/scan/mount.rs`: `health::router` and `opensearch::router` at
      prefix `""`; every other discovered router factory at `/api`. Assert both directions against
      the factories actually discovered in `crates/lanrurugi-api/src/**` and those merged in
      `crates/lanrurugi-server/src/app.rs` and `crates/lanrurugi-api/src/lib.rs` — a discovered
      factory missing from the table, or a table entry mounted nowhere, is an error.
- [x] T010 Wire the phases together in `crates/lanrurugi-api-docgen/src/scan/mod.rs`, producing the
      `RouteRecord` set per `data-model.md` §1, with `(method, absolute_path)` uniqueness enforced.
- [x] T011 Write scanner unit tests in
      `crates/lanrurugi-api-docgen/tests/scan_fixtures.rs` over inline fixture sources covering:
      a literal single-verb route; a multi-line multi-verb chain; the `axum::routing::post(...)`
      qualified form; `.route(` text inside a comment being ignored; a `#[cfg(test)]` module and a
      `#[tokio::test]` function being ignored; a runtime-computed path being rejected with a
      file:line message; a method-router expression with no recognisable verb being rejected; a
      `.nest()` inside a router factory being rejected.

**Checkpoint**: `cargo test -p lanrurugi-api-docgen` passes, and a debug run against the real
`crates/lanrurugi-api/src` reports the same route count a human gets by inspection.

---

## Phase 3: User Story 1 — Every endpoint the server serves is documented (Priority: P1) 🎯 MVP

**Goal**: One generated OpenAPI 3.1 document that lists every registered route at its true absolute
path, with each operation carrying the handler's own doc comment and a source pointer.

**Independent Test**: generate the document, then confirm the set of `(path, method)` pairs in it
equals the set the scanner extracted, that every path is absolute, and that the two deliberately
bare routes are not under `/api`. (`quickstart.md` Scenarios 1 and 2.)

- [x] T012 [US1] Implement document assembly in `crates/lanrurugi-api-docgen/src/emit.rs` per
      `contracts/openapi-document.md`: `openapi: 3.1.0`, `info` (title, workspace crate version,
      MIT license, a description stating the document is generated from the route table), a single
      placeholder server `http://localhost:3000`, document-level `security: [{bearerAuth: []}]`,
      `components.securitySchemes.bearerAuth`, and one tag per module that has routes.
- [x] T013 [US1] Implement the route layer of `crates/lanrurugi-api-docgen/src/merge.rs`: for each
      `RouteRecord` not yet claimed by another layer, build an operation with `operationId` derived
      from the handler name (path-suffixed on collision), `tags` from the module, `summary`/`description`
      from the handler's own doc comment (or the handler name verbatim when there is none), a
      non-empty `responses`, and `x-lanrurugi-source` / `x-lanrurugi-schema-source: "none"` /
      `x-lanrurugi-undocumented: true`. No schema is invented.
- [x] T014 [US1] Implement ordering and serialisation in `crates/lanrurugi-api-docgen/src/emit.rs`:
      sort paths and methods, sort JSON keys, and emit pretty-printed JSON so two runs over unchanged
      sources are byte-identical (`contracts/docgen-cli.md`'s determinism requirement). Strip the
      `--repository` prefix from every emitted source path so no absolute path can appear.
- [x] T015 [US1] Implement the CLI in `crates/lanrurugi-api-docgen/src/main.rs` with the flags,
      defaults, and exit codes of `contracts/docgen-cli.md` (`--output`, `--api-src`, `--app-src`,
      `--legacy`, `--sidecar`, `--repository`, `--check`, `--quiet`).
- [x] T016 [US1] Add a `docs-api` task to `.mise.toml` running the binary through
      `scripts/cargo-container-run.sh` (per this repository's rule that Rust work runs through a
      `mise` task, never a bare host `cargo`).
- [x] T017 [US1] Run `mise run docs-api` against the real repository and reconcile the result: the
      reported route count must match an independent count, `/health` and `/opensearch.xml` must be
      bare while everything else is under `/api`, and any route the scanner rejects must be
      inspected by hand rather than silenced. Record the final numbers in `research.md` §11 if they
      moved materially from the figures measured during planning.

**Checkpoint**: `docs/public/openapi.json` exists, is deterministic, lists every route, and is
correct about mount prefixes. This is the MVP: a developer can already read a complete, honest
endpoint list.

---

## Phase 4: User Story 2 — Undocumented endpoints fail the build instead of drifting (Priority: P1)

**Goal**: The gates, the coverage report, and the CI job that makes the completeness claim durable.

**Independent Test**: register a throwaway route and confirm it appears with no hand-written entry;
add a sidecar entry for a route that does not exist and confirm the build fails naming the file.
(`quickstart.md` Scenarios 3, 4 and 12.)

- [x] T018 [US2] Implement the gates in `crates/lanrurugi-api-docgen/src/validate.rs` per
      `contracts/docgen-cli.md`: resolution, mount consistency, completeness (assert
      `routes_described == routes_registered`), sidecar freshness (stale entry, duplicate
      `(path, method)`, unknown method key), and document integrity (path keys start with `/`, every
      operation has non-empty `responses`, `operationId` unique). Every failure names the offending
      route, file, and line, and exits `1`; a missing or unreadable input exits `2`.
- [x] T019 [US2] Implement the coverage report in `crates/lanrurugi-api-docgen/src/validate.rs`,
      printing `routes_registered`, `routes_described`, the `by_schema_source` breakdown, the
      `upstream_only_paths` list, `stale_sidecar_entries`, and `unresolved_routes`. Printed on every
      non-quiet run, including successful ones; non-gating figures never fail the build.
- [x] T020 [US2] Write integration tests in `crates/lanrurugi-api-docgen/tests/merge_pipeline.rs`
      over fixture input trees covering: a stale sidecar entry failing; two fragments claiming one
      `(path, method)` failing; an unknown method key failing; a missing route in the document
      tripping the completeness assertion; and the coverage figures being computed correctly on a
      small fixture.
- [x] T021 [US2] Add a `docs-generate` job to `.github/workflows/ci.yml`: checkout, `jdx/mise-action`,
      `mise run docs-api -- --check`, then `actions/upload-artifact` for `docs/public/openapi.json`.
      This job is the enforceable gate and must be the required check for the feature.
- [x] T022 [US2] Verify the gate actually gates: on a scratch branch, register a route carrying a
      runtime-computed path and confirm `mise run docs-api -- --check` fails with a file:line
      message; then remove it and confirm the job passes. Fix the message wording if it does not
      point a maintainer straight at the offending line.

**Checkpoint**: the reference cannot fall behind the code, and a description of a nonexistent
endpoint cannot be published.

---

## Phase 5: User Story 3 — Legacy-compatible endpoints carry real field-level documentation (Priority: P2)

**Goal**: Vendor the upstream document, layer it underneath the route-derived entries, add the
sidecar mechanism, and report upstream-only paths.

**Independent Test**: `/api/search` carries upstream parameters; the count of
`x-lanrurugi-schema-source == "legacy"` operations is in the fifties; per-operation `security` is
absent; the nine upstream-only paths are reported and not published. (`quickstart.md` Scenarios 4
and 5.)

- [x] T023 [US3] Vendor the upstream document to `docs/api/legacy-openapi.yaml`, fetched from
      `https://raw.githubusercontent.com/Difegue/LANraragi/dev/tools/openapi.yaml`, verbatim and
      unmodified, with a short header comment recording the source URL, the revision date, and the
      MIT licence. Add a `docs-vendor-legacy` `mise` task that performs the fetch, so refreshing is a
      deliberate, reviewable act rather than an invisible one.
- [x] T024 [US3] Implement `crates/lanrurugi-api-docgen/src/legacy.rs`: parse the vendored document
      with `serde-saphyr`, derive the `/api` prefix from its own `servers[0].url` rather than
      hardcoding it, prefix every path key, drop every per-operation `security`, drop upstream's
      `components.securitySchemes` and `x-mojo-to` extensions, and split the paths into `shared` and
      `upstream_only` against the `RouteRecord` set.
- [x] T025 [US3] Implement `crates/lanrurugi-api-docgen/src/sidecar.rs` per
      `contracts/sidecar-format.md`: read every `*.yaml`/`*.yml` under `docs/api/sidecar/`
      recursively, validate path keys against the `RouteRecord` set, validate method keys, reject
      duplicate `(path, method)` across fragments, and merge any `components.schemas` with duplicate
      names rejected.
- [x] T026 [US3] Extend `crates/lanrurugi-api-docgen/src/merge.rs` with the three-layer precedence:
      sidecar beats upstream beats auto-derived, whole-operation replacement, with upstream able to
      contribute an operation to a path the sidecar only partially claims. Set
      `x-lanrurugi-schema-source` accordingly.
- [x] T027 [US3] Extend the integration tests in `crates/lanrurugi-api-docgen/tests/merge_pipeline.rs`
      to cover precedence in both directions: a fragment fully replacing an upstream operation, a
      fragment adding a method to an upstream path without disturbing the existing one, an
      upstream-only path being reported rather than emitted, and the per-operation `security` fields
      being stripped from upstream-sourced operations.
- [x] T028 [P] [US3] Write the first sidecar tranche covering these API groups, one file per group
      under `docs/api/sidecar/`, reading each handler in the corresponding source file so no schema
      is guessed: `jobs.yaml` (`crates/lanrurugi-api/src/jobs.rs`, 2 paths), `activity.yaml`
      (`activity.rs`, 4), `sessions.yaml` (`sessions.rs`, 2), `api-tokens.yaml` (`api_tokens.rs`, 2),
      `preferences.yaml` (`preferences.rs`, 1).
- [ ] T029 [P] [US3] Write `download-queue.yaml` under `docs/api/sidecar/` covering the 19
      `download_queue.rs` paths — the largest LANrurugi-only group and the one with the most client
      relevance (it is the surface `specs/005-download-plugin-progress` added), reading
      `crates/lanrurugi-api/src/download_queue.rs` for each handler's real parameters and responses.
- [x] T030 [US3] Run `mise run docs-api` and record the coverage report in the feature's PR/report
      text: how many operations now carry sidecar detail, and what remains as the visible backlog
      (`x-lanrurugi-undocumented` count). The remaining groups are the documented follow-on work,
      not a defect in this tranche.

**Checkpoint**: 55 shared paths carry upstream field-level detail, 9 upstream-only paths are
reported, and ~30 LANrurugi-only paths carry hand-written detail with the rest honestly marked.

---

## Phase 6: User Story 4 — A developer tests an endpoint from the docs page (Priority: P2)

**Goal**: The site exists, renders the generated document through Scalar, gives the reader in-page
instance and token inputs plus working code samples, and the cross-origin defect that would break
JSON requests is fixed.

**Independent Test**: build the site and click through it; the reference lists every endpoint, the
server and token inputs are in-page, cURL samples copy cleanly, and `cors.rs` names
`Content-Type`. (`quickstart.md` Scenarios 6, 7, 8, 9, 10.)

- [x] T031 [US4] Fix `crates/lanrurugi-api/src/cors.rs`: extend `ALLOW_HEADERS` from `"Authorization"`
      to `"Authorization, Content-Type"`, with a comment recording why (`application/json` is not a
      CORS-safelisted `Content-Type` value, so a JSON-bodied cross-origin request is preflighted and
      the header must be allowed). Do not add a wildcard and do not change the setting's default.
- [x] T032 [US4] Add a test for the fix in `crates/lanrurugi-api/src/cors.rs`'s existing test module
      (or alongside it if none covers headers): with `enablecors` on, a response advertises
      `Content-Type` in `Access-Control-Allow-Headers`, and an `OPTIONS` preflight naming
      `content-type,authorization` is answered without reaching the auth layer.
- [x] T033 [US4] Create `docs/astro.config.mjs` and `docs/tsconfig.json` per
      `contracts/docs-site.md`: `base` from `DOCS_BASE` (default `/`), `site` from `DOCS_SITE_URL`,
      `defaultLocale: 'zh-CN'`, an explicit `sidebar` (required — without one `@scalar/starlight`
      declines to add its entry), and the `scalarStarlight` plugin pointed at a same-origin
      `openapi.json` with `proxyUrl` deliberately unset.
- [x] T034 [P] [US4] Write the hand-written pages under `docs/src/content/docs/`: `index.mdx`,
      `guide/install.md` (Docker/NAS), `guide/quickstart.md`, and `plugin/getting-started.md`. Chinese
      prose, matching the project's primary README language.
- [x] T035 [US4] Write `docs/src/content/docs/guide/api-testing.md` discharging `spec.md` FR-015 and
      `contracts/docs-site.md`'s "required hand-written content": requests go browser→your instance
      directly; the instance must be reachable; cross-origin access must be enabled on the instance;
      an HTTPS docs page can only call `https://` instances and `http://localhost` (with the
      mixed-content explanation and the `ssh -L` port-forward workaround); every operation has a
      copy-ready command line that does not depend on browser policy.
- [x] T036 [US4] Add `docs-dev` and `docs-build` tasks to `.mise.toml` wrapping
      `pnpm --filter lanrurugi-docs run dev` / `run build`, with the `DOCS_BASE` value passed through
      so both the root and `/docs/` builds are one command with different environment.
- [x] T037 [US4] Add a `docs-build` job to `.github/workflows/ci.yml` that `needs: docs-generate`,
      downloads the artifact into `docs/public/`, runs `pnpm install --frozen-lockfile` and the site
      build, and then deploys with `cloudflare/wrangler-action` **only** on a push to the release
      branch **and only** when the deployment secrets are present. A repository without those
      secrets must still produce a green run.
- [x] T038 [US4] Verify the site end to end: both `DOCS_BASE` builds succeed; the sub-path build's
      links and assets resolve when served from `/docs/`; the reference lists every endpoint from the
      generated document; the server input, the token input, and the cURL sample are all present and
      are Scalar's own UI rather than a hand-built parallel version; and the browser's network panel
      shows no request leaving for an external host.

**Checkpoint**: a developer can open the docs, point them at their own instance, and issue a real
request — or copy a working command line when the browser will not let them.

---

## Phase 7: User Story 5 — Plugin authors get a generated SDK reference (Priority: P3)

**Goal**: The SDK reference, generated from the same TypeScript the dispatcher loads, reachable from
the site and free of machine-specific paths.

**Independent Test**: `mise run docs-sdk` produces a browsable HTML tree and a normalised JSON file,
and no file under `docs/dist-sdk/` contains the workspace's absolute path.
(`quickstart.md` Scenario 6.)

- [x] T039 [US5] Write `scripts/normalize-sdk-doc-json.mjs`: read `deno doc --json` output on stdin,
      rewrite every `file://` node key and any embedded absolute path to a repository-relative form,
      and write the result out. Deterministic key ordering, so the artifact does not churn.
- [x] T040 [US5] Add a `docs-sdk` task to `.mise.toml` that runs `deno doc --html` over
      `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`,
      `crates/lanrurugi-plugin/dispatcher/dispatcher.ts` and `plugins/legacy-globals.d.ts` into
      `docs/dist-sdk/`, then `deno doc --json` over the same three files piped through
      `scripts/normalize-sdk-doc-json.mjs` into `docs/dist-sdk/plugin-sdk.json`. Keep the existing
      `plugin-sdk-docs` task working (the Dockerfile and any current user of it must not break).
- [x] T041 [US5] Wire the SDK reference into the site: a sidebar link to the `<base>sdk/` sub-path
      and a short pointer from `docs/src/content/docs/plugin/getting-started.md`. Do not re-render
      the SDK docs inside Starlight — the tool's own output is already a complete, searchable,
      self-contained reference.
- [x] T042 [US5] Extend the `docs-generate` CI job to also produce the SDK artifacts and upload them
      with the API document, so the `docs-build` job receives both from one run at one revision.

**Checkpoint**: a plugin author can read signatures and doc comments for the whole SDK, offline, and
tooling can consume the same information as JSON.

---

## Phase 8: User Story 6 — A self-hoster reads the docs offline from their own instance (Priority: P3)

**Goal**: The same site, built with `base: /docs/`, shipped inside the image and served by the
existing `LANRURUGI_DOCS_DIR` mount.

**Independent Test**: from a deployment built off this branch, `/docs/` serves the site and the API
reference page renders with no external network access. (`quickstart.md` Scenario 11.)

- [x] T043 [US6] Restructure the `docs-builder` stage in `Dockerfile`: base it on the same
      `node:${NODE_VERSION}-slim` image the `frontend-builder` stage already uses, copy the Deno
      binary in from the existing `deno-bin` stage, and have it produce `/build/docs` containing the
      built site with `base: /docs/` plus the SDK sub-tree. The runtime stage's
      `COPY --from=docs-builder /build/docs /usr/local/share/lanrurugi/docs` and
      `ENV LANRURUGI_DOCS_DIR=/usr/local/share/lanrurugi/docs` stay as they are.
- [x] T044 [US6] Add the API-document generation step to the `rust-builder` stage in `Dockerfile`
      (`cargo run -p lanrurugi-api-docgen -- --output <path>`), so the image build produces the
      document from the same source revision rather than depending on a committed copy. Copy the
      result into the `docs-builder` stage's `docs/public/`.
- [ ] T045 [US6] Confirm the image build does not regress the runtime footprint: the only new content
      in the runtime stage is static files under `/usr/local/share/lanrurugi/docs`, replacing the
      smaller SDK-only directory that was already there. No new runtime process, no new runtime
      dependency, no build tool shipped.
- [ ] T046 [US6] Verify offline serving: `build_app`'s `nest_service("/docs", ...)` already handles
      the mount, so confirm by inspection of `crates/lanrurugi-server/src/app.rs` that nothing needs
      changing, then exercise `/docs/` and `/docs/api-reference/` on a running instance and confirm
      the reference page renders with external access blocked.

**Checkpoint**: the identical site is readable from the reader's own instance with no internet.

---

## Phase 9: Polish & Cross-Cutting Concerns

- [x] T047 [P] Update `README.md`'s documentation section (and `README.en.md` / `README.ja.md`'s
      equivalents) to point at the documentation site and the generated API reference, and to record
      that the API reference is generated rather than hand-maintained. Before pushing, re-check the
      "Improvements over LANraragi" claims against what this feature actually shipped, per
      `CLAUDE.md`.
- [x] T048 [P] Add `011-api-docs-site` to the spec inventory in `CLAUDE.md`'s SPECKIT block, and
      correct that block's stale status claims for 002/003/005/007 (they are described there as
      not-yet-implemented while their own `tasks.md` files record them as complete — a drift this
      feature's own spirit says not to leave standing).
- [x] T049 Run `mise run check` (rust-check + frontend-lint) and fix every failure. Then run
      `mise run fmt` and `mise run clippy` and address anything they raise in the new crate — the new
      crate is a workspace member, so it is covered by the same gates as everything else and there is
      no exemption to claim.
- [ ] T050 Walk `quickstart.md`'s twelve scenarios end to end and record the outcome of each one,
      including any that could not be exercised in this environment (the live Cloudflare deployment,
      and the real-instance request in Scenario 10) with the reason stated rather than implied.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies; T002–T005 are parallel once T001 lands.
- **Foundational (Phase 2)**: depends on T001/T002. Blocks every user story. T008 is parallel with
  T006/T007/T009; T010 and T011 depend on all of them.
- **US1 (Phase 3)**: depends on Phase 2. T012–T014 are largely independent files; T015 depends on
  T012–T014; T016/T017 depend on T015.
- **US2 (Phase 4)**: depends on US1 (the gates validate the document US1 emits). T018/T019 are
  sequential; T020 depends on both; T021 depends on T015; T022 depends on T021.
- **US3 (Phase 5)**: depends on Phase 2 only for the route set, and on US1 for the merge pipeline it
  extends. T023 is independent of everything else and can start immediately. T028/T029 are parallel
  with each other once T025 lands.
- **US4 (Phase 6)**: T031/T032 (the `cors.rs` fix) are independent of the docs site and can be done
  first. T033–T036 form the site. T037 (the CI build job) depends on T021 and T036. T038 depends on
  all of them.
- **US5 (Phase 7)**: independent of US3/US4 except for the sidebar link (T041). Can run in parallel
  with US4 once Phase 2 is done.
- **US6 (Phase 8)**: depends on US1 (the document), US4 (the site), and US5 (the SDK artifacts),
  since it packages all three.
- **Polish (Phase 9)**: depends on every story that ships.

### User Story Dependencies

- **US1**: the only story with no story-level prerequisite. It is the MVP.
- **US2**: builds on US1's document but adds no new content; independently testable by the
  stale-entry and throwaway-route scenarios.
- **US3**: builds on US1's merge pipeline. Independently testable through the legacy coverage
  figures and the upstream-only report.
- **US4**: independent of US3 — it renders whatever the document contains, and can be demonstrated
  against a route-only document before any sidecar exists.
- **US5**: fully independent of US1–US4.
- **US6**: the only story that genuinely depends on the others, because it packages their output.

### Within Each User Story

Scanner before merge before emit before CLI before verification. Gates before the CI job that runs
them. Vendor the document before coding against it. Fix the header before claiming the try-it panel
works.

---

## Parallel Opportunities

Setup: T002, T003, T004, T005 together.

Foundational: T006, T007, T008, T009 target four different files and can be written together; T010
and T011 join them afterwards.

US3: T023 (fetch + vendor) is independent of all code; T028 and T029 are two authors writing two
disjoint sidecar files; T024 and T025 are two disjoint source files.

US4: T031/T032 (`crates/lanrurugi-api/src/cors.rs`) are disjoint from T033–T036 (`docs/`), so the
server-side defect fix and the site can proceed simultaneously.

US5 is parallel with US4 in its entirety.

Polish: T047 and T048 touch different files and can be done together.

---

## Parallel Example: Phase 2 (Foundational)

```bash
# Four disjoint files, one scanner:
Task: "Implement route extraction in crates/lanrurugi-api-docgen/src/scan/routes.rs"
Task: "Implement test-code exclusion in crates/lanrurugi-api-docgen/src/scan/mod.rs"
Task: "Implement handler metadata collection in crates/lanrurugi-api-docgen/src/scan/handlers.rs"
Task: "Implement the mount table and its assertions in crates/lanrurugi-api-docgen/src/scan/mount.rs"
```

## Parallel Example: US3 sidecar tranche

```bash
# Two disjoint API groups, both read from their own source file:
Task: "Write docs/api/sidecar/{jobs,activity,sessions,api-tokens,preferences}.yaml"
Task: "Write docs/api/sidecar/download-queue.yaml"
```

---

## Implementation Strategy

### MVP first

Phase 1 → Phase 2 → Phase 3. At that point `mise run docs-api` produces a document that lists every
endpoint this server serves, at correct paths, with each operation carrying the handler's own
documentation and a pointer into the source. That is already strictly better than the 71-line
contract document it replaces, and it cannot drift.

### Incremental delivery

1. MVP (Phases 1–3): a complete, honest, generated endpoint list.
2. Phase 4: the gates and CI job that keep it honest.
3. Phase 5: field-level detail for the compatibility surface, plus the sidecar mechanism.
4. Phase 6: the site a human actually reads and uses.
5. Phases 7–8: the SDK reference and the offline copy.
6. Phase 9: the bookkeeping that stops the repository's own claims from going stale.

Each increment is independently demonstrable, and no increment requires a later one to be useful.

### Notes

- `[P]` marks tasks touching different files with no dependency on an incomplete task.
- The scanner is the one place where a silent mistake is invisible downstream — every gate is built
  on its output, so T011's tests are the highest-leverage tests in this feature.
- `x-lanrurugi-undocumented` is not an apology, it is a measurement. The count from T030 is the
  backlog, and it should be quoted whenever someone asks how complete the reference is.
- Do not add response schemas by inference. A missing schema is visible and fixable; a wrong one is
  neither.

---

## Status at implementation (2026-10-11)

Every task above was executed except the four listed here, and the deviations are worth stating
plainly rather than leaving to a checkbox.

**Not completed, and why:**

- **T029** — `docs/api/sidecar/download-queue.yaml` covers the queue's lifecycle (listing, adding,
  editing, deleting, starting individually and in bulk: 8 of the file's 19 paths), not all 19. The
  remaining 11 are the archive-comparison sub-group under `/download_queue/{id}/compare*` plus
  `/overwrite`, `/rename`, `/stop`, `/confirm-older` and `/fetch-metadata`. They are listed in the
  reference from the route scan, with their own handler notes, and counted as backlog — the file's
  own header says so. This is a smaller tranche than planned, not an abandoned one.
- **T045** — the image build was not run (a full workspace release build is heavy, and the host was
  under sustained memory pressure throughout). The claim that the runtime stage gains only static
  files is verified by reading the Dockerfile, not by building it.
- **T046** — offline serving from a running instance was not exercised, because it needs an image
  built from this branch and the user's dev container runs the previous one. What *was* verified is
  stronger than nothing but weaker than the task: the built site was served locally and loaded in a
  real browser, and the API reference's every request went to that local server — including Scalar's
  own runtime and its fonts (see "Findings" below).
- **T050** — `quickstart.md` was walked for scenarios 1–9 and 12. Scenarios 10 and 11 need a live
  instance (a real API request from the page, and `/docs` served by a rebuilt image), so they are
  unverified rather than passing.

**Findings that changed the implementation**, each caught by checking rather than assuming:

1. **The API reference was loading its runtime from a CDN.** `@scalar/astro`'s client render mode
   falls back to jsdelivr when no `cdn` is configured, which would have made the API page the one
   part of the site that does not work offline — the entire point of shipping it in the image.
   Fixed by self-hosting the bundle (`docs/scripts/prepare-static-assets.mjs`).
2. **…and the obvious way to self-host it was wrong.** The first attempt used the code-split ESM
   build, which the loader injects as a *classic* script; confirmed live in a browser,
   "Cannot use import statement outside a module", with the reference never mounting. The correct
   target is the UMD bundle, which is also smaller to ship (4.2MB in one file vs. 8.9MB across 102).
3. **Scalar fetched its fonts from `fonts.scalar.com` on every page view** — a second external fetch,
   found only by watching a real browser's network panel. `withDefaultFonts: false` removes it.
4. **Two real documentation defects in the source**, both now public: `translation.rs`'s
   `clear_page_translation` had `get_page_translation`'s doc comment merged into its own (fixed), and
   the reference surfaced an empty `registries` tag group plus a `subscriptions::api` Rust module path
   used as a heading (both fixed in the generator).
5. **`mise run check` is vacuous without a commit.** It runs lefthook's pre-push hooks against *files
   being pushed*; with an uncommitted worktree both commands report "no matching push files" and the
   task passes without checking anything. The underlying commands (`mise run clippy`,
   `mise run fmt-check`, `tsc -b --force`, `eslint`) were run directly instead.

**Measured outcome**: 232 routes registered, 232 described; 73 operations carry the vendored
upstream document's field-level detail, 26 carry hand-written sidecar detail, 133 are listed with
the handler's own notes and marked as not yet described. 9 upstream paths this server does not serve
are reported by every run rather than published.

## Revision (2026-10-11) — three deliverables, not one site

The design above describes a single documentation site that also hosted the API reference and the
SDK tree as sub-paths. It now publishes **three independently deployable artefacts**: the docs site,
a standalone Swagger UI page, and the SDK reference — see `contracts/deliverables.md`.

Three things this changed that the task list above still describes in its old form:

- `docs/public/sdk/` no longer exists; the SDK builds to `docs/dist-sdk/` (its own Pages project).
  Every `docs/public/sdk` path in `plan.md`, `quickstart.md`, `data-model.md` and
  `contracts/docs-site.md` is superseded by `docs/dist-sdk/`.
- The API reference is no longer a whole-page Scalar viewer. That rendering put every operation inside
  Scalar's own application chrome, so the site had no per-group pages, no real heading anchors for its
  right-hand outline, and a sidebar of internal module names. It is now generated per group by
  `docs/scripts/generate-api-pages.mjs`, matching the reference site's structure.
- A fourth artefact exists that the original plan did not have: `docs/dist-swagger/`
  (`mise run docs-swagger`), which is what carries schema browsing and the "point it at any OpenAPI
  JSON URL" capability.

Two defects fixed while doing this, both of which the original plan's tasks would have shipped:
`/en/sdk/` was a dead link on all 43 English pages (Starlight locale-prefixes sidebar `link` values,
but the generated SDK tree was a `public/` asset existing only at `/sdk/`), and 13 root-absolute
internal links would have 404'd in the `base: /docs/` in-image build.

## Revision — 2026-10-11 (second pass)

Added after the site was built and reviewed live. Recorded here rather than as new task lines because
each is a correction to what the earlier tasks produced, not unbuilt work.

- **T015/T016 (authentication).** The document stated one global `bearerAuth` for every operation.
  It now derives each operation's requirement from `crates/lanrurugi-api/policy/route_policy.csv` —
  the file the server enforces — via the same `some(allow) && !some(deny)` rule Casbin applies. 38 of
  231 operations were misdescribed. A `sessionCookie` scheme was added.
- **T017 (the gate).** The first gate ("every route must be mentioned by the policy") could never
  fire: `p, session, /api/*, *, allow` matches every `/api` route, so the condition was unanswerable.
  Verified by injecting an uncovered route and watching it pass. Replaced with the invariant that is
  real — a guest token is read-only — which fires 118 times when the wildcard is widened.
- **T018 (role asymmetry).** 117 operations refuse a *guest* token while accepting an administrator
  one; the emitter had collapsed both into one boolean, so the two classes were indistinguishable.
  `x-lanrurugi-token-role` states it, and the Swagger page now sets `showExtensions` so the
  `x-lanrurugi-*` fields are visible at all.
- **T019 (per-role behaviour).** `GET /api/activity` returns a different set of entries per role, and
  the guest scoping of `/api/search` and the archive reads returns 404 rather than 403. None of it is
  expressible in OpenAPI. Added `docs/api/access-notes.yaml` + `x-lanrurugi-access-notes`, a fourth
  pipeline input, deliberately a *distinct field* so it does not reintroduce the field-level merging
  `merge.rs` rejects, and deliberately not a sidecar fragment (which replaces the whole operation and
  would force upstream's description to be copied into this repository).
- **Removed**: the upstream `🔑` marker (redundant for 37 operations, false for
  `POST /api/database/drop`), and the Swagger page's Explore field. The two upstream links in the
  site's top bar were also removed.
