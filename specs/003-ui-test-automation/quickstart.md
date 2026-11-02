# Quickstart: Validating Automated UI Test Coverage

Prerequisites: `mise install` for the pinned Rust/Node/Deno toolchain (Technology Stack
Constraints). A local Redis instance for the end-to-end layer (Playwright's `webServer`/
`globalSetup` starts and tears this down per run — see research.md §3-4 — a maintainer does not
need to hand-start one, but Redis itself, e.g. via `docker run redis:7-alpine`, must be reachable).
`p7zip-full` installed locally only if regenerating fixtures (research.md §5); running the test
suites themselves does not require it, since fixtures are checked into
`test-fixtures/archives/` (research.md §8).

## 1. Run the fast unit-level layer (spec User Story 1, Acceptance Scenario 3)

```
mise run test-frontend-unit          # or, scoped to one module: see §5 below
```

**Expected**: completes in well under a minute (SC-002), with no Redis/backend running at all. A
`useReaderSettings`/`crossArchiveNav`/`fileInfoText` failure here means the fast layer itself is
covering real logic — spot-check by deliberately breaking one function's return value and
confirming the corresponding test fails with a clear message before reverting.

## 2. Run the end-to-end layer against a real backend (spec User Story 1, Acceptance Scenario 4)

```
mise run test-frontend-e2e
```

**Expected**: Playwright's `webServer` config starts Redis, the compiled `lanrurugi-server`
binary, and a frontend preview server (research.md §3); tests run in both the `chromium` and
`firefox` projects (spec FR-013); the run completes in under ten minutes (SC-003). Each worker
uses an isolated Redis logical database (research.md §4) — confirm isolation by running with
`--workers=4` and checking no test intermittently fails due to another worker's data (FR-014).

## 3. Confirm a known regression is actually caught (spec User Story 1's Independent Test, SC-001)

Pick one entry from `data-model.md`'s Regression Fixture list — e.g. the category `pinned`-field
save 422 — and follow its `revert_to_reproduce` instruction (e.g. change `Categories.tsx`'s
`saveDetails` back to sending `'1'`/`'0'` instead of `'true'`/`'false'`).

```
pnpm exec playwright test categories.spec.ts
```

**Expected**: the corresponding test fails, and its name/output identifies the pinned-field save
path specifically (FR-007) — a maintainer should not need to open a browser to know what broke.
Revert the deliberate regression afterward and confirm the test passes again.

## 4. Confirm a deliberately-failing test retries once and produces artifacts (FR-012, FR-015)

Temporarily break any one E2E assertion (e.g. change an expected selector to one that doesn't
exist) and run just that test in CI mode:

```
CI=true pnpm exec playwright test reader.spec.ts --project=chromium
```

**Expected**: the test result shows two attempts (initial + one retry, both failing — Playwright
reports this as "Failed," not "Flaky," since a real assertion is broken on both attempts); a
screenshot and replayable trace are saved for the failure (`playwright-report/` or wherever
`playwright.config.ts` directs output); opening the trace with `pnpm exec playwright show-trace
<path>` shows the DOM/network state at the point of failure without needing to reproduce it live.
Revert the deliberate break afterward.

## 5. Run a scoped subset locally while working on one area (spec FR-005)

```
pnpm exec vitest run crossArchiveNav          # unit layer, one module
pnpm exec playwright test upload.spec.ts       # e2e layer, one flow area
```

**Expected**: either command returns a result for just that area without running the entire
respective suite, matching spec User Story 2's day-to-day workflow.

## 6. Confirm archive-format fixture coverage (spec User Story 4)

```
pnpm exec playwright test archive-formats.spec.ts
```

**Expected**: one passing (or, for the two higher-risk shapes, current-behavior-matching per
`data-model.md`'s `expected_behavior` field) scenario per fixture in `test-fixtures/archives/` —
zip, cbz, epub, rar, cbr, 7z, cb7, lzh, lha, tar, gz, bz2, xz, plus the multi-volume, encrypted,
and non-ASCII-filename fixtures (SC-006). A missing fixture for any `lanrurugi-scanner`-supported
format is itself a gap this scenario should surface, not silently skip.

## 7. Confirm no cross-run state leakage (FR-011, SC-007)

```
mise run test-frontend-e2e && mise run test-frontend-e2e
```

**Expected**: both runs produce the same pass/fail result for unchanged code — a second run must
not fail (or pass) differently because of state the first run left behind, since a bare `mise run
test-frontend-e2e` always tears down/starts clean (FR-011 default).

## 7a. Confirm a failed run's environment can be kept for manual inspection (spec Edge Case)

```
KEEP=1 mise run test-frontend-e2e
```

**Expected**: teardown is skipped for this run only — the uploaded archives/categories/Redis state
the run created remain inspectable afterward (e.g. by opening the app in a browser against the
same backend/Redis instance). Immediately follow with a bare `mise run test-frontend-e2e` (no
`KEEP`) and confirm it still starts from a clean state — `KEEP` must never persist across runs.

## 8. Confirm CI runs both layers automatically (spec User Story 3)

Open a pull request touching any file under `apps/frontend/src/`.

**Expected**: `.github/workflows/ci.yml`'s `frontend` job (or an added job, per how tasks.md
sequences this) reports both the Vitest and Playwright results on the PR itself, with no manual
step required to trigger either — matching the existing `rust`/`frontend` jobs' own
already-automatic behavior in this same workflow file.
