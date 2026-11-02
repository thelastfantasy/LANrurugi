---

description: "Task list for the Background Job Console feature"
---

# Tasks: Background Job Console

**Input**: Design documents from `/specs/002-job-console/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/jobs-api.md, quickstart.md

**Tests**: Not explicitly requested in spec.md — no dedicated test-writing tasks below; each
story's implementation task folds in the natural unit-test coverage for its own new code (per
this repo's existing convention in `specs/001-lanrurugi-full-rewrite/tasks.md`), and
`quickstart.md` gives the independent, manual verification scenario per story.

**Organization**: Tasks are grouped by user story (spec.md's US1/US2/US3) to enable independent
implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)

## Path Conventions

Web application (Rust backend + React frontend), adding to the existing Phase 1 workspace/app —
no new crate, no new frontend package (per plan.md's Structure Decision):
- Backend: `crates/lanrurugi-core/src/jobs.rs`, `crates/lanrurugi-api/src/jobs.rs`
- Frontend: `apps/frontend/src/api/types.ts`, `apps/frontend/src/api/hooks.ts`,
  `apps/frontend/src/pages/Jobs.tsx`, `apps/frontend/src/App.tsx`,
  `apps/frontend/src/pages/Settings/Settings.tsx`

---

## Phase 1: Setup

**Purpose**: Project initialization

No new setup required — this feature adds to the existing Phase 1 Cargo workspace and
`apps/frontend` app (already initialized, tooling already configured). Proceed directly to
Foundational.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Shared `JobRegistry` change every user story's backend work depends on

**⚠️ CRITICAL**: No user story work can begin until this phase is complete

- [x] T001 Add a creation-order index (`Vec<String>` of job IDs) to `JobRegistry` in
      `crates/lanrurugi-core/src/jobs.rs`, maintained by the existing `create()` method (append on
      insert), per data-model.md's "JobRegistry (extended)" section
- [x] T002 Add `MAX_TRACKED_JOBS: usize = 500` and eviction logic to `create()` in
      `crates/lanrurugi-core/src/jobs.rs`: once the registry holds this many entries, remove the
      oldest job(s) currently in a terminal state (`Finished`/`Failed`) — using the order index
      from T001 — before inserting the new one; if no terminal job exists to evict, insert anyway
      without dropping an active/queued job (research.md §2). Include unit tests covering: eviction
      of the oldest terminal job when the cap is reached, and non-eviction of active/queued jobs
      even when all slots are non-terminal (depends on T001)

**Checkpoint**: `JobRegistry` now tracks insertion order and enforces the retention bound — user
story implementation can begin

---

## Phase 3: User Story 1 - See what background work is running (Priority: P1) 🎯 MVP

**Goal**: A single page listing every tracked job with live state/progress, reachable from
Settings, with a proper empty state.

**Independent Test**: `quickstart.md` §1-2 — trigger any existing background action, open the job
console, confirm it appears and its state/progress update without a page reload; with a fresh
server and nothing triggered yet, confirm an explicit empty state instead of an error/blank page.

### Implementation for User Story 1

- [x] T003 [P] [US1] Add `pub async fn list_all(&self) -> Vec<JobStatus>` to `JobRegistry` in
      `crates/lanrurugi-core/src/jobs.rs`, returning jobs most-recently-created first using the
      order index from T001 (depends on T001)
- [x] T004 [US1] Add `GET /jobs` route (handler returning `{ "jobs": JobStatus[] }` per
      `contracts/jobs-api.md`) in `crates/lanrurugi-api/src/jobs.rs`, registered in that module's
      `router()` alongside the existing `/minion/*` routes without modifying them; include a test
      confirming an unauthenticated request gets the same 401/403 the rest of `/api` returns
      (FR-008 — this is the whole `/jobs` route group's shared auth-middleware placement, so
      T013/T014 don't repeat this test, they just inherit the same router()) (depends on T003)
- [x] T005 [P] [US1] Add a `JobRecord` interface (`id`/`name`/`state`/`progress`/`result`/`error`,
      distinct from the existing legacy-mimicking `JobStatus` interface) to
      `apps/frontend/src/api/types.ts` per `contracts/jobs-api.md`
- [x] T006 [US1] Add `useJobs()` query hook in `apps/frontend/src/api/hooks.ts` — `useQuery` over
      `GET /api/jobs`, `refetchInterval` matching the existing `useShinobuStatus`/`useLogLines`
      polling convention (research.md §3) (depends on T005)
- [x] T007 [US1] Create `apps/frontend/src/pages/Jobs.tsx`: job list page using `useJobs()`,
      rendering each job's name/state/progress with legacy `.itg` table styling (matching
      `Logs.tsx`/`Stats.tsx` conventions already established), a real empty state when the list is
      empty (FR-007), and `useDocumentTitle()`/`useApplyTheme()` calls matching every other ported
      page this session (depends on T006)
- [x] T008 [US1] Add `<Route path="/jobs" element={<Jobs />} />` inside the existing `<Layout>`
      route group in `apps/frontend/src/App.tsx` (depends on T007)
- [x] T009 [US1] Wire the existing "Open Minion Console" button (Background Workers section) in
      `apps/frontend/src/pages/Settings/Settings.tsx` to `navigate('/jobs')`, replacing its current
      stub `onClick` that just sets a "not available" status message (depends on T008)

**Checkpoint**: User Story 1 fully functional and independently testable — job list visible, live
state/progress, empty state, reachable from Settings

---

## Phase 4: User Story 2 - Inspect what a specific job did or why it failed (Priority: P2)

**Goal**: From the job list, see a finished job's result summary or a failed job's specific error
message.

**Independent Test**: `quickstart.md` §3-4 — trigger a job known to fail and one that succeeds,
open each job's detail view, confirm the error message / result payload shown is specific enough
to act on, not a generic message.

### Implementation for User Story 2

- [x] T010 [US2] Add an expandable detail view per job row in `apps/frontend/src/pages/Jobs.tsx`
      (e.g. click-to-expand, reusing the `CollapsibleSection` component from
      `apps/frontend/src/components/CollapsibleSection.tsx`) rendering `result` (pretty-printed,
      finished jobs) or `error` (failed jobs) from the `JobRecord` already fetched by US1's
      `useJobs()` — no new backend endpoint needed, this data is already present in `GET /api/jobs`'s
      response (depends on T007)

**Checkpoint**: User Stories 1 AND 2 both work independently — list view plus per-job detail

---

## Phase 5: User Story 3 - Clear old job entries (Priority: P3)

**Goal**: Clear finished/failed jobs individually or in bulk from the console, without disturbing
still-active jobs.

**Independent Test**: `quickstart.md` §5 — with several finished/failed jobs and one still-active
job listed, clear the finished ones (individually and via bulk-clear) and confirm they disappear
while the active job remains untouched.

### Implementation for User Story 3

- [x] T011 [P] [US3] Add `pub async fn clear(&self, id: &str) -> bool` to `JobRegistry` in
      `crates/lanrurugi-core/src/jobs.rs` — removes one job by ID, returns `false` (no-op) if the
      job doesn't exist or isn't in a terminal state (`Finished`/`Failed`); include unit tests for
      both the success case and the queued/active-job rejection case (depends on T001)
- [x] T012 [P] [US3] Add `pub async fn clear_finished(&self) -> usize` to `JobRegistry` in
      `crates/lanrurugi-core/src/jobs.rs` — removes every job in a terminal state, returns the
      count removed, leaves active/queued jobs untouched; include a unit test covering the mixed
      active+finished case (depends on T001)
- [x] T013 [US3] Add `DELETE /jobs/{id}` route in `crates/lanrurugi-api/src/jobs.rs` per
      `contracts/jobs-api.md` — `200` with `{ "operation": "clear_job", "success": 1 }` on success,
      `404` for an unknown ID, `409` if the job is still queued/active; include tests for all three
      response cases (200/404/409) (depends on T011)
- [x] T014 [US3] Add `DELETE /jobs?state=finished` route in `crates/lanrurugi-api/src/jobs.rs` per
      `contracts/jobs-api.md` — `200` with `{ "operation": "clear_finished_jobs", "success": 1,
      "cleared": <count> }`; `400` if `state` is missing or not `finished`; include tests for both
      response cases (200/400) (depends on T012)
- [x] T015 [P] [US3] Add `useClearJob(id)` and `useClearFinishedJobs()` mutation hooks in
      `apps/frontend/src/api/hooks.ts`, invalidating the `['jobs']` query key on success (depends
      on T013, T014)
- [x] T016 [US3] Add a per-job "Clear" action (disabled/hidden for queued/active jobs, per FR-004)
      directly to each list row in `apps/frontend/src/pages/Jobs.tsx` (the plain row from US1's
      T007 — deliberately not nested inside US2's expandable detail view from T010, so US3 stays
      implementable and testable without US2 having been built first) and a bulk clear button
      elsewhere on the page, both wired to the hooks from T015 (depends on T015, T007). The bulk
      button MUST be labeled to make clear it clears every finished/failed job, not just what's
      currently visible under an active US4 state/name filter (e.g. "Clear all finished", not
      just "Clear finished") — per FR-004's note, `useClearFinishedJobs()` (T015) is unscoped by
      design and always targets the full server-side set

**Checkpoint**: All three user stories independently functional — list/monitor, inspect, and clear

---

## Phase 6: User Story 4 - Find what you need in a long job list (Priority: P2)

**Goal**: Per-state counts, a state filter, a name search, and pagination over the job list —
mirroring legacy Minion Admin's own stat bar/filtering (minus Workers/Locks, which don't apply
here — see spec.md Assumptions), entirely client-side.

**Independent Test**: `quickstart.md` §7 — with a mix of jobs in different states/names listed,
confirm the per-state counts match reality, a state filter narrows the list correctly, a name
search narrows it correctly, and the list is paginated once it's long enough to need it.

### Implementation for User Story 4

- [x] T020 [P] [US4] Add a per-state count computation (queued/active/finished/failed) over the
      **unfiltered** `JobRecord[]` array from `useJobs()` in `apps/frontend/src/pages/Jobs.tsx`,
      rendered as a small stat bar above the list (FR-009) — pure derived value, no new hook or
      endpoint (research.md §5); counts MUST be computed before T021/T022's filters are applied,
      so they stay fixed totals that don't change as the admin filters/searches (depends on T007)
- [x] T021 [P] [US4] Add a state-filter control (e.g. clicking a stat from T020, or a dropdown) to
      `apps/frontend/src/pages/Jobs.tsx` that narrows the rendered list to one state via a local
      `.filter()` (FR-010) (depends on T007)
- [x] T022 [P] [US4] Add a name-search text input to `apps/frontend/src/pages/Jobs.tsx` that
      narrows the rendered list via a local case-insensitive substring `.filter()` on `name`
      (FR-011) (depends on T007)
- [x] T023 [US4] Add pagination (e.g. a simple page-size/page-index control matching legacy Minion
      Admin's own 10/20/50/100 convention) to `apps/frontend/src/pages/Jobs.tsx`'s rendered list via
      a local `.slice()`, applied after the state/name filters from T021/T022 (FR-012, SC-005)
      (depends on T021, T022)

**Checkpoint**: All four user stories independently functional — list/monitor, inspect, clear, and
find-in-a-long-list

---

## Phase 7: Polish & Cross-Cutting Concerns

- [x] T024 [P] Add i18n keys for every new user-facing string in `Jobs.tsx` to
      `apps/frontend/src/i18n/locales/{en,zh,zh_Hant}.json`, matching this session's established
      convention (no hardcoded English left in the page)
- [x] T025 Run `quickstart.md`'s full validation sequence (§1-7) end-to-end against a running
      `lanrurugi-server` instance
- [x] T026 [P] `cargo clippy` and `cargo test` for `crates/lanrurugi-core` and
      `crates/lanrurugi-api`; `eslint`/`tsc -b` for `apps/frontend`

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: None — nothing to do
- **Foundational (Phase 2)**: No dependencies beyond existing Phase 1 code — BLOCKS all user
  stories (T003, T011, T012 all depend on T001; T002 depends on T001)
- **User Stories (Phase 3-6)**: All depend on Foundational (Phase 2) completion
  - US1 (Phase 3) has no dependency on US2/US3/US4
  - US2 (Phase 4) depends on US1's `Jobs.tsx` existing (T007) — reuses its already-fetched data,
    adds no backend work of its own
  - US3 (Phase 5) depends on US1's `Jobs.tsx` existing (T007, for the plain list row its clear
    action attaches to) and on Foundational for its own `JobRegistry` methods (T011/T012 depend on
    T001) — deliberately does **not** depend on US2/T010, so US3 can be built and independently
    tested before US2 if desired (see T016's note)
  - US4 (Phase 6) depends only on US1's `Jobs.tsx`/`useJobs()` existing (T007) — no backend work,
    no dependency on US2/US3; independent of both and can be built in any order relative to them
- **Polish (Phase 7)**: Depends on all four user stories being complete

### Parallel Opportunities

- T003 and T005 (US1) can run in parallel — different files (`lanrurugi-core` vs frontend types)
- T011 and T012 (US3) can run in parallel — same file but independent methods, low conflict risk
- T015 (US3 hooks) can start once T013/T014 land, in parallel with no other US3 task
- T020, T021, T022 (US4) can run in parallel — independent derived-data concerns over the same
  array, though all touch the same `Jobs.tsx` file so treat this as low-conflict-risk parallelism
  (same caveat as T011/T012) rather than fully independent-file parallelism
- T024 and T026 (Polish) can run in parallel

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 2: Foundational (T001-T002)
2. Complete Phase 3: User Story 1 (T003-T009)
3. **STOP and VALIDATE**: `quickstart.md` §1-2 — job list visible, live-updating, empty state
   correct
4. Deploy/demo if ready — this alone already replaces the "not available in this port" stub with
   real value (list/monitor, matching the highest-priority need identified when this feature was
   first proposed)

### Incremental Delivery

1. Foundational → US1 (MVP: list/monitor) → US2 (inspect result/error) → US3 (clear) → US4
   (per-state counts/filter/search/pagination) → Polish
2. Each story is independently testable per its Independent Test above before moving to the next;
   US4 has no dependency on US2/US3 (research.md §5 — purely a derived view over US1's own data),
   so it may equally be pulled forward and built right after US1 if list-findability matters more
   than inspect/clear for a given rollout
