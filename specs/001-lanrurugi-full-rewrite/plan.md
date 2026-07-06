# Implementation Plan: LANrurugi — Full Rewrite (Phase 1 Core + Phase 2 Translation)

**Branch**: `001-lanrurugi-full-rewrite` | **Date**: 2026-07-05 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/001-lanrurugi-full-rewrite/spec.md`

## Summary

Rewrite LANraragi (Perl/Mojolicious + Redis) as LANrurugi (Rust/Axum/Tokio + Redis + React 19/TS),
delivering Phase 1 feature parity — library continuity (US1), correct non-merging archive
ingestion (US2), third-party API compatibility (US3), plugin-based metadata enrichment (US4),
backup/export (US5), a duplicate-repair tool for historically merged archives (US6), UI
localization (US7), and a benchmark suite proving genuine multi-core concurrency gains over the
legacy implementation (US8) — on a single, resource-conscious Rust binary that reuses the existing
Redis data store without requiring any destructive migration. Phase 2 (US9–US10, on-page
translation) is specified for continuity of vision but is out of scope for this plan; it will get
its own plan once Phase 1 is stable, per constitution Principle VI.

## Technical Context

**Language/Version**: Rust (stable channel, exact version pinned via `mise`/`.mise.toml` per
constitution Technology Stack Constraints) for the backend; TypeScript (strict mode) for the
frontend, targeting React 19.

**Primary Dependencies**:
- Backend: `axum` (HTTP), `tokio` (async runtime), `rayon` (CPU-bound data parallelism, bridged
  via `tokio::task::spawn_blocking` per Principle III), an async Redis client (`redis` crate +
  `deadpool-redis` or `fred`, choice finalized in research.md), `notify` (file watching, replaces
  Shinobu), `blake3` and/or `sha1` (archive-identity hashing — see research.md for the length/
  compatibility tradeoff), an image crate (thumbnailing), archive-format crates (`zip`; RAR/7z via
  shelling out to `unrar`/`7z` since RAR extraction cannot be legally reimplemented — see
  research.md), `clap` (CLI subcommands: `serve`, `rebuild-index`, `bench`), `criterion`
  (in-process microbenchmarks).
- Plugin runtime: Deno CLI invoked as a subprocess per constitution Principle IV (not a crate
  dependency — an external pinned binary, acquired per the Docker/toolchain constraints).
- Frontend: React 19, TypeScript, Vite, Tailwind CSS, Zustand (client state), TanStack Query
  (server-state sync/caching), an i18n library consuming the ported legacy `.po` locale content
  (see research.md).

**Storage**: Redis (reused as-is from the legacy deployment per constitution Principle I — no new
relational/embedded store introduced).

**Testing**: `cargo test` (backend unit/integration, including contract tests against the
OpenAPI-derived contracts in `contracts/`), a frontend test runner consistent with the user's
existing `~/jellyfin-suite` conventions (Vitest + Testing Library), `criterion` for Rust
microbenchmarks, and a dedicated cross-system comparison harness for US8 (drives both LANraragi
and LANrurugi against the same synthetic library and diffs the results).

**Target Platform**: Linux server, self-hosted (Docker image on Debian slim per constitution;
common deployment targets include home NAS/ARM boards, per the existing LANraragi user base).

**Project Type**: Web application (Rust backend + React SPA frontend, single deployable backend
binary serving the built frontend as static assets).

**Performance Goals**: SC-008 (responsive browsing/search/tag-filtering up to ~100,000 archives
and low-single-digit TB of content); SC-011 (benchmark-demonstrated improvement over the legacy
system for full library scan/ingestion and duplicate-repair reindex on the same multi-core
hardware).

**Constraints**: Constitution Principles I–VI in full — legacy Redis-data read-compatibility
(I), existing REST API contract fidelity (II), single-process/genuinely-concurrent architecture
with a mandatory Phase 1 benchmark (III), sandboxed Deno-subprocess plugin execution (IV),
secrets/trust-boundary rules that anticipate Phase 2 without building it (V, informs API/data
shape choices now so Phase 2 doesn't require reshaping them later), and strict Phase 1/Phase 2
scope separation (VI).

**Scale/Scope**: Single-owner, single-instance deployment (no multi-tenant/shared-community
scale per the spec's Clarifications); up to ~100,000 archives / low-single-digit TB per library
(SC-008); 8 Phase 1 user stories, 22 Phase 1 functional requirements (FR-001–FR-022), 9 Phase 1
success criteria (SC-001–SC-005, SC-008–SC-011).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Legacy Data & User-Trust Compatibility | Plan must reuse Redis as-is, and any new archive-ID algorithm must ship with a read-compatible legacy fallback + explicit rebuild tool, never a silent in-place change | **PASS** — data-model.md keeps Redis as the sole store; the size-aware ID algorithm from FR-005 is additive/opt-in (US6/FR-011–012 rebuild-index tool), legacy SHA-1(512KB) IDs remain readable indefinitely |
| II. API Contract Fidelity (Phase 1) | Plan must honor the existing `tools/openapi.yaml` contract for all pre-existing endpoints | **PASS** — contracts/ derives directly from the legacy `tools/openapi.yaml` (verified by reading the actual file, not assumed); new endpoints (backup trigger, rebuild-index, bench) are additive per FR-011 |
| III. Resource-Conscious, Genuinely Concurrent Architecture | Plan must consolidate Shinobu/Minion into one Tokio process and explicitly design the tokio/rayon split + Phase 1 benchmark | **PASS** — Project Structure below has no separate watcher/worker process; research.md covers the tokio/rayon bridging decision; US8/FR-020–022 make the benchmark a first-class Phase 1 deliverable |
| IV. Sandboxed, Language-Agnostic Plugin Extensibility | Plan must run plugins as an isolated Deno subprocess/worker pool with declared permissions | **PASS** — `lanrurugi-plugin` crate design (data-model.md) treats plugins as subprocesses only; no embedded JS engine considered |
| V. Secrets & Network Trust Boundaries | Phase 1 must not build Phase 2 translation, but must not make architectural choices that would force reshaping the API/secrets model later | **PASS (anticipatory)** — no Phase 2 code planned here; API contract additions in this plan are additive-only, leaving room for Phase 2's server-proxied cloud-provider calls without needing to break Phase 1 endpoints |
| VI. Phased Scope Discipline | Plan must cover only US1–US8 (Phase 1); Phase 2 (US9–US10) must not appear in Project Structure/tasks | **PASS** — Project Structure, data-model.md, and contracts/ below contain no translation/OCR/font-cache elements; Phase 2 is explicitly out of scope for this plan (see Summary) |

No violations requiring justification — **Complexity Tracking is empty.**

**Post-design re-check** (after research.md, data-model.md, contracts/, quickstart.md): still
PASS across all six principles — the size-aware/legacy ID coexistence and rebuild-index operation
in data-model.md implement Principle I exactly as designed; `contracts/rest-api.md` is additive-
only over the verified legacy path list (Principle II); research.md's `spawn_blocking` bridging
and the `bench/` harness implement Principle III's concurrency + benchmark mandate concretely;
`contracts/plugin-protocol.md` keeps plugins subprocess-only with pre-declared, pre-start
permissions (Principle IV); no Phase 2 entity, endpoint, or dependency was introduced anywhere in
this design (Principles V/VI). No new Complexity Tracking entries.

## Project Structure

### Documentation (this feature)

```text
specs/001-lanrurugi-full-rewrite/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
# Web application: Rust backend (Cargo workspace, single deployable binary) + React frontend

