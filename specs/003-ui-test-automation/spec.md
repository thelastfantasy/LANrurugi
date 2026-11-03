# Feature Specification: Automated UI Test Coverage

**Feature Branch**: `003-ui-test-automation`

**Created**: 2026-07-18

**Status**: Draft

**Input**: User description: "Add comprehensive automated UI test coverage for the LANrurugi
frontend (apps/frontend/), additive to Phase 1. Two layers: (1) fast unit-level coverage of pure
frontend logic — hooks, state machines (cross-archive navigation resolution), and
formatting/decoding helpers — none of this exists today, the whole frontend has zero automated
test coverage. (2) end-to-end browser coverage of the key user journeys this project's manual QA
sessions have repeatedly had to re-verify by hand: login, category creation/edit (including the
pinned-field save path), large archive upload, reader pagination/layout (icon spacing, fit-mode
switching, the loading-state/dead-whitespace bug), and archive deletion/random-archive navigation.
The goal is to replace ad-hoc manual browser-driven verification with a repeatable, CI-runnable
regression suite so bugs fixed once don't silently regress. Must not block or be blocked by Phase
1 (constitution Principle VI) since Phase 1 implementation is already complete — this is testing
infrastructure layered on top of already-shipped code, not new product functionality."

## Clarifications

### Session 2026-07-18

- Q: How should a flaky (nondeterministic) end-to-end test failure be handled? → A: Auto-retry a
  failed end-to-end test once; only a second consecutive failure counts as a real failure.
- Q: Which browser engine(s) must the end-to-end layer cover? → A: Chromium and Firefox.
- Q: How should concurrently-running end-to-end tests avoid contending for/corrupting shared
  backend (Redis) state? → A: Each parallel worker gets its own isolated backend/Redis
  instance-or-logical-database; tests may still run in parallel, but with no shared mutable state
  between workers.
- Q: What must be captured automatically when an end-to-end test fails? → A: A screenshot and a
  replayable trace (full DOM/network state around the failure), not just log text.
- Q: Does new frontend code need to meet a hard numeric coverage threshold to be merged? → A: No
  hard percentage threshold; coverage requirements stay scoped to key user journeys and
  already-fixed bugs (per the existing Assumptions section), growing organically rather than being
  gated on an abstract number.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Catch a regression before it reaches a real session (Priority: P1)

A maintainer changes frontend code (fixing a bug, refactoring a component, upgrading a
dependency) and wants to know, before spending time manually clicking through the app, whether
that change broke something that was already working — in particular, one of the specific bugs
this project has already found and fixed once through manual browser testing (icon spacing, the
reader's dead-whitespace-below-image bug, the category pinned-field save failure, the upload
body-size limit, archive-delete leaving orphaned search-index entries).

**Why this priority**: This is the entire reason automated coverage is being added. Every prior
bug in this project's history was found by a human manually driving a browser end-to-end — a slow,
easy-to-skip step that has already let bugs slip through more than once when re-verification
wasn't done after a later change. A regression suite that reproduces those specific known-bad
scenarios is the highest-value thing this feature can deliver, because it directly targets defects
that have already cost real debugging time once.

**Independent Test**: Introduce a deliberate regression of any one of the previously-fixed bugs
(e.g. revert the icon-spacing fix, or the loading-state cleanup) and confirm the test suite fails
with a clear indication of which scenario broke, before any human opens a browser.

**Acceptance Scenarios**:

1. **Given** the current, correct frontend code, **When** the full test suite runs, **Then** every
   test passes.
2. **Given** a change that reintroduces a previously-fixed UI bug, **When** the test suite runs,
   **Then** at least one test fails, and its name/output makes clear which behavior regressed.
3. **Given** a maintainer has just made a frontend change, **When** they want fast feedback on pure
   logic (no browser needed), **Then** results are available in seconds, not minutes.
