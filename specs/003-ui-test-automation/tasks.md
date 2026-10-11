---

description: "Task list for Automated UI Test Coverage (003-ui-test-automation)"
---

# Tasks: Automated UI Test Coverage

**Input**: Design documents from `/specs/003-ui-test-automation/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, quickstart.md (all present; no
`contracts/` — this feature adds no new external interface, see plan.md's Project Structure note)

**Tests**: This feature's own deliverable *is* test files — there is no separate "product code"
to write per story, so the tasks below (writing `.test.ts(x)`/`.spec.ts` files) are the
implementation, not an optional TDD front-matter. Per each task's own regression fixture (where
applicable), the task includes reverting the referenced fix to confirm the new test actually fails
without it, then re-confirming it passes with the fix restored — this stands in for "write failing
tests first," adapted to a codebase where the underlying fix already exists.

**Scope**: This plan only. User Story 4 (spec) covers archive-format fixture breadth; Phase 2
(specs/004-ocr-manga-translation) is out of scope per constitution Principle VI and is not
represented anywhere below.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1, US2, US4 — US3 is Foundational, see
  plan.md/this file's own note below)
- Every task names an exact file path, per plan.md's Project Structure

## Path Conventions

Shared fixtures at `test-fixtures/archives/` (repo root, sibling to `crates/` and `apps/`); frontend
test code under `apps/frontend/tests/{unit,e2e}/` and `apps/frontend/{vitest,playwright}.config.ts`;
CI changes in `.github/workflows/ci.yml`. See `plan.md` § Project Structure for the full tree.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Install the two test frameworks and their exact dependencies (research.md §1-2),
before any fixture or test file can be written.

- [x] T001 Add `vitest@4.1.10`, `@testing-library/react@16.3.2`, `@testing-library/dom`,
      `@testing-library/jest-dom`, `jsdom@29.1.1` as devDependencies in
      `apps/frontend/package.json` (research.md §1) — reverify these are still the actual latest
      stable npm releases at implementation time per constitution's "verified at implementation
      time, not a remembered version" rule before installing
- [x] T002 Add `@playwright/test@1.61.1` as a devDependency in `apps/frontend/package.json`
      (research.md §2), reverifying its current npm version the same way as T001
- [x] T003 [P] Create `apps/frontend/vitest.config.ts` with `test.environment: 'jsdom'`,
      `test.globals: true`, and `test.include` pointed at `tests/unit/**/*.test.{ts,tsx}`,
      importing the app's existing `vite.config.ts` per research.md §1 (no separate Tailwind
      config needed under Vitest 4.1)
- [x] T004 [P] Create `apps/frontend/tests/unit/setup.ts` importing
      `@testing-library/jest-dom/vitest` (research.md §1's `/vitest` subpath note), referenced via
      `test.setupFiles` in `vitest.config.ts` (T003)
- [x] T005 Add `"test:unit": "vitest run"` and `"test:unit:watch": "vitest"` scripts to
      `apps/frontend/package.json`
- [x] T006 [P] Install `p7zip-full` in the project's dev container/CI image if not already present
      (needed only to *generate* fixtures in Phase 2 below, not to run the resulting test suites —
      research.md §5/§8) — check `Dockerfile.build`/`lanrurugi-dev` image first, since it may
      already need to be extended for other reasons, before adding a redundant install step. No
      package is installed for lzh/lha generation (research.md §5.1): no FOSS tool can write that
      format at all (verified — not just `7z`), so `sample.lzh`/`sample.lha` are produced by a
      small checked-in generator script (T012), not an external binary

**Checkpoint**: `pnpm install` succeeds with both frameworks present; no test files exist yet.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The shared fixture archives (needed by both US1's upload-related regressions and
US4's format matrix), the Playwright config (needed by every end-to-end scenario), and CI
integration (US3 — automatic execution is a precondition for every other story's tests to provide
any real protection, per spec User Story 3's own stated rationale). **No user story's test files
can be written until this phase is complete.**

### Fixture generation (research.md §5-8)

- [x] T007 [P] Create `test-fixtures/archives/` directory at repo root
- [x] T008 [P] Generate `test-fixtures/archives/sample.zip` (plain zip, UTF-8-flagged, 1-2 tiny
      placeholder images) using the existing `zip` crate pattern already used in
      `crates/lanrurugi-scanner`'s own test helpers, or a one-off script — one fixture per format
      in this task and the next, per spec FR-009
- [x] T009 [P] Generate `test-fixtures/archives/sample.{cbz,epub}` (same content as T008, renamed —
      `cbz`/`epub` are zip-family per `crates/lanrurugi-scanner/src/archive_format.rs`)
- [x] T010 [P] Generate `test-fixtures/archives/sample.{7z,cb7}` using the `7z` CLI (research.md
      §5)
- [x] T011 [P] Generate `test-fixtures/archives/sample.{tar,gz,bz2,xz}` (tar and tar's own
      compressed variants, per `ARCHIVE_EXTENSIONS`)
- [x] T012 [P] Generate `test-fixtures/archives/sample.{lzh,lha}` via a small checked-in generator
      (e.g. `scripts/gen-lzh-fixture.rs`, run with `rustc` or as a `cargo` example — no external
      crate dependency needed) that hand-builds a minimal valid LHA/LZH level-1 archive, method
      `-lh0-` (stored, no compression), containing one tiny placeholder image (research.md §5.1) —
      neither `7z` nor any Debian-packaged tool can *write* this format. Verify the output against
      `delharc` (add as a dev-only verification dependency, or a one-off `cargo run` check) and
      against this project's own `lanrurugi_scanner::archive_format::list_pages` before committing
      the fixture, per research.md §5.1's documented construction pitfalls
- [x] T013 [P] Generate `test-fixtures/archives/multivolume.7z.001`/`.002` (7z CLI `-v1k`, research.md
      §5) and `test-fixtures/archives/encrypted.7z` (7z CLI `-mhe=on`, research.md §5) — these two
      back spec FR-010/US4 Acceptance Scenarios 2-3, and their `data-model.md`
      `expected_behavior: current-behavior-locked` field must be set only after T033/T034 (below)
      actually record what today's real behavior is
- [x] T014 [P] Copy `crypted.rar`, `archive.part1.rar` + `100M.part00002.rar`, `unicode.rar`,
      `unicodefilename❤️.rar` from the `unrar` crate's bundled `data/` directory (research.md §6)
      into `test-fixtures/archives/rar/`, preserving their original filenames and recording their
      source (crate name + version) in a short `test-fixtures/archives/rar/SOURCES.md`
- [x] T015 Move `crates/lanrurugi-scanner/tests-fixtures/cjk-names.7z` to
      `test-fixtures/archives/cjk-names.7z` (`git mv`, preserving history) and update the
      `include_bytes!` path in `crates/lanrurugi-scanner/src/archive_format.rs`'s existing
      `list_pages_and_read_entry_recover_cjk_names_from_a_real_7z` test to point at the new
      location; delete `crates/lanrurugi-scanner/tests-fixtures/` if now empty (research.md §7)
- [x] T016 Hand-build `test-fixtures/archives/cjk-shiftjis-noflag.zip` — a raw zip with the UTF-8
      general-purpose-bit-flag left unset and Shift-JIS-encoded internal filenames (research.md
      §5.2), reusing the same byte-construction approach already proven in
      `crates/lanrurugi-scanner/src/archive_format.rs`'s existing
      `list_pages_recovers_shift_jis_filename_without_utf8_flag` test fixture-building helper

### Playwright/CI plumbing

- [x] T017 Create `apps/frontend/playwright.config.ts`: `projects: [chromium, firefox]` (research.md
      §2, WebKit omitted), `retries: process.env.CI ? 1 : 0`, `use: { screenshot:
      'only-on-failure', trace: process.env.CI ? 'on-first-retry' : 'retain-on-failure' }`
      (research.md §2), `testDir: './tests/e2e'`
- [x] T018 Add a `webServer` array to `playwright.config.ts` (T017) starting Redis
      (`redis-server`), the compiled `lanrurugi-server` binary, and a frontend preview server, each
      with its own command + port-based readiness probe + `reuseExistingServer: !process.env.CI`
      (research.md §3)
- [x] T019 Create `apps/frontend/tests/e2e/global-setup.ts`: explicitly poll the backend's health
      before proceeding (research.md §3's caveat that `globalSetup` ordering vs. `webServer`
      readiness is not guaranteed), then flush/seed whatever baseline Redis state end-to-end tests
      need; wire it via `globalSetup` in `playwright.config.ts` (T017)
- [x] T020 Create `apps/frontend/tests/e2e/global-teardown.ts` and wire it via `globalTeardown` in
      `playwright.config.ts` (T017) — implements spec FR-011's "clean, known state before the next
      run begins". Skip the actual cleanup (but still run, so the process doesn't error) when
      `process.env.KEEP` is set, implementing the spec's Edge Case allowance for deliberately
      inspecting a failed run's environment afterward — the *next* run must still clean up
      normally regardless of whether the previous one was kept, so `KEEP` only ever affects the
      run it's set for, never becomes a persistent mode
- [x] T021 Create a worker-scoped Playwright fixture (e.g. `apps/frontend/tests/e2e/fixtures.ts`)
      that calls Redis `SELECT` keyed by `testInfo.parallelIndex` (research.md §4) so concurrent
      workers use isolated logical databases (spec FR-014); export a custom `test` from this file
      for all `.spec.ts` files to import instead of Playwright's base `test`
- [x] T022 Create `apps/frontend/tests/e2e/fixturePath.ts` (or equivalent shared helper), a single
      `fixturePath(name: string)` resolver into `test-fixtures/archives/` used by e2e tests only
      (data-model.md's Scenario Contract — no per-file hand-rolled relative paths). The unit layer
      does not need this helper: per spec User Story 1 Acceptance Scenario 3, unit-level scenarios
      are pure logic with no I/O, so none of T035-T037 reference `test-fixtures/archives/`
- [x] T023 Add `"test:e2e": "playwright test"` script to `apps/frontend/package.json`
- [x] T024 Modify `.github/workflows/ci.yml`'s existing `frontend` job to add a `pnpm run
      test:unit` step after the existing `eslint`/`build` steps
- [x] T025 Add a new `e2e` job to `.github/workflows/ci.yml`: checkout, `mise-action`, `pnpm
      install`, build the Rust backend (release or debug — confirm which this project's own CI
      convention favors), build the frontend, a Redis service container matching the existing
      `rust` job's service-container pattern already in this file, then `pnpm run test:e2e`;
      upload the Playwright HTML report + traces as a workflow artifact on failure so a maintainer
      can retrieve FR-015's captured evidence without local reproduction
- [x] T025a Add `[tasks.test-frontend-unit]` and `[tasks.test-frontend-e2e]` to `.mise.toml`,
      matching the naming/doc-comment style of the existing `[tasks.test]`/`[tasks.clippy]`
      entries: `test-frontend-unit` runs `pnpm -C apps/frontend run test:unit`;
      `test-frontend-e2e` runs `pnpm -C apps/frontend run test:e2e`, passing through `KEEP` from
      the calling shell's environment unmodified (T020's teardown opt-out) so `KEEP=1 mise run
      test-frontend-e2e` skips cleanup for a run a maintainer wants to inspect afterward, while a
      bare `mise run test-frontend-e2e` always starts from a clean/torn-down state per FR-011 —
      giving this feature the same one-command entry point this project's Rust
      `mise run test`/`clippy`/`build` tasks already have, rather than requiring a maintainer to
      remember raw `pnpm`/`playwright` invocations

**Checkpoint**: `mise run test-frontend-e2e` (even with zero `.spec.ts` files yet) successfully
starts/tears down Redis + backend + frontend without error, and `KEEP=1 mise run
test-frontend-e2e` leaves the environment up afterward for inspection; `mise run
test-frontend-unit` runs cleanly with zero test files; CI's new `e2e` job is green on a no-op run;
every fixture archive listed in data-model.md's Fixture Archive table exists on disk.

---

## Phase 3: User Story 1 - Catch a regression before it reaches a real session (Priority: P1) 🎯 MVP

**Goal**: Every known-bad scenario this project has already found and fixed once has a
corresponding automated test that fails against a reverted version of the fix (spec SC-001).

**Independent Test**: Revert any one of the six Regression Fixtures below and confirm the
corresponding test fails with a clear indication of what broke; re-apply the fix and confirm it
passes again (spec User Story 1's own Independent Test; quickstart.md §3).

### Implementation for User Story 1

- [x] T025b [P] [US1] Write `apps/frontend/tests/e2e/login.spec.ts` covering the sign-in journey
      required by spec FR-003: submit valid credentials and confirm the session reaches an
      authenticated state (e.g. a protected page/API call succeeds without a 401); submit invalid
      credentials and confirm the UI surfaces a clear failure rather than a silent no-op
- [x] T026 [P] [US1] Write `apps/frontend/tests/e2e/categories.spec.ts` covering the category
      `pinned`-field save regression (data-model.md Regression Fixture #1): create a static
      category, toggle "固定此分类", confirm the save request succeeds (200, not 422) and the
      toggle persists across a page reload. Verify by temporarily reverting `Categories.tsx`'s
      `saveDetails` to send `'1'`/`'0'` and confirming this test fails, then restoring the fix
- [x] T027 [P] [US1] Write `apps/frontend/tests/e2e/upload.spec.ts` covering the large-archive
      upload regression (data-model.md Regression Fixture #2): upload a fixture archive larger
      than axum's 2MB `DefaultBodyLimit` default (a padded copy of a `test-fixtures/archives/`
      fixture, or a dedicated oversized fixture) and confirm it succeeds rather than failing with
      "Error parsing `multipart/form-data` request". Verify by temporarily removing the
      `DefaultBodyLimit::max(...)` layer in `crates/lanrurugi-api/src/upload.rs` and confirming
      this test fails, then restoring it
- [x] T028 [P] [US1] Write `apps/frontend/tests/e2e/archive-lifecycle.spec.ts` covering the
      orphaned-search-index regression (data-model.md Regression Fixture #3): upload an archive,
      delete it, then click the UI's random-archive navigation link/button (spec FR-003 — a real
      click-through, not a direct `/api/search/random` HTTP call) and confirm the app lands on a
      valid archive rather than an empty/broken result due to a lingering ghost id in
      `LRR_TANKGROUPED`/`LRR_UNTAGGED`/`LRR_NEW`. Verify by temporarily reverting
      `crates/lanrurugi-search/src/indexer.rs`'s `remove_archive_index` call site in
      `delete_archive` and confirming this test fails, then restoring it
- [x] T029 [P] [US1] Write `apps/frontend/tests/e2e/reader.spec.ts` covering the icon-spacing
      regression (data-model.md Regression Fixture #4): open the reader, measure the gap between
      adjacent toolbar icons via `getBoundingClientRect()`, assert it is 3px (not 0px). Verify by
      temporarily removing the `marginRight: 3` style from Reader.tsx's toolbar `<a>` elements and
      confirming this test fails, then restoring it
- [x] T030 [US1] Extend `apps/frontend/tests/e2e/reader.spec.ts` (T029) covering the dead-whitespace
      regression (data-model.md Regression Fixture #5): open a page shorter than 75vh, confirm
      `#i3`'s rendered height matches the image's own height (no lingering `.loading` class /
      `min-height: 75vh` gap below it). Verify by temporarily making `#i3`'s className
      unconditionally include `'loading'` again and confirming this test fails, then restoring it