crates/
├── lanrurugi-server/          # Binary crate: main.rs, clap subcommands (serve/rebuild-index/bench),
│                               # Axum app assembly, Tokio runtime bootstrap, serves built frontend/dist
│   └── tests/                 # Integration tests (contract tests against contracts/, using cargo test)
├── lanrurugi-core/             # Domain types: Archive, Category, Grouping, ReadingProgress,
│                               # Extension — see data-model.md. No I/O.
│   └── tests/
├── lanrurugi-storage/          # Redis access layer; legacy SHA-1(512KB) + new size-aware ID
│                               # algorithms (Principle I); rebuild-index migration logic (US6)
│   └── tests/
├── lanrurugi-scanner/           # Shinobu-equivalent: notify-based file watcher, debounce,
│                                # ingestion pipeline, rayon-parallel hashing (US2, Principle III)
│   └── tests/
├── lanrurugi-plugin/            # Deno-subprocess plugin runtime: worker pool, permission
│                                # declarations, JSON-RPC-over-stdio protocol (US4, Principle IV)
│   └── tests/
├── lanrurugi-api/                # REST handlers matching contracts/ (legacy-derived + additive),
│                                  # OPDS, search (US3)
│   └── tests/
├── lanrurugi-search/              # Search query grammar/index over Redis (US3's search syntax
│                                  # compatibility)
│   └── tests/
└── lanrurugi-backup/              # Backup/export + restore, JSON shape derived from legacy
                                    # Model/Backup.pm (US5)
    └── tests/

bench/                       # US8 deliverable — not a crate's unit tests, a standalone harness
├── synthetic-library/       # Generator for a ~100k-archive synthetic test library (SC-008 scale)
├── criterion/                # In-process Rust microbenchmarks (hashing, thumbnailing)
└── compare/                  # Orchestrates both LANraragi (legacy) and LANrurugi against the
                               # same synthetic library on the same hardware; emits the SC-011
                               # comparison report

frontend/
├── src/
│   ├── components/
│   ├── pages/
│   ├── state/               # Zustand stores
│   ├── api/                 # TanStack Query hooks, typed from contracts/openapi.yaml
│   └── i18n/                 # Locale JSON converted from legacy locales/template/*.po (US7)
└── tests/                    # Vitest + Testing Library, mirroring ~/jellyfin-suite conventions
```

**Structure Decision**: Web application layout (Option 2), adapted to a Cargo workspace of
focused crates rather than one flat `backend/src/`, so each crate has a single clear
responsibility and can be unit-tested independently — while still producing exactly **one**
deployable binary (`lanrurugi-server`, exposing `serve`/`rebuild-index`/`bench` as `clap`
subcommands) per constitution Principle III. `bench/` sits outside `crates/` because US8 requires
driving *both* the legacy Perl system and the new binary side by side, which is an external
orchestration concern, not an in-crate unit test. Frontend follows a standard Vite/React layout
with `i18n/` called out explicitly since US7 depends on porting real legacy locale content, not a
greenfield i18n setup.

## Complexity Tracking

*No entries — Constitution Check produced no violations requiring justification.*