4. **Given** a maintainer wants confidence in a real user-facing flow (not just isolated logic),
   **When** the relevant end-to-end scenario runs, **Then** it exercises the actual rendered page
   in a real browser against a real running backend, the same way this project's manual QA
   sessions have verified fixes so far — not a mocked/simulated approximation of the UI.

---

### User Story 2 - Verify a fix works without manual re-testing every related flow (Priority: P2)

A maintainer just fixed a bug (e.g. today's category-edit 422, or the reader's overlay-opacity
issue) and wants confidence that the fix is both correct and complete, without having to remember
and manually re-click through every adjacent flow that could plausibly have been affected by the
same change.

**Why this priority**: Directly reduces the verification cost of every future fix in this project.
It's secondary to Story 1 (having *any* regression coverage at all) because it's about efficiency
of ongoing work rather than the foundational safety net, but it's the day-to-day payoff that
justifies the investment.

**Independent Test**: Pick a recently-fixed bug, write or run the corresponding test for it, and
confirm it passes against the fixed code and fails against a reverted version — demonstrating the
test actually exercises the fix, not just a coincidentally-passing assertion.

**Acceptance Scenarios**:

1. **Given** a bug fix has just been made, **When** the maintainer runs the relevant subset of
   tests (not the entire suite), **Then** they get a pass/fail answer for that specific area
   without needing to run everything.
2. **Given** a fix touches a page with several interactive elements (e.g. the Reader's settings
   overlay, or the Categories page's per-field autosave), **When** the corresponding test scenario
   runs, **Then** it exercises the actual user interaction path (clicking, typing, waiting for the
   resulting network request/UI update) rather than only checking that a component renders without
   crashing.

---

### User Story 3 - Run the suite automatically on every change, not just when someone remembers to (Priority: P2)

A maintainer opens a pull request (or pushes a commit) and wants the test suite to run
automatically, the same way this project's existing Rust/frontend build-and-lint checks already
do, so regression coverage isn't something that has to be manually remembered and invoked.

**Why this priority**: Coverage that exists but isn't consistently run provides little real
protection — this project's own history shows manual verification gets skipped under time
pressure. Automatic, unavoidable execution on every change is what turns the test suite from
"available if you remember" into an actual safety net. It's P2 rather than P1 because the tests
having correct, meaningful assertions (Stories 1-2) has to exist first for automatic execution to
be worth anything.

**Independent Test**: Open a change that intentionally breaks a covered scenario and confirm the
automated check reports failure and is visible on the change itself, without any manual step to
trigger it.

**Acceptance Scenarios**:

1. **Given** a code change is proposed, **When** it is submitted for review, **Then** the full fast
   (unit-level) test suite runs automatically and its result is visible without a manual command.
2. **Given** a code change affects frontend behavior, **When** it is submitted for review, **Then**
   the relevant end-to-end scenarios run automatically against a real build of the change, and
   failures block or clearly flag the change before it can be merged.

---

### User Story 4 - Exercise every archive format the library actually has to handle (Priority: P2)

A maintainer wants confidence that upload, ingestion, and reading work correctly not just for a
plain zip, but for the full range of archive shapes real users' libraries actually contain — this
project has already found and fixed a real defect (mojibake in non-UTF-8-flagged CJK filenames)
that only manifests inside a real archive of a specific format, not in a plain-zip test case, so
format coverage is treated as its own first-class concern rather than an afterthought of "some
upload test."

**Why this priority**: Archive-format handling is one of the highest-risk areas in this project —
it's the boundary where an entire external file (of a format this project doesn't control the
creation of) enters the system, and it's already produced at least one real, previously-undetected
bug. It's P2 rather than P1 because Stories 1-3 (having any regression suite at all, running it
automatically) are the precondition for this coverage to matter; this story is about *breadth*
within that suite, not its existence.

**Independent Test**: For each archive format/variant in scope, upload a fixture archive of that
exact format and confirm the result (successful ingestion and readable pages, or today's actual
current failure/rejection behavior for variants not yet fully supported) matches what the suite
has recorded as this project's expected behavior for that variant.

