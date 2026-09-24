# Implementation Plan: Automated UI Test Coverage

**Branch**: `003-ui-test-automation` | **Date**: 2026-07-18 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/003-ui-test-automation/spec.md`

## Summary

Add the two-layer automated frontend test coverage that specs/001-lanrurugi-full-rewrite's own
plan already called for (Vitest + Testing Library, referenced in its Technical Context/Testing
section) but was never actually implemented — the frontend currently has zero test dependencies
and zero test files. Layer 1 is fast, no-backend Vitest + React Testing Library unit coverage for
pure logic (reader settings/navigation hooks, cross-archive navigation resolution, metadata
formatting/decoding). Layer 2 is Playwright end-to-end coverage (Chromium + Firefox) driving a
real running backend + Redis, reproducing the specific defects this project's manual
chrome-devtools-MCP-driven QA sessions have already found and fixed once (category pinned-field
save failure, upload body-size limit, archive-delete leaving orphaned search-index entries, reader
icon-spacing and dead-whitespace layout bugs), plus systematic fixture-archive coverage across
every format the scanner supports and the higher-risk shapes (multi-volume, encrypted, non-ASCII
filenames) that have already produced a real defect. Both layers run automatically in the
project's existing CI (constitution's "automated, non-bypassable... gates" pattern, extended to
the frontend job rather than a new mechanism), with a maintainer able to run either layer locally
scoped to a subset. This is testing infrastructure layered on top of Phase 1's already-shipped
code; it does not change Phase 1 product behavior and does not block or get blocked by Phase 2
(constitution Principle VI).

## Technical Context

**Language/Version**: TypeScript (strict mode, matching `apps/frontend/tsconfig.json`), targeting
the project's existing React 19 frontend. No backend language/version change — the end-to-end
layer drives the existing Rust/Axum backend as an external black box over HTTP, it does not modify
or add Rust test code.

**Primary Dependencies**: `vitest@4.1.10` + `@testing-library/react@16.3.2` (+
`@testing-library/dom`, `@testing-library/jest-dom`, `jsdom@29.1.1` test environment) for Layer 1;
`@playwright/test@1.61.1` for Layer 2 (see research.md §1–2 for exact versions/config, verified
live against npm at plan time per constitution's "latest stable release" rule — not reused from
`~/jellyfin-suite`'s own pins unverified).

**Storage**: N/A for Layer 1 (pure logic, no I/O). Layer 2 exercises the project's existing Redis
data store through the real running backend; per-worker isolation uses `SELECT`-ed Redis logical
databases keyed by Playwright's `testInfo.parallelIndex` (research.md §4).

**Testing**: This feature *is* the testing setup — Vitest (unit) and Playwright (end-to-end) are
being introduced, not consumed as an existing given.

**Target Platform**: Linux CI runner (GitHub Actions, matching the project's existing
`.github/workflows/ci.yml`) and any contributor's local development machine. Browser engines:
Chromium and Firefox (per spec Clarifications; WebKit explicitly out of scope).

**Project Type**: Web application test tooling, added to the existing single-repo, single
Cargo-workspace, single-frontend-app structure (constitution Technology Stack Constraints) — not a
new deployable, not a new app.

**Performance Goals**: Spec SC-002 (unit-level results in under one minute, no backend required)
and SC-003 (full end-to-end run in under ten minutes).

**Constraints**: Spec FR-012 (auto-retry a failed end-to-end test exactly once before it counts as
a real failure), FR-013 (Chromium + Firefox minimum), FR-014 (isolated backend/Redis state per
concurrent worker), FR-015 (automatic screenshot + replayable trace on failure).

**Scale/Scope**: Initial coverage scoped to the specific known-bad scenarios this project has
already found and fixed once (spec Assumptions — not exhaustive on day one), plus systematic
fixture-archive coverage across the 13 formats `lanrurugi-scanner` supports
(`crates/lanrurugi-scanner/src/archive_format.rs::ARCHIVE_EXTENSIONS`) and three higher-risk
shapes (multi-volume, encrypted, non-ASCII filenames).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Legacy Data & User-Trust Compatibility | This feature must not alter any Redis data shape/keying, and end-to-end tests must not run against a shared instance that could corrupt a real user's data | **PASS** — this is test-only infrastructure; no product data model changes. FR-014 (isolated per-worker Redis) and FR-011 (clean state per run) specifically exist to prevent any test run from touching non-test data, directly addressing the real data-corruption incidents this project's own manual QA already hit (a shared Redis instance between `cargo test` and live browser verification silently wiped production-like data mid-session) |
| II. API Contract Fidelity (Phase 1) | This feature must not modify existing `/api/*` request/response shapes | **PASS** — end-to-end tests are a pure HTTP client of the existing, already-shipped API; no endpoint is added, removed, or reshaped by this feature |
| III. Resource-Conscious, Genuinely Concurrent Architecture | N/A for a test-tooling feature — no new production runtime component is introduced | **PASS (not applicable)** — Playwright's own worker parallelism is a test-harness concern, not a change to the shipped `lanrurugi-server` binary's concurrency model |
| IV. Sandboxed, Language-Agnostic Plugin Extensibility | N/A — this feature does not touch the plugin runtime | **PASS (not applicable)** |
| V. Secrets & Network Trust Boundaries | N/A — this feature does not touch Phase 2 or any LLM-provider secret handling | **PASS (not applicable)** |
| VI. Phased Scope Discipline | Must remain additive to Phase 1, must not require or block Phase 2 work | **PASS** — spec FR-008 explicitly forbids depending on Phase 2 completion or requiring Phase 1 product-code changes merely for "testability"; this plan adds only test infrastructure and test files, no production code changes are in scope |

No violations requiring justification — **Complexity Tracking is empty.**

**Post-design re-check** (after research.md, data-model.md, quickstart.md): still PASS across all
six principles. research.md's fixture-sourcing decisions (§5-8) touch no product data shape — they
add test-only binary files under a new `test-fixtures/` directory, never written to by the shipped
`lanrurugi-server` binary at runtime (Principle I unaffected). data-model.md introduces no
persisted entity and no new endpoint (Principle II unaffected, confirmed by the explicit
no-`contracts/` decision). No Phase 2 concept, dependency, or file appears anywhere in research.md/
data-model.md/quickstart.md (Principle VI unaffected). No new Complexity Tracking entries.

## Project Structure

### Documentation (this feature)

```text
specs/003-ui-test-automation/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command) — see note below
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

Note on `contracts/`: this feature adds no new REST/external interface (constitution Principle II
gate above) — the end-to-end layer is a *client* of the existing, already-contracted API, not a
new surface. `contracts/` is therefore replaced by a lightweight "Scenario Contract" description in
data-model.md (the fixed vocabulary of Test Scenario / Regression Fixture / Fixture Archive
entities from the spec) rather than an API schema, matching the template's own guidance to skip
this artifact type when there's no new external interface.

### Source Code (repository root)

This feature adds to the existing single Cargo workspace + single frontend app (constitution
Technology Stack Constraints) — no new crate, no new deployable, no new top-level directory beyond
the one shared fixtures directory research.md §7 calls for.

```text
test-fixtures/                       # NEW — shared between Rust unit tests and Playwright E2E
└── archives/                        # research.md §7: single source of truth, no per-layer copies
    ├── sample.zip / .cbz / .epub / .rar / .cbr / .7z / .cb7 / .lzh / .lha / .tar / .gz / .bz2 / .xz
    │                                 # one fixture per lanrurugi-scanner-supported format (FR-009)
    ├── multivolume.7z.001/.002/...  # research.md §5 (7z CLI `-v`)
    ├── encrypted.7z                  # research.md §5 (7z CLI `-mhe=on`)
    ├── cjk-names.7z                  # MOVED from crates/lanrurugi-scanner/tests-fixtures/
    ├── cjk-shiftjis-noflag.zip       # hand-built, non-UTF-8-flagged (research.md §5.2)
    └── rar/                          # reused verbatim from the `unrar` crate's own bundled
                                       # fixtures (research.md §6) — no project-built RAR fixture:
                                       # no FOSS RAR-writer exists, so a genuine-CJK-content RAR
                                       # fixture is out of scope for this feature (tasks.md T041)
        ├── crypted.rar, archive.part1.rar, unicode.rar, unicodefilename❤️.rar, ...

crates/
└── lanrurugi-scanner/
    └── tests-fixtures/              # cjk-names.7z REMOVED (moved above); directory deleted if
                                       # left empty, otherwise kept for any scanner-only fixture
                                       # that only ever needs to be `include_bytes!`-ed, not
                                       # uploaded through Playwright

apps/frontend/
├── vitest.config.ts                 # NEW — Layer 1 (research.md §1); a `test: {...}` block over
│                                     # the existing vite.config.ts, per jellyfin-suite's own
│                                     # precedent of a repo-root-rooted config
├── playwright.config.ts             # NEW — Layer 2 (research.md §2-4): chromium+firefox projects,
│                                     # retries: 1 in CI, screenshot/trace-on-failure, webServer
│                                     # array (Redis + lanrurugi-server + frontend preview),
│                                     # globalSetup for Redis seed/flush + backend readiness poll
├── src/
│   └── ...                           # existing app code — no product code changes in this
│                                     # feature's scope (constitution Principle VI / spec FR-008)
└── tests/
    ├── unit/                        # Layer 1 — Vitest + RTL, no backend
    │   ├── useReaderSettings.test.ts
    │   ├── useReaderNavigation.test.ts
    │   ├── crossArchiveNav.test.ts
    │   └── fileInfoText.test.ts     # + tag-namespace-grouping helper coverage
    └── e2e/                         # Layer 2 — Playwright, real backend + real Redis
        ├── login.spec.ts
        ├── categories.spec.ts       # incl. the pinned-field save regression (spec US1/US2)
        ├── upload.spec.ts           # incl. the large-archive body-size-limit regression
        ├── reader.spec.ts           # icon spacing, fit-mode switching, dead-whitespace regression
        ├── archive-lifecycle.spec.ts # delete + random-archive-navigation regression
        └── archive-formats.spec.ts  # spec User Story 4 — one scenario per fixture in
                                       # test-fixtures/archives/

.github/workflows/ci.yml             # MODIFIED — frontend job gains a Vitest step; a new job (or
                                       # an added step depending on task-level sequencing) runs
                                       # Playwright against a real built backend + Redis service
                                       # container, matching the existing `rust` job's Redis
                                       # service-container pattern already in this file

.mise.toml                            # MODIFIED — adds [tasks.test-frontend-unit] and
                                       # [tasks.test-frontend-e2e], giving this feature the same
                                       # one-command entry point this project's Rust
                                       # test/clippy/build tasks already have, rather than
                                       # requiring a maintainer to remember raw pnpm/playwright
                                       # invocations. test-frontend-e2e defaults to a clean
                                       # environment per run (spec FR-011); a `KEEP` environment
                                       # variable opts a single run out of teardown, for the
                                       # spec's explicitly-allowed "inspect a failed run by hand"
                                       # case — KEEP never persists to the next run
```

**Structure Decision**: Web-application layout (matching 001's own "Structure Decision"), adding to
the existing structure rather than introducing a new one. All new code lives inside the existing
`apps/frontend/` app (`vitest.config.ts`, `playwright.config.ts`, `tests/unit/`, `tests/e2e/`) —
no new crate, no new top-level app. The one genuinely new top-level directory is
`test-fixtures/archives/`, justified by research.md §7: it must be reachable by both
`crates/lanrurugi-scanner`'s Rust tests (`include_bytes!`) and `apps/frontend`'s Playwright tests
(real file upload) without maintaining two copies of the same binary fixtures, so it cannot live
inside either single side's own tree without implying false ownership. `.github/workflows/ci.yml`
is modified, not replaced, extending the existing `frontend` job and Redis-service-container
pattern already established there rather than inventing a parallel CI mechanism (constitution's
"automated, non-bypassable... gates" — the local hook is a fast-feedback convenience, CI is the
authoritative gate, per Engineering Workflow & Quality Gates).

## Complexity Tracking

*No entries — Constitution Check produced no violations requiring justification. The one
structural addition beyond a single app/crate (`test-fixtures/archives/`) is a plain shared data
directory, not an architectural pattern, service, or project — it does not introduce the kind of
complexity this section exists to gate.*
