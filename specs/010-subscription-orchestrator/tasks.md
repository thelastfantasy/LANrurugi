---
description: "Task list for Subscription Orchestrator"
---

# Tasks: Subscription Orchestrator

**Input**: Design documents from `specs/010-subscription-orchestrator/`

**Prerequisites**: `plan.md`, `spec.md`, `research.md`, `data-model.md`, `contracts/`, `quickstart.md`

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an unfinished task)
- **[Story]**: Which user story the task serves (US1–US4)

## Path Conventions

Paths below are repo-relative. Rust lives under `crates/`, the frontend under `apps/frontend/src/`,
extensions under `plugins/`.

## Testing posture

Neither the spec nor the constitution asks for TDD, so this is **not** a blanket test-first list.
Test tasks appear only where a defect would be silent or expensive:

- `matcher.rs` — pure rule evaluation, cheap to test exhaustively, and wrong answers here are
  invisible until a user notices a missing download weeks later.
- The inconclusive-cycle guard — its failure mode (works permanently marked handled) produces no
  error at all, so a test is the only thing that can catch a regression.

---

## Phase 1: Setup

**Purpose**: Create the module skeletons so later tasks have somewhere to land.

- [ ] T001 Create the orchestrator module skeleton (`mod`/`scheduler`/`matcher`/`runner`/`reservations`) in `crates/lanrurugi-api/src/subscriptions/` and register it in `crates/lanrurugi-api/src/lib.rs`
- [ ] T002 [P] Create the storage module skeleton in `crates/lanrurugi-storage/src/subscriptions.rs` and register it in `crates/lanrurugi-storage/src/lib.rs`
- [ ] T003 [P] Create the `plugins/discovery/` directory with a short README stating what a discovery extension must export (point at `contracts/discovery-capability.md`)

---

## Phase 2: Foundational (blocks every user story)

**Purpose**: The discovery capability, persistence, and scheduler. Nothing user-visible works without
these, and all four stories sit on top of them.

**⚠️ Every user story phase depends on this phase being complete.**

### Discovery capability (the one new extension surface)