**Acceptance Scenarios**:

1. **Given** a fixture archive in any of this project's supported formats (zip, cbz, epub, rar,
   cbr, 7z, cb7, lzh, lha, tar, gz, bz2, xz), **When** it is uploaded, **Then** it is ingested
   successfully and every page inside it is viewable in the reader.
2. **Given** a fixture archive that is a multi-volume/split archive (a single logical archive
   spread across multiple numbered volume files), **When** the first volume is uploaded/ingested,
   **Then** the test suite documents and locks in whatever this project's *current* behavior
   actually is (full multi-volume support, partial/first-volume-only handling, or a clear rejection)
   — this story covers characterizing and guarding today's real behavior, not prescribing what that
   behavior should be; multi-volume archive handling itself (if it needs to change) is a product
   decision tracked outside this testing-infrastructure feature.
3. **Given** a password-protected/encrypted archive, **When** it is uploaded, **Then** the test
   suite documents and locks in whatever this project's *current* behavior actually is — this
   story covers characterizing and guarding today's real behavior (so a future change to it is a
   deliberate, visible decision rather than a silent regression), not prescribing what encrypted-
   archive handling should look like; encrypted-archive support itself is a product decision
   tracked outside this testing-infrastructure feature.
4. **Given** any fixture archive whose internal filenames use a non-ASCII encoding (the CJK
   mojibake case this project already fixed once, for zip specifically), **When** it is uploaded,
   **Then** filenames are decoded correctly across every archive format the fix is expected to
   apply to (not only the format the original bug happened to be found in), since the underlying
   decoding logic is format-independent and a format-specific regression would otherwise go
   unnoticed.

---

### Edge Cases

- What happens when a test that depends on a real backend/database can't reach one (e.g. local
  development without the full stack running)? → Unit-level tests (Story 1's "seconds, not
  minutes" tier) MUST NOT require a running backend at all. End-to-end tests that do require one
  MUST fail with a clear "backend/database unavailable" message rather than a confusing unrelated
  error, consistent with this project's existing pattern of gracefully skipping/failing clearly
  when `LANRURUGI_TEST_REDIS_URL` isn't set for backend tests.
- What happens when an end-to-end scenario depends on test data (a specific archive, category, or
  account state) that doesn't already exist? → The scenario MUST set up whatever state it needs
  itself (or use a fixture provisioned specifically for tests) rather than depending on a
  hand-curated, easily-drifting shared environment — this project's own manual QA sessions have
  repeatedly hit exactly this problem (stale/incomplete test data producing false bug reports).
- What happens when a UI element's exact pixel position legitimately changes on purpose (a real
  redesign, not a regression)? → The relevant test(s) MUST be updated as part of that same change,
  not treated as an unrelated failure to work around.
- What happens when the same underlying bug could be caught at either the unit level or the
  end-to-end level? → Prefer the fastest layer that can actually catch it; end-to-end coverage is
  reserved for behavior that only manifests through real rendering/network/browser interaction
  (layout, cross-request sequencing), not for logic that's equally verifiable in isolation.
- What happens to fixture archives and the state they create (uploaded archives, categories,
  Redis entries) after a test run? → Each run MUST leave the environment it used in a known,
  clean state before the next run starts — either by tearing down everything it created, or by
  running against an environment that is itself freshly (re)provisioned per run. Runs MUST NOT
  silently accumulate leftover archives/categories/index entries across runs, and MUST NOT depend
  on a *specific* prior run having left behind particular state — this project has already lost
  real debugging time twice to exactly this class of problem (a stray leftover test archive
  skewing a manual bug investigation, and orphaned search-index entries left behind by an
  incomplete delete).
