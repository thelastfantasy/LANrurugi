---

description: "Task list for LANrurugi Phase 1 (User Stories 1-8)"
---

# Tasks: LANrurugi — Full Rewrite (Phase 1 Core)

**Input**: Design documents from `/specs/001-lanrurugi-full-rewrite/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md (all present)

**Tests**: Not explicitly requested in spec.md (no TDD mandate), so this list does not include a
separate "write failing tests first" phase per story. Each story instead ends with a task that
runs its `quickstart.md` validation scenario as the acceptance check. A cross-system/contract test
task is included in User Story 3 specifically because "existing contract must not change"
(Principle II) is otherwise unverifiable except by such a test.

**Scope**: Phase 1 only — User Stories 1–8 from spec.md. User Stories 9–10 (Phase 2 translation)
are explicitly out of scope per constitution Principle VI and plan.md's Summary; they are not
represented anywhere below.

**Revision note (2026-07-05)**: This version incorporates the remediation from the
`/speckit-analyze` pass — full `contracts/rest-api.md` path coverage (previously ~32 legacy paths
had no task), a search/browse responsiveness benchmark for SC-008 (previously uncovered), a
background full-verification task for FR-007's optional deferred-check clause, and a shared
job-status abstraction backing the `/minion/*`-equivalent endpoints. Task IDs were renumbered
accordingly (previous T001–T078 → current T001–T096); this is the authoritative numbering.

**Revision note (2026-07-06)**: Added `pnpm` (T002) and Rust build-acceleration tooling
(`sccache`/`mold`, T003; `Swatinem/rust-cache` in CI, T007) per new constitution Technology
Stack Constraints content — enriched existing task descriptions only, no renumbering (task count
unchanged at 96).

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1–US8, per spec.md)
- Every task names an exact file path, per plan.md's Project Structure

## Path Conventions

Cargo workspace crates under `crates/<name>/src/...` (each producing library code compiled into
the single `lanrurugi-server` binary, per plan.md/constitution Principle III); benchmark harness
under `bench/`; frontend under `frontend/src/...`. See `plan.md` § Project Structure for the full
tree.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Project initialization, toolchain, and quality-gate scaffolding.

- [ ] T001 Create Cargo workspace `Cargo.toml` at repo root with members
      `crates/lanrurugi-core`, `crates/lanrurugi-storage`, `crates/lanrurugi-scanner`,
      `crates/lanrurugi-plugin`, `crates/lanrurugi-api`, `crates/lanrurugi-search`,
      `crates/lanrurugi-backup`, `crates/lanrurugi-server`, per `plan.md` Project Structure
- [ ] T002 [P] Initialize frontend Vite + React 19 + TypeScript + Tailwind project in
      `frontend/`, managed via `pnpm` with a `pnpm-workspace.yaml` at repo root declaring it as a
      workspace package, per constitution Technology Stack Constraints (Repository layout)
- [ ] T003 [P] Create `.mise.toml` at repo root pinning exact Rust/Node/Deno/`pnpm` versions,
      plus `sccache` and `mold` as mise-managed tools for local Rust build acceleration
      (`RUSTC_WRAPPER=sccache`, `mold` as the linker), per constitution Technology Stack
      Constraints
- [ ] T004 [P] Adapt `lefthook.yml` at repo root from `~/jellyfin-suite/lefthook.yml`, adding a
      `cargo check && cargo fmt --check` command scoped to `crates/**/*.rs` alongside the
      frontend lint command (memory: `jellyfin-suite-tooling-reference`)
- [ ] T005 [P] Adapt `frontend/eslint.config.mjs` from
      `~/jellyfin-suite/apps/frontend/eslint.config.mjs` (memory:
      `jellyfin-suite-tooling-reference`)
- [ ] T006 [P] Write `Dockerfile` at repo root: Debian `bookworm-slim` base, multi-stage copy of a
      pinned-version Deno binary, `fonts-noto-cjk` package install, Rust release build stage
      copying `lanrurugi-server` and `frontend/dist`, per constitution Technology Stack
      Constraints
- [ ] T007 [P] Configure CI pipeline in `.github/workflows/ci.yml` running `cargo check`,
      `cargo fmt --check`, `cargo clippy`, `cargo test`, and `eslint` as the authoritative gate
      per constitution Engineering Workflow & Quality Gates, using the `Swatinem/rust-cache`
      GitHub Action (caching `~/.cargo` and `target/`) plus `sccache` (T003) so CI Rust builds
      aren't fully cold on every run

**Checkpoint**: Workspace builds (even with empty crates), lint/format tooling runs locally and
in CI.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core infrastructure every user story depends on.

**⚠️ CRITICAL**: No user story task may begin until this phase is complete.

- [ ] T008 Define core domain types (`Archive`, `Category`, `Grouping`, `ReadingProgress`,
      `Extension`, `Stamp`) in `crates/lanrurugi-core/src/entities.rs`, per `data-model.md`
- [ ] T009 [P] Implement the legacy archive-ID algorithm (SHA-1 of first 512KB) in
      `crates/lanrurugi-storage/src/id.rs`
- [ ] T010 [P] Implement the size-aware archive-ID algorithm (SHA-1 of first 512KB ++ u64
      big-endian file size) in `crates/lanrurugi-storage/src/id.rs`, per `research.md` §1 and
      constitution Technology Stack Constraints
- [ ] T011 Implement the Redis connection pool (`redis` + `deadpool-redis`) in
      `crates/lanrurugi-storage/src/redis.rs`
- [ ] T012 Implement Redis read/write mappers for Archive/Category/Grouping/ReadingProgress/Stamp
      in `crates/lanrurugi-storage/src/repository.rs`, matching legacy field names/key shapes
      verified in `data-model.md`
- [ ] T013 Implement a generic job-status tracking abstraction (store + polling) in
      `crates/lanrurugi-core/src/jobs.rs`, to be reused by the backup, rebuild-index, and bench
      job endpoints (US5/US6/US8) instead of each reimplementing its own job tracking
- [ ] T014 Set up the Axum app skeleton and router assembly in
      `crates/lanrurugi-server/src/app.rs`
- [ ] T015 Implement API-key authentication middleware in
      `crates/lanrurugi-server/src/middleware/auth.rs`, matching legacy auth semantics
      (`contracts/rest-api.md`)
- [ ] T016 Set up the `clap` CLI with `serve` / `rebuild-index` / `bench` subcommands in
      `crates/lanrurugi-server/src/main.rs`
- [ ] T017 Set up structured logging and error-handling infrastructure in
      `crates/lanrurugi-server/src/telemetry.rs`
- [ ] T018 [P] Implement the rayon-thread-pool + `tokio::task::spawn_blocking` bridging helper in
      `crates/lanrurugi-core/src/concurrency.rs`, per `research.md` §5 and constitution
      Principle III

**Checkpoint**: `lanrurugi serve` boots an empty Axum server backed by Redis; auth middleware and
the rayon/tokio bridge are available to every subsequent crate.

---

## Phase 3: User Story 1 - Continue an existing library without losing anything (Priority: P1) 🎯 MVP

**Goal**: Point LANrurugi at an existing library and see all data intact, with the full legacy
read-side API surface available (per `contracts/rest-api.md`).

**Independent Test**: `quickstart.md` §1 — open an existing legacy library, confirm every archive
lists with prior tags/categories/groupings/reading-progress unchanged.

- [ ] T019 [US1] Implement the library-open/bootstrap routine (verifies Redis reachable, archive
      folder reachable, no destructive step) in `crates/lanrurugi-storage/src/bootstrap.rs`
- [ ] T020 [P] [US1] Implement `GET /archives` listing endpoint in
      `crates/lanrurugi-api/src/archives.rs`, per `contracts/rest-api.md`
- [ ] T021 [P] [US1] Implement `GET /archives/{id}/metadata` endpoint in
      `crates/lanrurugi-api/src/archives.rs`
- [ ] T022 [P] [US1] Implement `GET /categories` and `GET /tankoubons` listing endpoints
      preserving grouping/volume order in `crates/lanrurugi-api/src/categories.rs` and
      `crates/lanrurugi-api/src/tankoubons.rs`
- [ ] T023 [P] [US1] Implement `GET /archives/{id}/progress/{page}` (read) in
      `crates/lanrurugi-api/src/archives.rs`
- [ ] T024 [P] [US1] Implement `GET /archives/{id}` (full detail) and `DELETE /archives/{id}` in
      `crates/lanrurugi-api/src/archives.rs`
- [ ] T025 [P] [US1] Implement `GET /archives/untagged` in `crates/lanrurugi-api/src/archives.rs`
- [ ] T026 [P] [US1] Implement `GET /archives/{id}/thumbnail` in
      `crates/lanrurugi-api/src/archives.rs`
- [ ] T027 [US1] Implement `GET /archives/{id}/categories` and `GET /archives/{id}/tankoubons`
      (per-archive membership) in `crates/lanrurugi-api/src/archives.rs`
- [ ] T028 [US1] Implement `GET /archives/{id}/toc` and `GET /archives/{id}/download` in
      `crates/lanrurugi-api/src/archives.rs`
- [ ] T029 [P] [US1] Implement `GET /archives/{id}/isnew` in `crates/lanrurugi-api/src/archives.rs`
- [ ] T030 [US1] Implement Stamp repository access plus `GET /archives/{id}/stamps`,
      `GET/PUT /archives/{id}/stamps/{index}`, `GET/PUT /stamps/{id}` in
      `crates/lanrurugi-api/src/stamps.rs` (new file — closes the Stamp-entity gap identified in
      `data-model.md`)
- [ ] T031 [US1] Implement `POST /categories/bookmark_link`, `GET /categories/bookmark_link/{id}`,
      `GET/PUT/DELETE /categories/{id}`, `PUT/DELETE /categories/{id}/{archive}` in
      `crates/lanrurugi-api/src/categories.rs`
- [ ] T032 [US1] Implement `GET/PUT/DELETE /tankoubons/{id}`, `GET /tankoubons/{id}/full`,
      `GET /tankoubons/{id}/thumbnail`, `GET/PUT /tankoubons/{id}/progress/{page}`,
      `PUT/DELETE /tankoubons/{id}/{archive}` in `crates/lanrurugi-api/src/tankoubons.rs`
- [ ] T033 [P] [US1] Implement `GET /info` in `crates/lanrurugi-api/src/misc.rs`
- [ ] T034 [US1] Build the library-browse page (list archives/categories/tankoubons with existing
      metadata) in `frontend/src/pages/Library.tsx`
- [ ] T035 [US1] Run `quickstart.md` §1 against a real/representative legacy Redis dataset and
      confirm SC-001

**Checkpoint**: A pre-existing legacy library is fully browsable with no data loss, and every
legacy read/write/manage endpoint touching archives, categories, tankoubons, and stamps is
implemented — demoable MVP.

---

## Phase 4: User Story 2 - New archives are found, catalogued correctly, and never falsely merged (Priority: P1)

**Goal**: New files are auto-ingested (including manual upload); false-merge defect (the
rewrite's named motivation) is fixed.

**Independent Test**: `quickstart.md` §2 — drop two archives sharing leading content but
differing later; confirm both are catalogued as distinct entries, and that a byte-identical
re-scan of the same file still maps to the same archive (Clarifications Q2).

- [ ] T036 [US2] Implement the `notify`-based file watcher with a 3–5s debounce window in
      `crates/lanrurugi-scanner/src/watcher.rs`, per `research.md` §6
- [ ] T037 [US2] Implement the per-file ingestion timeout (~30s) in
      `crates/lanrurugi-scanner/src/pipeline.rs`
- [ ] T038 [US2] Implement the mpsc-based ingestion pipeline in
      `crates/lanrurugi-scanner/src/pipeline.rs`
- [ ] T039 [P] [US2] Implement rayon-parallel batch hashing for bulk scans (bridged via
      `spawn_blocking`) in `crates/lanrurugi-scanner/src/hashing.rs`, per constitution
      Principle III
- [ ] T040 [US2] Implement the "wait until file is stable" partial-write check in
      `crates/lanrurugi-scanner/src/watcher.rs` (FR-006)
- [ ] T041 [US2] Implement new-archive cataloguing (create an Archive record via
      `lanrurugi-storage`, size-aware ID by default) in `crates/lanrurugi-scanner/src/pipeline.rs`
- [ ] T042 [US2] Implement archive-format handling — `zip` crate for ZIP; shell out to
      `unrar`/`7z` for RAR/7z — and page-count extraction in
      `crates/lanrurugi-scanner/src/archive_format.rs`, per `research.md` §3
- [ ] T043 [US2] Implement thumbnail generation (`image` crate, rayon + `spawn_blocking`) in
      `crates/lanrurugi-scanner/src/thumbnail.rs`, per `research.md` §4
- [ ] T044 [US2] Implement an optional background full-verification job that re-checks
      cheap-fingerprint collisions against a stronger comparison (FR-007's deferred-verification
      clause) in `crates/lanrurugi-scanner/src/verify.rs`, using the T013 job-tracking
      abstraction to report progress
- [ ] T045 [P] [US2] Implement `POST /shinobu/rescan`, `GET /shinobu`, `POST /shinobu/stop`,
      `POST /shinobu/restart` endpoints in `crates/lanrurugi-api/src/shinobu.rs`
- [ ] T046 [US2] Implement `POST /archives/upload` and the `/tempfolder` staging endpoint in
      `crates/lanrurugi-api/src/upload.rs`, reusing T041's cataloguing logic
- [ ] T047 [P] [US2] Implement `POST /regen_thumbs` in `crates/lanrurugi-api/src/archives.rs`,
      reusing T043's thumbnail logic
- [ ] T048 [US2] Run `quickstart.md` §2 and confirm SC-002/SC-003 — both archives sharing a
      prefix but differing later appear distinct, **and** a byte-identical re-scan of an
      already-catalogued file still maps to the same archive (Clarifications Q2)

**Checkpoint**: New files (watched or manually uploaded) are auto-catalogued; the historical
false-merge defect no longer reproduces on fresh scans; a background job can double-check
borderline collisions without blocking ingestion.

---

## Phase 5: User Story 3 - Existing third-party tools keep working (Priority: P2)

**Goal**: Third-party clients using the legacy API contract work unmodified.

**Independent Test**: `quickstart.md` §3 — run an existing client's standard flow against
LANrurugi and confirm identical behavior.

- [ ] T049 [US3] Implement the search query grammar parser (namespace:tag, wildcards, boolean) in
      `crates/lanrurugi-search/src/grammar.rs`, per `research.md` §8
- [ ] T050 [US3] Port the Redis-based search execution (title sorted set, tag-set filtering,
      search-results cache keyed by filter/sort/flags) in
      `crates/lanrurugi-search/src/engine.rs`
- [ ] T051 [P] [US3] Implement `GET /search`, `GET /search/ids`, `GET /search/random`,
      `GET /search/cache` endpoints in `crates/lanrurugi-api/src/search.rs`, per
      `contracts/rest-api.md`
- [ ] T052 [P] [US3] Implement `PUT /archives/{id}/metadata` (tag/title/summary update) in
      `crates/lanrurugi-api/src/archives.rs`
- [ ] T053 [P] [US3] Implement `GET /archives/{id}/page`, `GET /archives/{id}/files`,
      `GET /archives/{id}/files/thumbnails` endpoints in `crates/lanrurugi-api/src/archives.rs`
- [ ] T054 [US3] Implement `GET /opds`, `GET /opds/{id}`, `GET /opds/{id}/pse` endpoints in
      `crates/lanrurugi-api/src/opds.rs`
- [ ] T055 [US3] Write a contract-replay test suite (records legacy request/response pairs and
      replays them against the new endpoints) in `crates/lanrurugi-server/tests/contract_api.rs`,
      to verify Principle II/FR-013 objectively
- [ ] T056 [US3] Run `quickstart.md` §3 against an existing third-party client and confirm SC-004

**Checkpoint**: Existing LANraragi-compatible clients (readers, scripts, OPDS) work against
LANrurugi without modification.

---

## Phase 6: User Story 4 - Automatic metadata enrichment via extensions (Priority: P2)

**Goal**: JS/TS plugins run sandboxed in Deno subprocesses to enrich archive metadata.

**Independent Test**: `quickstart.md` §4 — enable a plugin, enrich an untagged archive, induce a
timeout, confirm isolation.

- [ ] T057 [US4] Implement the Deno subprocess worker-pool manager in
      `crates/lanrurugi-plugin/src/pool.rs`, per `research.md` §7 and constitution Principle IV
- [ ] T058 [US4] Implement the newline-delimited JSON request/response protocol client in
      `crates/lanrurugi-plugin/src/protocol.rs`, per `contracts/plugin-protocol.md`
- [ ] T059 [US4] Implement permission-flag construction (`--allow-net=<hosts>` etc.) from a
      plugin's declared `plugin_info` in `crates/lanrurugi-plugin/src/permissions.rs`
- [ ] T060 [US4] Write the Deno dispatcher script (dynamic `import()` of the requested plugin
      module) in `crates/lanrurugi-plugin/dispatcher/dispatcher.ts`
- [ ] T061 [US4] Implement per-request timeout and failure isolation in
      `crates/lanrurugi-plugin/src/pool.rs` (FR-013)
- [ ] T062 [P] [US4] Implement `GET /plugins/{type}`, `POST /plugins/use`, `POST /plugins/queue`
      endpoints in `crates/lanrurugi-api/src/plugins.rs`
- [ ] T063 [US4] Write a sample metadata plugin exercising the protocol in
      `crates/lanrurugi-plugin/samples/sample-metadata-plugin.ts`
- [ ] T064 [P] [US4] Implement `POST /download_url` (download-type plugin trigger) in
      `crates/lanrurugi-api/src/plugins.rs`
- [ ] T065 [US4] Run `quickstart.md` §4 and confirm FR-012–FR-014 (enrichment, isolation,
      denied-by-default permissions)

**Checkpoint**: Metadata plugins run isolated from the host process; a hung/failing plugin cannot
degrade the rest of the system.

---

## Phase 7: User Story 5 - Back up and export the library (Priority: P2)

**Goal**: On-demand backup/export and restore, matching the legacy JSON shape, plus the
remaining `/database/*` maintenance endpoints.

**Independent Test**: `quickstart.md` §5 — trigger a backup, restore onto a fresh instance,
confirm exact match.

- [ ] T066 [US5] Implement the backup-JSON builder (categories/tankoubons/stamps/archives) in
      `crates/lanrurugi-backup/src/build.rs`, matching `data-model.md`'s Backup/Export document
      shape
- [ ] T067 [US5] Implement the consistent-snapshot guarantee (no partial/corrupt output under
      concurrent writes) in `crates/lanrurugi-backup/src/build.rs` (FR-010)
- [ ] T068 [US5] Implement restore-from-backup logic in `crates/lanrurugi-backup/src/restore.rs`
- [ ] T069 [P] [US5] Implement `POST /database/backup`, `GET /database/backup/{jobid}`,
      `POST /database/restore` endpoints in `crates/lanrurugi-api/src/database.rs`, reusing the
      T013 job-tracking abstraction for the `{jobid}` polling
- [ ] T070 [P] [US5] Implement `GET /database/stats`, `GET /database/isnew`,
      `POST /database/drop`, `POST /database/clean` in `crates/lanrurugi-api/src/database.rs`
- [ ] T071 [US5] Implement the generic `GET /minion/{jobid}`, `GET /minion/{jobid}/detail`,
      `GET /minion/{jobname}/queue` endpoints in `crates/lanrurugi-api/src/jobs.rs`, backed by
      the T013 job-tracking abstraction (also used by T069 and US6's rebuild-index job)
- [ ] T072 [US5] Build backup/restore UI controls in `frontend/src/pages/Settings/Backup.tsx`
- [ ] T073 [US5] Run `quickstart.md` §5 and confirm SC-009

**Checkpoint**: Users can protect and restore their tagging/organization work on demand; the full
`/database/*` maintenance surface and a generic job-status API are available.

---

## Phase 8: User Story 6 - Fix historical false duplicates after the fact (Priority: P3)

**Goal**: A guided, one-time repair for archives already merged by the legacy defect.

**Independent Test**: `quickstart.md` §6 — on a library with a seeded false-merge pair, run the
repair and confirm a clean split with no data loss on the correctly-tracked side.

- [ ] T074 [US6] Implement the rebuild-index core logic (recompute IDs, re-key records, split
      previously-merged pairs per `data-model.md`'s "Rebuild/Reindex operation") in
      `crates/lanrurugi-storage/src/rebuild.rs`
- [ ] T075 [US6] Implement reference updates (Category/Grouping/Stamp re-keying to the new
      Archive ID) in `crates/lanrurugi-storage/src/rebuild.rs` (FR-012)
- [ ] T076 [P] [US6] Implement `POST /database/rebuild-index` and its job-status polling (reusing
      the T013/T071 job-tracking abstraction) in `crates/lanrurugi-api/src/database.rs`, per
      `contracts/rest-api.md`
- [ ] T077 [US6] Wire the `lanrurugi rebuild-index` CLI subcommand in
      `crates/lanrurugi-server/src/cli/rebuild_index.rs`
- [ ] T078 [US6] Run `quickstart.md` §6 and confirm SC-005

**Checkpoint**: Historically-merged archives can be split apart without losing already-tracked
metadata.

---

## Phase 9: User Story 7 - Use the interface in your preferred language (Priority: P3)

**Goal**: UI available in the 14 legacy-supported languages, falling back to English.

**Independent Test**: `quickstart.md` §7 — switch language, confirm rendering and
missing-string fallback.

- [ ] T079 [US7] Convert the 14 legacy `locales/template/*.po` files to i18next-shaped JSON in
      `frontend/src/i18n/locales/*.json`, per `research.md` §10
- [ ] T080 [US7] Set up `react-i18next` with English fallback in `frontend/src/i18n/index.ts`
- [ ] T081 [P] [US7] Build the language-selector UI control in
      `frontend/src/components/LanguageSelector.tsx`
- [ ] T082 [US7] Wire existing UI strings through the i18n layer across `frontend/src/`
      (sweep pass — no untranslated literal strings left in components)
- [ ] T083 [US7] Run `quickstart.md` §7 and confirm SC-010

**Checkpoint**: Non-English-reading users get a fully localized interface, matching legacy
language coverage.

---

## Phase 10: User Story 8 - Verify and demonstrate concurrency/performance gains over the previous system (Priority: P2 — scheduled last; see Dependencies)

**Goal**: A benchmark suite proves LANrurugi's concurrency improvement over the legacy system,
**and** validates that interactive browsing/search stays responsive at the target scale (SC-008).

**Independent Test**: `quickstart.md` §8 — run the benchmark against a synthetic library at
target scale on both systems, confirm a report with concrete numbers.

- [ ] T084 [US8] Implement the synthetic-library generator (~100,000 archives, configurable
      smaller scale) in `bench/synthetic-library/generate.rs`
- [ ] T085 [P] [US8] Implement `criterion` microbenchmarks (hashing throughput, thumbnail
      decode/resize) in `bench/criterion/benches.rs`
- [ ] T086 [US8] Implement the cross-system comparison harness (drives both the legacy LANraragi
      instance and the new binary against the same synthetic library) in
      `bench/compare/run.rs`
- [ ] T087 [US8] Implement benchmark-report generation matching
      `contracts/benchmark-report.md` in `bench/compare/report.rs`
- [ ] T088 [US8] Implement a search/browse responsiveness benchmark against the synthetic
      library at the ~100,000-archive scale (SC-008) in `bench/compare/interactive_load.rs`,
      measuring `/search` and `/archives` pagination latency, not just bulk-operation throughput
- [ ] T089 [P] [US8] Implement `POST /bench/run`, `GET /bench/{reportid}` endpoints in
      `crates/lanrurugi-api/src/bench.rs`
- [ ] T090 [US8] Wire the `lanrurugi bench` CLI subcommand in
      `crates/lanrurugi-server/src/cli/bench.rs`
- [ ] T091 [US8] Run `quickstart.md` §8 (including the single-core-host edge case) and confirm
      SC-011 **and** SC-008 (via T088's interactive-load results)

**Checkpoint**: A published report demonstrates LANrurugi outperforming the legacy system on bulk
scan/ingestion and duplicate-repair reindex, and remaining responsive for interactive
browsing/search, on the same hardware and at the target library scale.

---

## Phase 11: Polish & Cross-Cutting Concerns

**Purpose**: Improvements spanning multiple user stories.

- [ ] T092 [P] Update `README.md`/`docs/` to reflect the shipped Phase 1 feature set
- [ ] T093 Code cleanup and refactoring pass across `crates/`
- [ ] T094 [P] Security hardening pass — plugin permission audit (`crates/lanrurugi-plugin/`),
      API-key handling review (`crates/lanrurugi-server/src/middleware/auth.rs`)
- [ ] T095 Run the full `quickstart.md` end-to-end across all 8 Phase 1 stories on a clean
      checkout
- [ ] T096 [P] Verify the Docker image (`Dockerfile` from T006) builds and runs
      `lanrurugi serve` correctly end-to-end

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — start immediately.
- **Foundational (Phase 2)**: Depends on Setup. **Blocks all user stories.**
- **User Stories (Phase 3–10)**: All depend on Foundational. Ordered by priority (P1 → P2 → P3)
  **with one explicit exception**: User Story 8 is nominally P2 but is scheduled last (Phase 10,
  after the P3 stories) because its acceptance scenarios (`quickstart.md` §8) require **both**
  User Story 2's ingestion pipeline and User Story 6's rebuild-index operation to exist so they
  can be benchmarked — this is a hard technical dependency, not a priority demotion.
- **Polish (Phase 11)**: Depends on all Phase 1 user stories (US1–US8) being complete.

### User Story Dependencies

- **US1 (P1)**: Foundational only.
- **US2 (P1)**: Foundational only. Independent of US1 (can proceed in parallel once Foundational
  is done), though both are typically demoed together since US1 is the MVP baseline.
- **US3 (P2)**: Foundational only.
- **US4 (P2)**: Foundational only.
- **US5 (P2)**: Foundational only (reuses T013's job-tracking abstraction).
- **US6 (P3)**: Foundational only (uses the ID algorithms from T009/T010 and, for job polling,
  T013/T071).
- **US7 (P3)**: Foundational only (frontend-only story).
- **US8 (P2, scheduled last)**: Foundational **and** US2 **and** US6 must be complete.

### Parallel Opportunities

- All Setup tasks marked `[P]` (T002–T007) can run in parallel once T001 exists.
- Foundational tasks T009/T010 (the two ID algorithms) and T018 (concurrency bridge) can run in
  parallel with each other; T011/T012 (Redis) are sequential; T013 (job-tracking) can proceed in
  parallel with T009/T010; T014–T017 depend on T011/T012.
- Once Foundational is done, **US1, US2, US3, US4, US5, US6, US7 can all be staffed and
  implemented in parallel** (they share no files) — only US8 must wait on US2 and US6.
- Within each story, tasks marked `[P]` touch different files (or independent endpoint additions
  within the same file, per this project's established task-granularity convention) and can run
  in parallel; unmarked tasks within a story are sequential (e.g. T041 needs T039's hashing and
  T036's watcher).

---

## Parallel Example: User Story 1

```bash
# After Foundational (Phase 2) completes, launch US1's parallel-safe tasks together:
Task: "Implement GET /archives listing endpoint in crates/lanrurugi-api/src/archives.rs"
Task: "Implement GET /archives/{id}/metadata endpoint in crates/lanrurugi-api/src/archives.rs"
Task: "Implement GET /categories and GET /tankoubons listing endpoints in crates/lanrurugi-api/src/categories.rs and crates/lanrurugi-api/src/tankoubons.rs"
Task: "Implement GET /archives/{id}/progress/{page} in crates/lanrurugi-api/src/archives.rs"
Task: "Implement GET /archives/{id} and DELETE /archives/{id} in crates/lanrurugi-api/src/archives.rs"
Task: "Implement GET /archives/untagged in crates/lanrurugi-api/src/archives.rs"
Task: "Implement GET /info in crates/lanrurugi-api/src/misc.rs"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1 (Setup) and Phase 2 (Foundational).
2. Complete Phase 3 (US1).
3. **STOP and VALIDATE**: run `quickstart.md` §1 against a real legacy library copy.
4. This alone is a demoable MVP: an existing library, fully browsable, zero data loss.

### Incremental Delivery

1. Setup + Foundational → foundation ready.
2. US1 → validate → demo (MVP).
3. US2 → validate → demo (the rewrite's headline bug fix is live).
4. US3, US4, US5 → validate each independently → demo (ecosystem/enrichment/safety-net parity).
5. US6, US7 → validate each independently → demo (historical repair, localization).
6. US8 → validate → demo (published proof of the concurrency improvement, including SC-008).
7. Polish.

### Parallel Team Strategy

With multiple contributors, after Foundational is done: one person on US1, one on US2 (the two
P1 stories, ship first), then remaining contributors split across US3/US4/US5/US6/US7 in
parallel — all independent of each other. US8 is picked up last by whoever finishes US2/US6
first, since it depends on both.

---

## Notes

- `[P]` tasks touch different files (or independent endpoint additions within the same file) and
  have no unfinished-task dependency.
- `[Story]` labels map every implementation task to spec.md's user stories for traceability.
- No dedicated TDD test-writing phase per story (not requested by spec.md); each story instead
  ends with its `quickstart.md` scenario as the acceptance checkpoint, plus one dedicated
  contract-replay test task (T055) for US3 specifically, since API-contract fidelity
  (Principle II) is otherwise unverifiable.
- Commit after each task or logical group, per constitution's lefthook/CI quality gates
  (T004/T007).
- User Story 9–10 (Phase 2 translation) tasks are intentionally absent — see
  `phase2-design-notes.md` for that exploratory material; it gets its own `/speckit-tasks` run
  once Phase 2 has its own plan.
- This revision closes every gap raised by the 2026-07-05 `/speckit-analyze` pass: full
  `contracts/rest-api.md` path coverage (T024–T033, T046–T047, T064, T070–T071), SC-008 coverage
  (T088), FR-007's deferred-verification clause (T044), and the U1 double-check on T048.
