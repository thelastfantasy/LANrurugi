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

- [X] T001 Create the orchestrator module skeleton (`mod`/`scheduler`/`matcher`/`runner`/`reservations`) in `crates/lanrurugi-api/src/subscriptions/` and register it in `crates/lanrurugi-api/src/lib.rs`
- [X] T002 [P] Create the storage module skeleton in `crates/lanrurugi-storage/src/subscriptions.rs` and register it in `crates/lanrurugi-storage/src/lib.rs`
- [X] T003 [P] Create the `plugins/discovery/` directory with a short README stating what a discovery extension must export (point at `contracts/discovery-capability.md`)

---

## Phase 2: Foundational (blocks every user story)

**Purpose**: The discovery capability, persistence, and scheduler. Nothing user-visible works without
these, and all four stories sit on top of them.

**⚠️ Every user story phase depends on this phase being complete.**

### Discovery capability (the one new extension surface)

- [X] T004 Add the discovery capability's types to `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`: criteria input, user-supplied-listing input, candidate output, and the explicit complete/degraded flag per `contracts/discovery-capability.md`
- [X] T005 Mirror those types in `crates/lanrurugi-plugin/src/protocol.rs`, following the field-for-field convention the existing protocol types use
- [X] T006 Add the discovery types to `plugins/legacy-globals.d.ts` so an extension can annotate its own function without importing the SDK (the project's established zero-import constraint)
- [X] T007 Dispatch the discovery call in `crates/lanrurugi-plugin/dispatcher/dispatcher.ts`, alongside the existing capability cases
- [X] T008 Add a pool method to invoke discovery in `crates/lanrurugi-plugin/src/pool.rs`, routing the declared sign-in state into the call the same way existing capability calls do
- [X] T009 Add interval guidance (suggested + minimum) to the extension options surface in `plugin-sdk.ts` and `protocol.rs`, reusing the existing declared-default/user-override settings shape

### Persistence

- [X] T010 [P] Implement the `Subscription` record (fields, state enum incl. paused-with-reason) in `crates/lanrurugi-storage/src/subscriptions.rs` per `data-model.md`
- [X] T011 [P] Implement `Check Cycle` persistence with its three outcomes — completed / failed / **inconclusive** — plus bounded history, in `crates/lanrurugi-storage/src/subscriptions.rs`
- [X] T012 [P] Implement `Reservation Entry` persistence, keeping discarded entries rather than deleting them (FR-016), in `crates/lanrurugi-storage/src/subscriptions.rs`
- [X] T013 [P] Implement per-subscription `Seen Work` sets in `crates/lanrurugi-storage/src/subscriptions.rs`
- [X] T014 Add the new key namespaces to `crates/lanrurugi-storage/src/keys.rs` following the existing `LANRURUGI_*` convention

### Scheduler

- [X] T015 Implement due-time computation in `crates/lanrurugi-api/src/subscriptions/scheduler.rs` — "due or not", never "how many times due", which is what makes the single catch-up (FR-008a) fall out for free
- [X] T016 Implement the single scheduler loop and its in-flight set so one subscription can never overlap itself (FR-012), in `crates/lanrurugi-api/src/subscriptions/scheduler.rs`
- [X] T017 Register the scheduler as a background task in `crates/lanrurugi-server/src/main.rs`, following the existing periodic-task pattern and respecting the project's background-work resource posture

**Checkpoint**: discovery can be called, subscriptions persist, and the scheduler decides what is due.

---

## Phase 3: User Story 1 — Track a creator (P1) 🎯 MVP

**Goal**: A subscription finds new works by a creator and brings them in.

**Independent test**: Create one subscription, press Check now, approve the matches, see them land in
the library — with no URL typed by hand.

### Tests

- [X] T018 [P] [US1] Unit-test due-time and catch-up logic in `crates/lanrurugi-api/src/subscriptions/scheduler.rs` — a subscription whose schedule lapsed over several intervals must come due exactly once

### Implementation

- [X] T019 [US1] Implement one check cycle end to end in `crates/lanrurugi-api/src/subscriptions/runner.rs`: call discovery, record a cycle, queue survivors through the existing download path
- [X] T020 [US1] Implement the pre-download duplicate check (normalised source identity) in `crates/lanrurugi-api/src/subscriptions/matcher.rs`
- [X] T021 [US1] Record Seen Work entries from **conclusive cycles only** in `runner.rs` — the guard that keeps a degraded check from hiding works forever
- [X] T022 [US1] Honour `auto_download`, defaulting to wait-for-confirmation (FR-007a), in `runner.rs`
- [X] T023 [US1] Attribute subscription-started downloads distinguishably from user-started ones (FR-022) in `crates/lanrurugi-api/src/subscriptions/runner.rs`
- [X] T024 [P] [US1] Implement subscription CRUD + enable/disable + check-now endpoints in `crates/lanrurugi-api/src/subscriptions/api.rs` per `contracts/subscription-api.md` — the check-now endpoint was missing despite this being ticked; added as `POST /subscriptions/{id}/check` (spawned, since a check pages the source and is longer than a request should hold open)
- [X] T025 [US1] Implement `GET /subscriptions/sources` (which sources can back a subscription, with their interval bounds) in `crates/lanrurugi-api/src/subscriptions/mod.rs`
- [X] T026 [US1] Enforce the interval floor on create/edit — **refuse with the reason**, never silently clamp (FR-004b) — in `crates/lanrurugi-api/src/subscriptions/mod.rs`
- [X] T027 [US1] Refuse sources lacking discovery, with the reason shown (FR-002b), in `crates/lanrurugi-api/src/subscriptions/mod.rs`
- [X] T028 [P] [US1] Implement pending-approval endpoints (list / approve / dismiss) in `crates/lanrurugi-api/src/subscriptions/api.rs`, backed by a `PendingApproval` record in `crates/lanrurugi-storage/src/subscriptions.rs` (its own record, not read back out of the age-bounded cycle history, so an unanswered question cannot expire on its own)
- [X] T029 [P] [US1] Build the subscription management section in `apps/frontend/src/pages/Settings/SubscriptionsSection.tsx`, following the existing settings-section pattern and using `common-ui/Form` components rather than raw inputs
- [X] T030 [P] [US1] Add API types and hooks for subscriptions in `apps/frontend/src/api/types.ts` and `apps/frontend/src/api/hooks.ts`
- [X] T031 [US1] Build the pending-approval surface in `apps/frontend/src/pages/Settings/SubscriptionsSection.tsx`, above the subscription list rather than inside each row — "what is waiting for me" is what the page is opened to answer
- [X] T032 [P] [US1] Write the first real discovery extension in `plugins/discovery/ehentai.ts`, declaring its sign-in dependency and interval bounds
- [X] T032a [US1] Detect an interposed bot challenge in `plugins/discovery/ehentai.ts` and report it as an error, never as a completed or degraded check (FR-002i) — verified live against e-hentai.org, which answers from behind Cloudflare
- [X] T032b [US1] Return `title`/`rating`/`tags` per candidate in `plugins/discovery/ehentai.ts`, read per gallery row rather than per field across the page (FR-002j) — without this every required-tag rule rejects every candidate
- [X] T033 [P] [US1] Add i18n keys for the subscription UI across all 14 locale files in `apps/frontend/src/i18n/locales/`
- [X] T033a [US1] Add subscription editing to `SubscriptionsSection.tsx` (FR-001/FR-012a/FR-023/FR-024) by reusing the create form for both modes — without it, changing a rule means delete-and-recreate, which loses `last_checked_at` and the Seen Work set and so re-downloads the entire back catalogue
- [X] T033b [US1] Expose per-subscription check history in `SubscriptionsSection.tsx` (FR-011), showing each cycle's outcome and each candidate's verdict with the rule that rejected it
- [X] T033c [US2] Type the filterable candidate fields on the SDK's own `DiscoveredCandidate` (`crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts` and `plugins/legacy-globals.d.ts`), so a misspelled field is a compile error rather than a silently dropped property — the same guarantee `DownloadResultShape` gives (FR-011a)
- [X] T033d [US2] Implement the generic field-rule engine in `crates/lanrurugi-api/src/subscriptions/matcher.rs`: `FieldRule`/`FieldOperator`/`RuleValue` in storage, operator-per-type evaluation, absence held rather than rejected (FR-011b/c/d)
- [X] T033e [US2] Apply date rules before any permanent verdict (FR-011c/e) — a work inside its waiting period returns `TooSoon` and is deliberately excluded from Seen Work, since marking it seen would make the wait permanent
- [X] T033f [US2] Populate `posted_at`/`category`/`uploader`/`pages` per gallery row in `plugins/discovery/ehentai.ts` — verified live against e-hentai.org, 25/25 rows for every field
- [X] T033g [US2] Render the field-rule form from the source's own fields in `apps/frontend/src/pages/Settings/FieldRuleEditor.tsx`, with the control and operators chosen by each field's type
- [X] T033h [US2] Report which candidate fields an extension writes via `PluginIntrospection::candidate_fields` (static analysis in `dispatcher.ts`, reusing `usesPropertyKey`) and expose them through `GET /subscriptions/sources`, so the form offers filters only over fields that will be populated (FR-011a)
- [X] T033i [US1] Surface `CandidateVerdict::TooSoon` distinctly in the history view — it previously fell through to "already seen", which means the opposite (one will be reconsidered, the other never will)
- [X] T033j [US1] Add the `on_source_changed`/`on_source_removed` controls to the subscription form; the policies existed server-side with no way for a user to set them
- [X] T033k [US2] Page the e-hentai listing by its `next=<gid>` cursor, bounded by a host-set `max_pages` (issue #111) — one page never shows a work that entered the result set by being edited, since those sort by original publication date
- [X] T033l [US2] Replace the flat rule list with a condition tree (`Condition::All`/`Any`/`Not`) in `crates/lanrurugi-storage/src/subscriptions.rs` and evaluate it in `matcher.rs`, with pending propagating over the whole tree (FR-011f/g/h)
- [X] T033m [US2] Fold the four superseded fixed filters into the tree via `Filters::effective_condition` so there is one evaluation path, not two — `runner.rs`'s snapshot "still matches" check now uses the same one
- [X] T033n [US2] Build the tree editor in `FieldRuleEditor.tsx`: per-group all/any, invert any node or group into an exclusion, nesting capped at 3 levels, plus `.field-rule-group` added to all 5 theme files reusing each theme's own accent
- [X] T033o [US1] Add `POST /subscriptions/{id}/preview` and `runner::preview_check`, sharing `decide` with the real check so a preview can never disagree with what it previews (FR-011i/j); accepts an unsaved draft so rules can be tried before being committed, and works on a disabled subscription
- [X] T033p [US1] Build the preview panel in `apps/frontend/src/pages/Settings/SubscriptionPreview.tsx` — three bands (would take / still waiting / ruled away) each with its reason, plus `.preview-row-*` in all 5 theme files; a waiting candidate is banded apart from an excluded one because it will be reconsidered
- [X] T033q [US1] Write 10 real rendering tests for the condition-tree editor in `apps/frontend/tests/unit/fieldRuleEditor.test.tsx` — these found four genuine defects: a dead end after inverting the root, icon buttons with no accessible name, the root's remove being indistinguishable from a child's, and two identically-labelled add buttons acting on different groups
- [X] T033r [US2] Carry a work's titles as a map keyed by language (`{ origin, ja, … }`) through `DiscoveredCandidate`, both SDKs and the storage records, accepting a bare string as `{ origin }` so extensions written against the older shape keep working — naming languages in the protocol (`title_jpn` and friends) would mean changing it again for the next source
- [X] T033s [US2] Fill `title.ja` in `plugins/discovery/ehentai.ts` from the JSON API's `title_jpn`, batched one request per listing page — the listing itself carries only one title, so the original is unreachable from the page alone. Verified live: 25 candidates, 20 with an original title
- [X] T033t [US2] Match a `title` rule against *any* of a work's titles in `matcher.rs` — a work is the same work whichever language names it, so a rule written in one must not fail because another was compared
- [X] T033u Turn the interface-language setting into an ordered preference list (`LanguageOrderEditor.tsx`, stored comma-separated in the existing `language` field so older single values still read correctly), and choose both the interface language and a work's displayed title from it (FR-011k/l) — this also removed the host's hard-coded preference for Japanese
- [X] T033v [US1] Add `GET /subscriptions/history`, interleaving every subscription's recent candidates by time (FR-011m) — per-subscription history answers "what did this rule do", which is the other question
- [X] T033w [US1] Build the cross-subscription history modal in `SubscriptionHistoryModal.tsx`, reachable from the Subscriptions section heading, filterable by taken/turned-away and reusing the preview's own verdict bands so a verdict reads the same wherever it appears
- [X] T033x [US2] Offer tag namespaces as fields of their own (`language`, `artist`, …) plus `is_empty`/`is_not_empty` in `matcher.rs` — "in Chinese or Japanese, or carrying no language tag" is otherwise unsayable, since excluding specific tags cannot say "none of this kind"
- [X] T033y [US1] Join the download queue's own state into `GET /subscriptions/history` and offer a retry for restartable failures (FR-011n) — a cycle records only the decision, so a failed download read as "queued" forever
- [X] T033z [US1] Show which login/download plugin a source resolves to, warn when the declared login is missing, and accept per-subscription cookies/headers in that case (FR-011o/p) — a missing sign-in shrinks a listing rather than failing it
- [X] T034a [US1] Hold the first check for a settling period after a subscription's settings are written, on edit as well as creation, and say so in the list (FR-012f)
- [X] T034b [US1] Drop the separate creator/uploader input: a listing URL expresses it exactly (verified live — both return the same 25 works) and also expresses what it cannot, such as a favourites page or several conditions at once. One way to say a thing rather than two that overlap
- [X] T034c [US1] Let a subscription be created switched off, so a rule can be written now and enabled once its preview looks right

**Checkpoint**: US1 is independently demonstrable — quickstart Scenarios 1–3 pass.

---

## Phase 4: User Story 2 — Narrow what gets downloaded (P2)

**Goal**: Rules keep unwanted works out, and a rejected candidate says which rule rejected it.

**Independent test**: Add an exclude-tag rule, run a check, confirm the work is not queued and the
reason names the rule.

### Tests

- [X] T034 [P] [US2] Unit-test rule evaluation exhaustively in `crates/lanrurugi-api/src/subscriptions/matcher.rs` — required/excluded tags, rating floor, excluded categories, and their interactions

### Implementation

- [X] T035 [US2] Implement filter evaluation as pure functions in `matcher.rs`, returning *which* rule rejected a candidate rather than a bare boolean (FR-011)
- [X] T036 [US2] Record per-candidate verdicts and reasons into the cycle record in `runner.rs`
- [X] T037 [US2] Defer newer revisions of held works to the existing revision comparison instead of discarding them as duplicates (FR-009b) — already holds: deduplication is by canonical source URL and a revision carries its own, so it never reports `AlreadyHeld`; which revision supersedes which is decided by `download_manager::version_history::classify` at download time, where the series history is actually available. Pinned by a regression test in `runner.rs`
- [ ] T038 [US2] Add the post-download content-duplicate backstop (FR-009a) alongside the existing duplicate handling in `crates/lanrurugi-api/src/download_manager/ingest.rs`
- [X] T039 [P] [US2] Add filter editing to `SubscriptionsSection.tsx`
- [X] T040 [P] [US2] Surface per-candidate rejection reasons in the subscription's recent-activity view in `SubscriptionsSection.tsx` (the rejecting *rule* is the payload: a too-strict subscription and a broken one are indistinguishable without it)

**Checkpoint**: quickstart Scenario 4 passes; US1 still works unchanged.

---

## Phase 5: User Story 3 — See and act on failures (P2)

**Goal**: Works that could not download are visible with their reason, and actionable.

**Independent test**: Force a download failure, confirm the reservation entry appears with the real
reason, retry it successfully after removing the obstacle.

### Implementation

- [X] T041 [US3] Create reservation entries on download failure, carrying source URL, reason, and originating subscription, in `crates/lanrurugi-api/src/subscriptions/reservations.rs`
- [X] T042 [US3] Implement retry / retry-all / discard endpoints in the same file per `contracts/subscription-api.md`
- [X] T043 [US3] Make later cycles consult discarded entries so a discarded work is not re-reserved (FR-016) — `settled_sources` is passed as `CycleContext::discarded` and consulted in `decide`; pinned by a regression test
- [X] T044 [US3] Bound reservation growth and tell the user when entries were dropped (FR-018) in `reservations.rs`
- [X] T045 [P] [US3] Build the reservation group in `apps/frontend/src/pages/Upload/ReservationGroup.tsx` — collapsed by default behind an icon control (FR-017)
- [X] T046 [US3] Mount the reservation group on the upload page beside the download queue in `apps/frontend/src/pages/Upload/`
- [X] T047 [P] [US3] Link each reservation entry to the subscription that produced it (FR-026) in `ReservationGroup.tsx`
- [X] T048 [P] [US3] Add reservation API types and hooks in `apps/frontend/src/api/types.ts` and `hooks.ts`
- [X] T049 [P] [US3] Add i18n keys for the reservation UI across all 14 locale files in `apps/frontend/src/i18n/locales/`

**Checkpoint**: quickstart Scenario 5 passes; US1 and US2 unaffected.

---

## Phase 6: User Story 4 — Stay within a download budget (P3)

**Goal**: A credit failure pauses the subscription or reserves the item, as the user chose.

**Independent test**: Trigger a credit failure on a pause-configured subscription; confirm it stops
and shows the reason, and that resume restores normal scheduling.

### Implementation

- [X] T050 [US4] Recognise the insufficient-credit failure signal and branch on the subscription's choice, in `runner.rs`
- [X] T051 [US4] Implement pause-with-reason and resume in `crates/lanrurugi-api/src/subscriptions/mod.rs` (FR-019, FR-020)
- [X] T052 [P] [US4] Show paused state and reason, with a resume control, in `SubscriptionsSection.tsx`
- [X] T053 [P] [US4] Add i18n keys for budget/pause states across all 14 locale files in `apps/frontend/src/i18n/locales/`

**Checkpoint**: quickstart Scenario 9 passes; all four stories work together.

---

## Phase 7: Authoring assistant coverage

**Purpose**: Without this, every new source is still hand-written — which defeats having an
assistant at all (FR-027).

- [X] T054 Consolidate the assistant's capability list into one definition (FR-028) — it is currently hardcoded in several places, e.g. `const PLUGIN_TYPES` in `crates/lanrurugi-api/src/plugin_wizard/lookup.rs`
- [X] T055 Teach the generation flow to produce a discovery capability in `crates/lanrurugi-api/src/plugin_wizard/generate.rs`
- [X] T056 Extend the draft trial run so a generated discovery capability is verifiable before saving (FR-029) in `crates/lanrurugi-api/src/plugin_wizard/trial_run.rs`

---

## Phase 8: Polish & cross-cutting

- [ ] T057 Test the inconclusive-cycle guard: a degraded check must not write Seen Work, and works it could not see must still be found once sign-in is restored (`crates/lanrurugi-api/src/subscriptions/runner.rs` tests)
- [X] T058 Report degraded/inconclusive checks distinctly from "nothing found" in the UI (FR-002f/h) in `SubscriptionsSection.tsx`
- [ ] T059 Handle a source raising its minimum interval after subscriptions already exist — bring them into line and tell the user — in `crates/lanrurugi-api/src/subscriptions/scheduler.rs`
- [X] T060 [P] Add structured, translatable failure reasons for every new failure path in `crates/lanrurugi-core/src/queue_error.rs`, matching the existing queue-error style
- [ ] T061 [P] Update `README.md`'s improvements section — subscriptions have no legacy equivalent
- [ ] T062 Run every scenario in `specs/010-subscription-orchestrator/quickstart.md` end to end, especially Scenario 6 step 4 (works found again after sign-in is restored)

---

## Implementation status (2026-10-04)

88 of 93 tasks complete.

**Completed since the 2026-10-03 note:**

- **T028/T031 — pending approvals, end to end.** A non-auto-download match now persists a
  `PendingApproval` record, and `GET /subscriptions/pending` plus the approve/dismiss endpoints let
  the user act on it. Approving hands the work to the ordinary download queue, inheriting its
  existing start/stop/retry behaviour instead of getting a path of its own; the pending record is
  deleted only after the queue accepts it. Dismissing marks the work seen, so the same question is
  not re-asked next cycle. This closes the hole that made the *default* mode unusable.
- **T033a/T033b — editing and check history.** Neither existed as a task before today, although
  FR-001/FR-023/FR-024 required editing and FR-011 required exposing per-candidate verdicts. Both
  were missing from the UI while their backend endpoints and frontend hooks already existed and were
  simply never called — the kind of gap that type-checking cannot see.
- **T040/T058 — verdicts and inconclusive outcomes in the UI.** Cycle history shows each candidate's
  verdict, including which rule rejected it, and renders an inconclusive cycle distinctly from a
  completed one that found nothing.

**Filter rules are now source-declared rather than built in (FR-011a–e).** The fields a subscription
can filter on follow from the SDK's own candidate type plus which keys an extension writes — the same
two signals `returns_version_history` uses — so a source with a page count or a publication date becomes
filterable without the host changing. Two earlier hard-coded fields (`minimum_age_secs`,
`require_rating`) were removed in favour of this; `required_tags`/`excluded_tags`/`minimum_rating`/
`excluded_categories` stay built in because they are cross-source and earn dedicated UI.

Verified end to end against real e-hentai data: of 25 candidates, "published at least 3 hours ago and
rated 4+" held 3 as premature (not recorded as seen, so reconsidered later), rejected 12 on rating, and
accepted 10. The 3 held are exactly the ones the previous rating-only rule would have waved through
while they still carried no rating.

**Still outstanding:**

- [ ] **T037, T038** — deferring newer revisions to the existing revision comparison, and the
  post-download content-duplicate backstop. The pre-download source check (T020) is in place, so
  duplicates are caught; these two close the remaining gap for *revisions* specifically.
- [ ] **T043** — consulting discarded reservations from the matcher, so a discarded work is not
  re-reserved. Recorded and tested at the data layer; not yet read back.
- [ ] **T057** — the inconclusive-guard integration test (the unit test exists).
- [ ] **T059** — bringing existing subscriptions into line when a source raises its minimum interval.
- [ ] **T061, T062** — README, and running `quickstart.md`'s ten scenarios end to end.

**Not verified end to end.** Compilation, unit tests, and route mounting were checked. No real
subscription check has been run against a live source, and none of `quickstart.md`'s ten scenarios
has been executed. The UI has not been visually reviewed.