- What happens if a maintainer deliberately wants to inspect a failure by hand afterward (the
  same way this project's own manual QA sessions have used a running instance to dig into a bug)?
  → The suite MUST NOT be forced to always destroy its environment immediately on completion in a
  way that makes this impossible; leaving a failed run's environment inspectable on request is
  acceptable as long as the *next* run does not silently reuse that leftover state (see previous
  edge case).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The project MUST have a fast (sub-minute, no real backend required) automated test
  layer covering pure frontend logic that currently has zero coverage: reader settings
  persistence, reader page-navigation/spread computation, cross-archive navigation resolution, and
  metadata formatting/decoding helpers (file-info text construction, tag-namespace grouping).
- **FR-002**: The project MUST have an automated, real-browser test layer that exercises complete
  user journeys against a real running instance of the application (real backend, real Redis),
  not a mocked approximation — matching the fidelity this project's manual QA has relied on to
  actually find its real bugs so far.
- **FR-003**: The real-browser test layer MUST cover, at minimum, the specific user journeys this
  project has already found defects in through manual testing: signing in, creating and editing a
  category (including a field whose save previously failed silently), uploading a large archive
  file, paginating through a reader session (including the file-info-and-icon-toolbar layout and
  fit-mode switching), and deleting an archive followed by using random-archive navigation.
- **FR-004**: Both test layers MUST run automatically on every proposed code change to the
  frontend, with results visible on the change itself, without requiring a maintainer to manually
  invoke them.
- **FR-005**: A maintainer MUST be able to run either test layer locally, scoped to a subset (e.g.
  a single page or flow) rather than only the entire suite, to get fast feedback while working on
  a specific area.
- **FR-006**: End-to-end test scenarios MUST provision whatever data/state they depend on
  themselves (or via a dedicated fixture step) rather than assuming a particular pre-existing
  library/account state, so results are reproducible regardless of what data happens to already
  exist in the environment the suite runs against.
- **FR-007**: When a test fails, the failure output MUST identify which specific scenario/behavior
  broke clearly enough that a maintainer doesn't need to re-run it manually in a browser just to
  understand what went wrong.
- **FR-008**: This feature MUST NOT modify or depend on completion of Phase 2 (on-page manga
  translation) work, and MUST NOT require re-opening or modifying Phase 1's already-shipped
  functional code merely to make it "more testable" unless a specific piece of code is genuinely
  untestable as currently structured.
- **FR-009**: The project MUST maintain a fixture archive for each format this project's library
  supports (zip, cbz, epub, rar, cbr, 7z, cb7, lzh, lha, tar, gz, bz2, xz), usable by upload/
  ingestion/reading test scenarios (Story 4) without a maintainer having to hand-craft one first.
- **FR-010**: The project MUST maintain (or generate on demand) at least one fixture archive
  covering each of these specifically higher-risk shapes: a multi-volume/split archive, a
  password-protected/encrypted archive, and — for at least one archive format known to have had a
  filename-encoding defect — a fixture whose internal filenames use a non-ASCII (non-UTF-8-flagged)
  encoding, so this class of defect has permanent regression coverage rather than being
  re-discovered by chance. This feature only characterizes and locks in whatever this project's
  *current* multi-volume/encrypted-archive behavior actually is; if/when product support for either
  is added or changed (tracked as its own separate feature), the corresponding fixture-backed test
  is updated to match the new intended behavior as part of that work, not invented speculatively
  here.
- **FR-011**: Every test run MUST leave the environment (uploaded archives, categories, and any
  other state it created) in a clean, known state before the next run begins — either by tearing
  its own created state down, or by running against an environment provisioned fresh for that run
  — so that one run's leftover data cannot influence or be mistaken for another run's results.
- **FR-012**: An end-to-end test that fails MUST be automatically retried exactly once before being
  counted as a real failure; only a failure that reproduces on both the original attempt and the
  retry MUST be reported as a genuine regression.
- **FR-013**: The end-to-end test layer MUST cover, at minimum, the Chromium and Firefox browser
  engines.
- **FR-014**: When end-to-end tests run concurrently, each concurrent worker MUST use its own
  isolated backend/Redis instance or logical database, so parallel test execution cannot corrupt
  or contend over shared mutable state between workers — this is in addition to, not a replacement
  for, FR-011's per-run cleanliness requirement.
- **FR-015**: When an end-to-end test fails (including on its final retry per FR-012), the system
  MUST automatically capture a screenshot and a replayable trace of the failure (full DOM/network
  state around the point of failure), so a maintainer can diagnose it without needing to reproduce
  the failure by hand in a live browser first.

### Key Entities

- **Test Scenario**: A single named, independently-runnable check — either a fast unit-level
  assertion against isolated logic, or an end-to-end journey driving a real browser against a real
  running instance. Tagged/grouped so a maintainer can run a meaningful subset without invoking the
  entire suite.
- **Regression Fixture**: The known-bad case a scenario exists to guard against — traceable back to
  a specific bug this project already found and fixed (e.g. "category pinned-field 422",
  "reader dead whitespace below image"), so the suite's coverage can be reasoned about in terms of
  "which real defects would this have caught" rather than abstract line/branch coverage numbers.
- **Fixture Archive**: A prepared, reusable archive file representing one format/variant this
  project's library must handle (a specific container format, or a higher-risk shape like
  multi-volume or encrypted) — maintained as a first-class test asset in its own right (FR-009/
  FR-010), not a one-off file a single test scenario happens to create for itself.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every UI bug this project has already found and fixed through manual testing has at
  least one corresponding automated test that fails against a reverted version of its fix.