- [ ] T004 Add the discovery capability's types to `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`: criteria input, user-supplied-listing input, candidate output, and the explicit complete/degraded flag per `contracts/discovery-capability.md`
- [ ] T005 Mirror those types in `crates/lanrurugi-plugin/src/protocol.rs`, following the field-for-field convention the existing protocol types use
- [ ] T006 Add the discovery types to `plugins/legacy-globals.d.ts` so an extension can annotate its own function without importing the SDK (the project's established zero-import constraint)
- [ ] T007 Dispatch the discovery call in `crates/lanrurugi-plugin/dispatcher/dispatcher.ts`, alongside the existing capability cases
- [ ] T008 Add a pool method to invoke discovery in `crates/lanrurugi-plugin/src/pool.rs`, routing the declared sign-in state into the call the same way existing capability calls do
- [ ] T009 Add interval guidance (suggested + minimum) to the extension options surface in `plugin-sdk.ts` and `protocol.rs`, reusing the existing declared-default/user-override settings shape

### Persistence

- [ ] T010 [P] Implement the `Subscription` record (fields, state enum incl. paused-with-reason) in `crates/lanrurugi-storage/src/subscriptions.rs` per `data-model.md`
- [ ] T011 [P] Implement `Check Cycle` persistence with its three outcomes — completed / failed / **inconclusive** — plus bounded history, in `crates/lanrurugi-storage/src/subscriptions.rs`
- [ ] T012 [P] Implement `Reservation Entry` persistence, keeping discarded entries rather than deleting them (FR-016), in `crates/lanrurugi-storage/src/subscriptions.rs`
- [ ] T013 [P] Implement per-subscription `Seen Work` sets in `crates/lanrurugi-storage/src/subscriptions.rs`
- [ ] T014 Add the new key namespaces to `crates/lanrurugi-storage/src/keys.rs` following the existing `LANRURUGI_*` convention

### Scheduler

- [ ] T015 Implement due-time computation in `crates/lanrurugi-api/src/subscriptions/scheduler.rs` — "due or not", never "how many times due", which is what makes the single catch-up (FR-008a) fall out for free
- [ ] T016 Implement the single scheduler loop and its in-flight set so one subscription can never overlap itself (FR-012), in `crates/lanrurugi-api/src/subscriptions/scheduler.rs`
- [ ] T017 Register the scheduler as a background task in `crates/lanrurugi-server/src/main.rs`, following the existing periodic-task pattern and respecting the project's background-work resource posture

**Checkpoint**: discovery can be called, subscriptions persist, and the scheduler decides what is due.

---

## Phase 3: User Story 1 — Track a creator (P1) 🎯 MVP

**Goal**: A subscription finds new works by a creator and brings them in.

**Independent test**: Create one subscription, press Check now, approve the matches, see them land in
the library — with no URL typed by hand.

### Tests

- [ ] T018 [P] [US1] Unit-test due-time and catch-up logic in `crates/lanrurugi-api/src/subscriptions/scheduler.rs` — a subscription whose schedule lapsed over several intervals must come due exactly once

### Implementation

- [ ] T019 [US1] Implement one check cycle end to end in `crates/lanrurugi-api/src/subscriptions/runner.rs`: call discovery, record a cycle, queue survivors through the existing download path
- [ ] T020 [US1] Implement the pre-download duplicate check (normalised source identity) in `crates/lanrurugi-api/src/subscriptions/matcher.rs`
- [ ] T021 [US1] Record Seen Work entries from **conclusive cycles only** in `runner.rs` — the guard that keeps a degraded check from hiding works forever
- [ ] T022 [US1] Honour `auto_download`, defaulting to wait-for-confirmation (FR-007a), in `runner.rs`
- [ ] T023 [US1] Attribute subscription-started downloads distinguishably from user-started ones (FR-022) in `crates/lanrurugi-api/src/subscriptions/runner.rs`
- [ ] T024 [P] [US1] Implement subscription CRUD + enable/disable + check-now endpoints in `crates/lanrurugi-api/src/subscriptions/mod.rs` per `contracts/subscription-api.md`
- [ ] T025 [US1] Implement `GET /subscriptions/sources` (which sources can back a subscription, with their interval bounds) in `crates/lanrurugi-api/src/subscriptions/mod.rs`
- [ ] T026 [US1] Enforce the interval floor on create/edit — **refuse with the reason**, never silently clamp (FR-004b) — in `crates/lanrurugi-api/src/subscriptions/mod.rs`
- [ ] T027 [US1] Refuse sources lacking discovery, with the reason shown (FR-002b), in `crates/lanrurugi-api/src/subscriptions/mod.rs`
- [ ] T028 [P] [US1] Implement pending-approval endpoints (list / approve / dismiss) in `crates/lanrurugi-api/src/subscriptions/mod.rs`
- [ ] T029 [P] [US1] Build the subscription management section in `apps/frontend/src/pages/Settings/SubscriptionsSection.tsx`, following the existing settings-section pattern and using `common-ui/Form` components rather than raw inputs
- [ ] T030 [P] [US1] Add API types and hooks for subscriptions in `apps/frontend/src/api/types.ts` and `apps/frontend/src/api/hooks.ts`
- [ ] T031 [US1] Build the pending-approval surface in `apps/frontend/src/pages/Settings/SubscriptionsSection.tsx`
- [ ] T032 [P] [US1] Write the first real discovery extension in `plugins/discovery/ehentai.ts`, declaring its sign-in dependency and interval bounds
- [ ] T033 [P] [US1] Add i18n keys for the subscription UI across all 14 locale files in `apps/frontend/src/i18n/locales/`

**Checkpoint**: US1 is independently demonstrable — quickstart Scenarios 1–3 pass.

---

## Phase 4: User Story 2 — Narrow what gets downloaded (P2)

**Goal**: Rules keep unwanted works out, and a rejected candidate says which rule rejected it.

**Independent test**: Add an exclude-tag rule, run a check, confirm the work is not queued and the
reason names the rule.

### Tests

- [ ] T034 [P] [US2] Unit-test rule evaluation exhaustively in `crates/lanrurugi-api/src/subscriptions/matcher.rs` — required/excluded tags, rating floor, excluded categories, and their interactions

### Implementation

- [ ] T035 [US2] Implement filter evaluation as pure functions in `matcher.rs`, returning *which* rule rejected a candidate rather than a bare boolean (FR-011)
- [ ] T036 [US2] Record per-candidate verdicts and reasons into the cycle record in `runner.rs`
- [ ] T037 [US2] Defer newer revisions of held works to the existing revision comparison instead of discarding them as duplicates (FR-009b) in `matcher.rs`
- [ ] T038 [US2] Add the post-download content-duplicate backstop (FR-009a) alongside the existing duplicate handling in `crates/lanrurugi-api/src/download_manager/ingest.rs`
- [ ] T039 [P] [US2] Add filter editing to `SubscriptionsSection.tsx`
- [ ] T040 [P] [US2] Surface per-candidate rejection reasons in the subscription's recent-activity view in `SubscriptionsSection.tsx`

**Checkpoint**: quickstart Scenario 4 passes; US1 still works unchanged.

---

## Phase 5: User Story 3 — See and act on failures (P2)

**Goal**: Works that could not download are visible with their reason, and actionable.

**Independent test**: Force a download failure, confirm the reservation entry appears with the real
reason, retry it successfully after removing the obstacle.

### Implementation

- [ ] T041 [US3] Create reservation entries on download failure, carrying source URL, reason, and originating subscription, in `crates/lanrurugi-api/src/subscriptions/reservations.rs`
- [ ] T042 [US3] Implement retry / retry-all / discard endpoints in the same file per `contracts/subscription-api.md`
- [ ] T043 [US3] Make later cycles consult discarded entries so a discarded work is not re-reserved (FR-016) in `matcher.rs`
- [ ] T044 [US3] Bound reservation growth and tell the user when entries were dropped (FR-018) in `reservations.rs`
- [ ] T045 [P] [US3] Build the reservation group in `apps/frontend/src/pages/Upload/ReservationGroup.tsx` — collapsed by default behind an icon control (FR-017)
- [ ] T046 [US3] Mount the reservation group on the upload page beside the download queue in `apps/frontend/src/pages/Upload/`
- [ ] T047 [P] [US3] Link each reservation entry to the subscription that produced it (FR-026) in `ReservationGroup.tsx`
- [ ] T048 [P] [US3] Add reservation API types and hooks in `apps/frontend/src/api/types.ts` and `hooks.ts`
- [ ] T049 [P] [US3] Add i18n keys for the reservation UI across all 14 locale files in `apps/frontend/src/i18n/locales/`

**Checkpoint**: quickstart Scenario 5 passes; US1 and US2 unaffected.

---

## Phase 6: User Story 4 — Stay within a download budget (P3)

**Goal**: A credit failure pauses the subscription or reserves the item, as the user chose.

**Independent test**: Trigger a credit failure on a pause-configured subscription; confirm it stops
and shows the reason, and that resume restores normal scheduling.

### Implementation

- [ ] T050 [US4] Recognise the insufficient-credit failure signal and branch on the subscription's choice, in `runner.rs`
- [ ] T051 [US4] Implement pause-with-reason and resume in `crates/lanrurugi-api/src/subscriptions/mod.rs` (FR-019, FR-020)
- [ ] T052 [P] [US4] Show paused state and reason, with a resume control, in `SubscriptionsSection.tsx`
- [ ] T053 [P] [US4] Add i18n keys for budget/pause states across all 14 locale files in `apps/frontend/src/i18n/locales/`

**Checkpoint**: quickstart Scenario 9 passes; all four stories work together.

---

## Phase 7: Authoring assistant coverage

**Purpose**: Without this, every new source is still hand-written — which defeats having an
assistant at all (FR-027).

- [ ] T054 Consolidate the assistant's capability list into one definition (FR-028) — it is currently hardcoded in several places, e.g. `const PLUGIN_TYPES` in `crates/lanrurugi-api/src/plugin_wizard/lookup.rs`
- [ ] T055 Teach the generation flow to produce a discovery capability in `crates/lanrurugi-api/src/plugin_wizard/generate.rs`
- [ ] T056 Extend the draft trial run so a generated discovery capability is verifiable before saving (FR-029) in `crates/lanrurugi-api/src/plugin_wizard/trial_run.rs`

---

## Phase 8: Polish & cross-cutting

- [ ] T057 Test the inconclusive-cycle guard: a degraded check must not write Seen Work, and works it could not see must still be found once sign-in is restored (`crates/lanrurugi-api/src/subscriptions/runner.rs` tests)
- [ ] T058 Report degraded/inconclusive checks distinctly from "nothing found" in the UI (FR-002f/h) in `SubscriptionsSection.tsx`
- [ ] T059 Handle a source raising its minimum interval after subscriptions already exist — bring them into line and tell the user — in `crates/lanrurugi-api/src/subscriptions/scheduler.rs`
- [ ] T060 [P] Add structured, translatable failure reasons for every new failure path in `crates/lanrurugi-core/src/queue_error.rs`, matching the existing queue-error style
- [ ] T061 [P] Update `README.md`'s improvements section — subscriptions have no legacy equivalent
- [ ] T062 Run every scenario in `specs/010-subscription-orchestrator/quickstart.md` end to end, especially Scenario 6 step 4 (works found again after sign-in is restored)

---

## Dependencies & Execution Order

### Phase dependencies

- **Setup (Phase 1)**: no dependencies
- **Foundational (Phase 2)**: needs Setup — **blocks all user stories**
- **US1 (Phase 3)**: needs Foundational. MVP.
- **US2 (Phase 4)**: needs Foundational. Independently testable, but most meaningful after US1 since
  filters refine a loop US1 establishes.
- **US3 (Phase 5)**: needs Foundational. Independent of US2.
- **US4 (Phase 6)**: needs Foundational; reuses US3's reservation path for its "continue and reserve"
  option.
- **Phase 7 (assistant)**: needs the discovery capability from Foundational; independent of all UI work.
- **Phase 8 (polish)**: after the stories it touches.

### Within a story

Storage → orchestrator logic → endpoints → UI. Tests for `matcher.rs` can be written before or
alongside its implementation; they do not gate other tasks.

### Parallel opportunities

- T002, T003 in Setup
- T010–T013 (separate entities, one file but non-overlapping sections — serialise if that proves awkward)
- T024/T028 (endpoints) with T029/T030/T032/T033 (UI and extension) once their shared types exist
- Most i18n tasks (T033, T049, T053) with their respective UI tasks
- Phase 7 alongside Phases 4–6 entirely

---

## Implementation Strategy

### MVP = Phase 1 + Phase 2 + Phase 3

That delivers the feature's actual promise: a subscription that brings in new works unattended. It is
also where the discovery capability — the only genuinely new extension surface — gets proven. Stop
here and you have something worth using.

### Incremental order after MVP

1. **US3 before US2** is worth considering. Without the reservation list, a failing subscription is
   indistinguishable from one with nothing to find, which makes everything else harder to diagnose
   while developing.
2. **US2** then makes the loop livable rather than a firehose.
3. **US4** last — it only matters once subscriptions run unattended at volume.
4. **Phase 7** whenever convenient; it blocks no user-facing behaviour but determines whether the
   second source costs an afternoon or a day.

### The two tasks most likely to be got wrong

- **T021** (Seen Work only from conclusive cycles). Getting this backwards produces no error, no
  failed test, and no user-visible symptom until someone notices a work that never appeared. T057
  exists specifically to pin it.
- **T026** (refuse, don't clamp, a too-short interval). Silent clamping leaves the user misreading
  every later result.