- [x] T030a [US1] Extend `apps/frontend/tests/e2e/reader.spec.ts` (T029) covering fit-mode switching
      required by spec FR-003 (not a regression fixture — new coverage for existing functionality):
      change the fit-mode setting (e.g. width-fit vs. height-fit vs. container-width) via the
      settings panel, confirm the rendered page image's computed size/aspect changes accordingly,
      and confirm the choice persists across a page reload
- [x] T031 [P] [US1] Extend `apps/frontend/tests/e2e/archive-formats.spec.ts` (created fully in
      Phase 5/US4, but this one scenario belongs to US1) covering the CJK-mojibake-and-locale
      regression (data-model.md Regression Fixture #6): upload
      `test-fixtures/archives/cjk-shiftjis-noflag.zip` (T016) and confirm the reader shows the
      correctly-decoded filename, not mojibake or an error. Verify by temporarily reverting
      `crates/lanrurugi-scanner/src/archive_format.rs`'s `Utf8LocaleGuard` to use `newlocale`'s
      empty-string argument instead of the hardcoded `"C.utf8"` and confirming this test fails
      under this project's own dev container (which has no `LANG` env var set), then restoring it
- [x] T031a [P] [US1] Write `apps/frontend/tests/e2e/upload.spec.ts`'s duplicate-vs-race scenario
      covering the upload-vs-watcher ingestion race regression (data-model.md Regression Fixture
      #7, found during this feature's own implementation): upload a never-before-seen archive and
      confirm it succeeds (`success: 1`), not a spurious 409 "already exists" — repeat several
      times in a tight loop (the race is intermittent, not deterministic) to catch a regression
      that only manifests occasionally. Verify by temporarily reverting
      `crates/lanrurugi-api/src/upload.rs`'s staging-path-then-move order back to writing directly
      into `archive_dir` before calling `ingest_file`, confirming this test fails (at least
      intermittently across repeated runs) under this project's own dev container with the file
      watcher enabled (the default), then restoring the fix
- [x] T032 [US1] Write `apps/frontend/tests/unit/fileInfoText.test.ts` covering the reader's
      file-info text construction (`fileInfoText()` in Reader.tsx) — single-page and double-page-
      spread cases, matching legacy's `"filename :: WxH :: sizeKB"` format
- [x] T033 [US1] Write `apps/frontend/tests/e2e/archive-formats.spec.ts`'s multi-volume scenario
      (data-model.md's `current-behavior-locked` Fixture Archive entry): upload
      `test-fixtures/archives/multivolume.7z.001` and record whatever this project's actual current
      behavior is (full assembly, first-volume-only, or rejection) as the test's assertion —
      this task's *output* (which behavior was observed) also finalizes T013's
      `expected_behavior` documentation for this fixture
- [x] T034 [US1] Write `apps/frontend/tests/e2e/archive-formats.spec.ts`'s encrypted-archive
      scenario (data-model.md's `current-behavior-locked` Fixture Archive entry): upload
      `test-fixtures/archives/encrypted.7z` and record whatever this project's actual current
      behavior is as the test's assertion — finalizes T013's `expected_behavior` documentation for
      this fixture

**Checkpoint**: Running `pnpm run test:e2e` and `pnpm run test:unit` together exercises every
Regression Fixture in data-model.md; deliberately reverting any one of the six underlying fixes
causes exactly one test to fail with a clear name/message (quickstart.md §3-4).

---

## Phase 4: User Story 2 - Verify a fix works without manual re-testing every related flow (Priority: P2)

**Goal**: A maintainer can run a scoped subset of either test layer (a single page/flow) rather
than the entire suite, and that subset actually exercises real interaction paths, not just
render-without-crashing checks (spec FR-005, quickstart.md §5).

**Independent Test**: Run `pnpm exec vitest run crossArchiveNav` and `pnpm exec playwright test
upload.spec.ts` independently and confirm each returns a result scoped to just that area (spec
User Story 2's own Independent Test).

### Implementation for User Story 2

- [x] T035 [P] [US2] Write `apps/frontend/tests/unit/useReaderSettings.test.ts`: persistence
      round-trip (localStorage read/write) for every `ReaderSettings` field, matching the hook's
      actual current field set
- [x] T036 [P] [US2] Write `apps/frontend/tests/unit/useReaderNavigation.test.ts`: page-navigation
      and spread-computation logic (single/double-page mode, manga-mode direction flip, widespread
      auto-fallback)
- [x] T037 [P] [US2] Write `apps/frontend/tests/unit/crossArchiveNav.test.ts`: cross-archive
      navigation resolution — `resolveAdjacentArchive`'s within-page-list stepping and
      edge-of-page cache-window-shift behavior, and `setupArchiveNavigation`'s same-origin-referrer
      gating
- [x] T038 [US2] Add `test:` prefixed tags (per data-model.md's Test Scenario `tags` field — at
      minimum `reader`, `categories`, `upload`, `archive-formats`) to every `.spec.ts`/`.test.ts`
      file written so far (T026-T037), enabling `--grep`/tag-scoped runs without a separate scoping
      mechanism
- [x] T039 [US2] Extend `apps/frontend/tests/e2e/categories.spec.ts` (T026) with a second scenario
      exercising the Categories page's other per-field autosave paths (name, predicate) via real
      typing + blur + network-request assertions, not just the pinned-field checkbox — demonstrating
      real interaction-path coverage per spec User Story 2 Acceptance Scenario 2

**Checkpoint**: Both `vitest run <module>` and `playwright test <file>.spec.ts` return scoped
results without invoking the full respective suite.

---

## Phase 5: User Story 4 - Exercise every archive format the library actually has to handle (Priority: P2)

**Goal**: Every archive format/variant the library supports has at least one fixture-backed
upload/reading test (spec SC-006).

**Independent Test**: For each fixture in `test-fixtures/archives/`, upload it and confirm the
result matches what the suite has recorded as expected for that variant (spec User Story 4's own
Independent Test; quickstart.md §6).

### Implementation for User Story 4

- [x] T040 [US4] Complete `apps/frontend/tests/e2e/archive-formats.spec.ts` (already holds the US1
      CJK scenario from T031 and the two `current-behavior-locked` scenarios from T033/T034): add
      one scenario per remaining plain-format fixture from T008-T012 (zip, cbz, epub, rar, cbr,
      7z, cb7, lzh, lha, tar, gz, bz2, xz) — upload, confirm successful ingestion, confirm every
      page is viewable in the reader (spec FR-009)
- [x] T041 [P] [US4] Add a RAR-specific scenario to `archive-formats.spec.ts` using
      `test-fixtures/archives/rar/unicode.rar` (T014), confirming its bundled non-ASCII (Latin/
      symbol/emoji) filenames decode correctly — general non-ASCII-filename coverage (spec FR-010).
      `sample.rar`/`sample.cbr` themselves (plain-format coverage in T040) were generated using
      RARLab's own official freeware `rar` CLI (downloaded directly from rarlab.com under its
      try-before-you-buy license, used once as a one-time maintainer task per research.md §6, then
      discarded — not checked in, not depended on for regeneration), so plain RAR/CBR upload+read
      coverage is real, not skipped. A genuine CJK-mojibake regression extension to RAR specifically
      (matching User Story 4 Acceptance Scenario 4's "not only the format the original bug happened
      to be found in") is still not attempted with hand-rolled bytes — RAR's format is materially
      more complex than LZH, and reusing the licensed tool for that would exceed the one-time/
      one-fixture use already made

**Checkpoint**: All 13 `lanrurugi-scanner`-supported formats plus the three higher-risk shapes
(multi-volume, encrypted, non-ASCII filename) have fixture-backed coverage; SC-006 is met.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Final validation against quickstart.md and spec.md's success criteria as a whole,
after all stories are individually complete.

- [x] T042 [P] Run `pnpm run test:unit` twice in a row and confirm results are identical for
      unchanged code (spec SC-007 for the unit layer)
- [x] T043 [P] Run `pnpm run test:e2e` twice in a row and confirm results are identical for
      unchanged code (spec SC-007, FR-011; quickstart.md §7)
- [x] T044 Time a full `pnpm run test:unit` run and confirm it completes in under one minute
      (SC-002); time a full `pnpm run test:e2e` run and confirm it completes in under ten minutes
      (SC-003) — if either exceeds budget, note the finding rather than silently letting it pass
      unmeasured
- [x] T045 Run the full quickstart.md validation end-to-end (all numbered sections, §1-§8 including
      §7a) as the final acceptance check for this feature as a whole
- [x] T046 [P] Update this project's own `README.md`/CLAUDE.md-adjacent contributor docs (if any
      exist describing "how to run tests") to mention `mise run test-frontend-unit`/
      `test-frontend-e2e`, so this feature's own existence is discoverable by a future contributor
      without reading this tasks.md
- [x] T047 Configure the `e2e` job (T025) and the existing `frontend` job's new `test:unit` step
      (T024) as required status checks for the frontend branch-protection rule (or equivalent CI
      gate this project already uses), so spec SC-004's "100% of proposed frontend changes have
      both test layers run against them, with no path to merge that skips either" is an enforced
      gate, not just an available job a contributor could ignore
- [x] T048 [P] Add a short note to `lefthook.yml` (as a comment) or this feature's own
      documentation recording that `test:unit`/`test:e2e` are deliberately *not* pre-commit hooks —
      consistent with the constitution's own guidance that slow checks (full test suites) belong at
      pre-push/CI rather than every commit, matching the existing `rust-check`/`frontend-lint`
      hooks' scope (fast static checks only) — a deliberate choice, not a coverage gap

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories (fixture
  files and Playwright/CI plumbing are load-bearing for every story's tests)
- **User Story 1 (Phase 3)**: Depends on Foundational completion. No dependency on US2/US4
- **User Story 2 (Phase 4)**: Depends on Foundational completion. Independent of US1/US4 content-
  wise, though T038/T039 touch files US1 created (tagging existing files, extending
  `categories.spec.ts`) — sequence after Phase 3 in practice even though not a hard blocker
- **User Story 4 (Phase 5)**: Depends on Foundational completion (all fixture files must exist).
  Builds on `archive-formats.spec.ts`, which US1 (T031/T033/T034) already started — sequence
  after Phase 3
- **Polish (Phase 6)**: Depends on all desired user stories being complete

### User Story Dependencies

- **User Story 1 (P1)**: Can start after Foundational (Phase 2). No dependency on US2/US4
- **User Story 2 (P2)**: Can start after Foundational (Phase 2); T038/T039 assume US1's files
  already exist, so schedule after Phase 3 in single-contributor execution
- **User Story 4 (P2)**: Can start after Foundational (Phase 2); assumes `archive-formats.spec.ts`
  already exists from US1's T031/T033/T034, so schedule after Phase 3 in single-contributor
  execution

### Within Each User Story

- Fixture/config prerequisites (Phase 2) before any story's test files
- Independent per-flow test files (T026-T030) can be written in parallel — different files
- `archive-formats.spec.ts` (T031, T033, T034, T040, T041) is a shared file across US1/US4 — these
  tasks are NOT parallel with each other (same file), even though marked otherwise across
  different stories

### Parallel Opportunities

- All fixture-generation tasks in Phase 2 (T007-T016) marked [P] can run in parallel — independent
  files
- T026-T030 (US1, different `.spec.ts` files) can run in parallel
- T035-T037 (US2, different `.test.ts` files) can run in parallel
- T041 (US4) can run in parallel with T040 only if scoped to a distinct scenario within the same
  file is coordinated (same file — treat as sequential in practice despite the [P] marker's
  "different files" default rule, since both extend `archive-formats.spec.ts`)

---

## Parallel Example: User Story 1

```bash
# Launch independent per-flow regression tests together:
Task: "Write apps/frontend/tests/e2e/categories.spec.ts covering the pinned-field regression"
Task: "Write apps/frontend/tests/e2e/upload.spec.ts covering the body-size-limit regression"
Task: "Write apps/frontend/tests/e2e/archive-lifecycle.spec.ts covering the orphaned-index regression"
Task: "Write apps/frontend/tests/e2e/reader.spec.ts covering the icon-spacing regression"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — fixtures, Playwright config, and CI integration
   block every story; US3's "runs automatically" requirement is delivered here, not as its own
   phase, since it has no independent test content of its own beyond "the CI job exists and is
   green")
3. Complete Phase 3: User Story 1 — all six known Regression Fixtures covered
4. **STOP and VALIDATE**: quickstart.md §3-4 against the MVP scope
5. This alone satisfies spec SC-001 (every already-fixed bug has a corresponding test) and is a
   complete, demonstrable increment

### Incremental Delivery

1. Setup + Foundational → CI runs an (initially empty) test suite automatically on every PR
2. User Story 1 → the six known regressions are locked in (MVP) → demo: revert one fix, watch CI
   catch it
3. User Story 2 → scoped local runs work, tagging in place → demo: `vitest run <module>` in
   seconds
4. User Story 4 → full format-matrix coverage → demo: SC-006 met, every supported format has a
   fixture-backed test
5. Polish → cross-run consistency and timing budgets confirmed

### Parallel Team Strategy

With multiple contributors, after Foundational is complete:

- Contributor A: User Story 1 (six regression scenarios, five different files)
- Contributor B: User Story 2 (three unit test files, independent of A's files)
- User Story 4 depends on `archive-formats.spec.ts` existing from User Story 1's T031/T033/T034 —
  best scheduled after A finishes that file, not fully parallel with User Story 1

---

## Notes

- [P] tasks = different files, no dependencies — except where explicitly flagged otherwise (the
  shared `archive-formats.spec.ts` file across US1/US4)
- [Story] label maps task to specific user story for traceability; Foundational-phase tasks
  (including US3's CI-automation requirement) carry no story label per the template's own
  convention for blocking prerequisites
- Every regression-fixture task (T026-T031) includes the "revert the fix, confirm the test fails,
  restore the fix" step as part of the task itself — this is this feature's substitute for
  "write failing tests first," since the product code these tests exercise already exists
- Commit after each task or logical group, consistent with this project's existing convention