- **SC-002**: A maintainer gets a pass/fail result for pure-logic changes in under one minute,
  without needing a running backend.
- **SC-003**: A maintainer gets a pass/fail result for a full end-to-end user-journey change in
  under ten minutes.
- **SC-004**: 100% of proposed code changes to the frontend have both test layers run against them
  automatically, with zero changes merged based on manual-only verification going forward.
- **SC-005**: Time a maintainer spends manually re-clicking through previously-verified flows after
  an unrelated change drops to near zero for any flow with test coverage.
- **SC-006**: Every archive format/variant this project's library supports (including the
  higher-risk multi-volume and encrypted shapes) has at least one fixture-backed upload/reading
  test, so a future format-specific regression is caught by the suite rather than by a user's real
  library.
- **SC-007**: Two consecutive test runs, back to back with no manual cleanup step in between,
  produce the same result for unchanged code — demonstrating no cross-run state leakage.

## Assumptions

- This feature adds testing infrastructure and test scenarios on top of Phase 1's already-shipped
  frontend and backend; it does not require changing Phase 1's product behavior, and Phase 1 is
  not blocked on it (constitution Principle VI — additive, non-blocking).
- End-to-end scenarios run against a real backend instance (with a real Redis) started specifically
  for the test run, not the developer's own manually-managed local instance — this keeps runs
  reproducible and avoids the "which data happens to already be in my local Redis" class of false
  positive/negative this project's manual QA sessions have hit before.
- The fast unit-level layer and the real-browser end-to-end layer are complementary, not
  redundant: logic that can be verified in isolation belongs in the fast layer; behavior that only
  manifests through real rendering, network sequencing, or cross-page navigation belongs in the
  end-to-end layer. Coverage is not expected to be exhaustive on day one — it starts from the
  specific known-bad scenarios this project has already paid to discover once, and grows from
  there as new bugs are found.
- "Automatically on every proposed change" reuses this project's existing continuous-integration
  mechanism (the same one that already runs the backend's checks) rather than introducing a
  separate, differently-triggered system.
- New frontend code is not gated on meeting a hard numeric coverage-percentage threshold to be
  merged; coverage requirements remain scoped to key user journeys and already-fixed bugs (per the
  previous assumption) rather than an abstract line/branch coverage target, avoiding the incentive
  to write low-value tests purely to hit a number.
