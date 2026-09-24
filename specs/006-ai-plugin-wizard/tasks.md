---

description: "Task list for AI Plugin Creation Wizard (User Stories 1-7)"
---

# Tasks: AI Plugin Creation Wizard

**Input**: Design documents from `/specs/006-ai-plugin-wizard/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md (all present)

**Tests**: Not explicitly requested in spec.md (no TDD mandate), so this list does not include a
separate "write failing tests first" phase per story. Each story instead ends with a task that runs
its `quickstart.md` validation scenario(s) as the acceptance check. A handful of `cargo test` unit
tests are still included in Setup/Foundational and Polish for logic that's otherwise hard to verify
manually (the tool-calling message loop, the redirect-trail-capturing fetch helper) — this mirrors
001's own precedent of adding focused tests where "otherwise unverifiable except by such a test"
applies, not a blanket TDD policy.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to (US1–US7, per spec.md)
- Every task names an exact file path, per plan.md's Project Structure

## Path Conventions

Backend: new `crates/lanrurugi-api/src/plugin_wizard/` module (plus one new function in the existing
`crates/lanrurugi-llm/src/lib.rs`, plus new rows in `crates/lanrurugi-api/policy/route_policy.csv`).
Frontend: new `apps/frontend/src/pages/PluginWizard/` directory. See `plan.md` § Project Structure
for the full tree.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: New dependency installation and empty scaffolding for the module/page this feature adds.

- [X] T001 [P] Add `@uiw/react-codemirror@^4.25.11`, `@codemirror/lang-javascript@^6.2.5`,
      `codemirror@^6.0.2` to `apps/frontend/package.json` (verify these are still the actual
      latest stable versions at implementation time per constitution's dependency-version rule —
      research.md §5 records the versions checked when this plan was written, not a hard pin)
- [X] T002 [P] Create `crates/lanrurugi-api/src/plugin_wizard/mod.rs` with an empty `Router<AppState>`
      and register it under `/plugin-wizard` in `crates/lanrurugi-api/src/lib.rs`'s existing router
      assembly, alongside the other feature routers

**Checkpoint**: New dependency installed, empty module wired into the app but not yet doing anything.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Infrastructure every user story's tasks below actually call into.

**⚠️ CRITICAL**: No user story task may begin until this phase is complete.

- [X] T003 Add Session-only access control for this feature (FR-023 — logged-in users only, API
      tokens of either role rejected) via this project's existing Casbin policy mechanism, **not**
      a new middleware/guard: in `crates/lanrurugi-api/policy/route_policy.csv`, add a `deny` row
      for each of `token_admin`/`token_guest`/`anonymous` × each of the four new endpoint paths
      (`/api/plugin-wizard/lookup`, `/api/plugin-wizard/generate`, `/api/plugin-wizard/trial-run`,
      `/api/plugin-wizard/save`, all `POST`) — 12 rows total, following the exact pattern of the
      existing `/api/archives DELETE` session-only rows (route_policy.csv lines 32-62). **Must**
      include the full `/api/...` prefix on every row — the file's own comment (lines 22-31)
      documents a real, previously-shipped incident where a rule written without that prefix
      silently never matched and let a Token through a route meant to be Session-only; verify with
      a real end-to-end HTTP request (matching `auth_flow.rs`'s own
      `session_only_route_rejects_a_real_admin_token_but_accepts_a_real_session` test pattern), not
      just the in-memory `authz::tests` unit tests, which don't exercise axum's routing layer
- [X] T004 In `apps/frontend/src/pages/PluginWizard/useWizardSession.ts`, add a per-`TypeSession`
      in-flight-operation guard addressing the spec's own Edge Case ("user triggers a new
      generate/fix/edit action while a trial run is still in progress"): while any generate/
      trial-run/save call is in flight for a given type, disable the UI actions that would start a
      conflicting one for that same type (generate, AI auto-fix, trial-run, manual-edit-triggered
      re-run) rather than allowing two concurrent calls to race and leave inconsistent state;
      actions for a *different* type are unaffected — a decision, not left implicit
- [X] T005 In `crates/lanrurugi-llm/src/lib.rs`, factor the existing `chat()`'s HTTP-call/status-
      code-error-translation body into a private shared helper, then add a new
      `pub async fn tool_chat(redis_config, messages: Vec<Message>, tools: Vec<Tool>, temperature,
      max_tokens) -> Result<ToolChatResponse, String>` using model `deepseek-v4-pro` (not
      `deepseek-chat` — research.md §1) that calls the shared helper and returns either
      `ToolChatResponse::Content(String)` or `ToolChatResponse::ToolCalls(Vec<ToolCall>)` depending
      on whether the response has a `tool_calls` array; existing `chat()`/`json_chat()` callers
      (`tankoubons.rs`, `recommend_llm.rs`, `artist_backfill.rs`) untouched
- [X] T006 [P] Create `crates/lanrurugi-api/src/plugin_wizard/fetch.rs`: a `fetch_with_redirect_trail
      (url: &str) -> Result<FetchResult, FetchError>` using a `reqwest::Client` built with a custom
      `redirect::Policy` that records every hop via `attempt.previous()` into a trail and stops once
      the trail exceeds a fixed cap constant, returning a distinct `FetchError::RedirectCapExceeded`
      variant carrying the trail so far (FR-011, research.md §4); also define the concrete per-fetch
      and per-trial-run-call timeout constants from research.md §6 here, applied via
      `reqwest::ClientBuilder::timeout`/a `tokio::time::timeout` wrapper respectively
- [X] T007 [P] In `apps/frontend/src/pages/PluginWizard/useWizardSession.ts`, define the
      `WizardSession`/`DomainLookupResult`/`TypeSession`/`DraftRevision`/`TrialRunResult` TypeScript
      types exactly per data-model.md, plus an empty reducer/state hook skeleton (no real actions
      wired up yet — later stories add to it; T004's in-flight guard lives alongside this)
- [X] T008 [P] Create `apps/frontend/src/pages/PluginWizard/index.tsx` (default-exported page
      component only, per constitution Principle VII) mounted at a new route (e.g. `/plugins/wizard`,
      linked from the existing Plugins page's own navigation), rendering a step container with no
      real steps yet

**Checkpoint**: `/api/plugin-wizard/*` rejects tokens end-to-end; `tool_chat()` callable and
unit-testable in isolation; page route reachable with an empty shell; foundation ready for all seven
user stories.

---

## Phase 3: User Story 1 - Look up existing plugins by domain and choose which types to create (Priority: P1) 🎯 MVP

**Goal**: Given a domain, accurately report per-type plugin coverage and let the user select 1-3
missing types with per-type inputs collected.

**Independent Test**: Look up a domain with partial coverage; confirm covered types are marked
unselectable and missing types can be chosen (per spec's own Independent Test for this story).

### Implementation for User Story 1

- [X] T009 [US1] Implement `POST /plugin-wizard/lookup` in
      `crates/lanrurugi-api/src/plugin_wizard/lookup.rs`: for each of `login`/`metadata`/`download`,
      reuse `discover_namespaces()` + `PluginPool::plugin_info()` (same pattern
      `plugins.rs::find_matching_plugin` already uses for a URL match) to test each installed
      plugin's `url_pattern` against the input domain, returning `TypeCoverage` per
      `contracts/plugin-wizard-api.md` (including the covered plugin's full `source_code` for
      FR-009's later reference-sample use)
- [X] T010 [US1] Create `apps/frontend/src/pages/PluginWizard/DomainLookupStep.tsx`: domain input +
      submit, calling the T009 endpoint and storing the result via `useWizardSession.ts`
- [X] T011 [US1] Create `apps/frontend/src/pages/PluginWizard/TypeSelectionStep.tsx`: renders the
      three types' coverage state (covered types shown disabled with which plugin they map to,
      per FR-004), multi-select up to 3 from the missing types, and the FR-003 "fully covered,
      nothing to create" empty state. Per spec's own Edge Case on switching types mid-session:
      deselecting a type **discards** its `TypeSession` entirely (including all `revisions`/
      `trialRuns` history) rather than keeping it around inert; reselecting that same type later
      starts a genuinely fresh `TypeSession`, it never resurrects the discarded one — implement this
      as the deselect handler's explicit behavior, not an incidental side effect of some other change
- [X] T012 [US1] Extend `TypeSelectionStep.tsx` (or a sibling `TypeDetailsForm.tsx`) with, per
      selected type: a page-feature-description textarea (FR-005), a dynamic list input for ≥3
      target links (metadata/download) or a credentials form (login), and an optional auxiliary-
      reference-URL list input (FR-006)
- [X] T013 [US1] Add the FR-007 per-type login-dependency prompt: when both `login` and at least one
      metadata/download type are selected, render an explicit yes/no question for each
      metadata/download type ("does this need login to work"), writing the answer into that type's
      `TypeSession.dependsOnLogin` — never defaulted, must be explicitly answered before proceeding
- [X] T014 [US1] Run `quickstart.md` scenarios 1–2 against a real dev environment; fix any
      discrepancy found before moving on

**Checkpoint**: Domain lookup and type/input selection fully functional and independently
verifiable — no generation/trial-run/save capability yet, by design.

---

## Phase 4: User Story 2 - Generate usable plugin code from a description (Priority: P1)

**Goal**: Produce a working `.ts` draft from a type's collected inputs, with AI able to fetch real
page content mid-generation rather than guessing from text alone.

**Independent Test**: Given a confirmed type + description, generation returns a structurally valid
draft; confirm (via logs) the tool-calling loop actually invoked `fetch_page` at least once.

### Implementation for User Story 2

- [X] T015 [US2] Create `crates/lanrurugi-api/src/plugin_wizard/generate.rs`: build the system prompt
      (SDK type/interface docs +, if present, `reference_sample_code` per FR-009) and user prompt
      from the request body per `contracts/plugin-wizard-api.md`, declare the single `fetch_page`
      tool (same JSON schema as the contract doc), and call `tool_chat()` (T005)
- [X] T016 [US2] In `generate.rs`, implement the agentic loop: while the response is `ToolCalls`,
      execute each requested fetch via `fetch_with_redirect_trail` (T006) against the requested URL
      (defaulting to the first supplied test link/auxiliary URL if AI's first call doesn't specify
      one — contract doc's own note on this), append a `role: "tool"` message with the result
      (including `redirect_cap_exceeded` outcomes, FR-011) and call `tool_chat()` again; stop once a
      `Content` response is returned and treat it as the final code
- [X] T017 [US2] Handle the `previous_code`/`previous_error` request fields in `generate.rs`'s prompt
      construction (present only for an AI-auto-fix call, FR-017/US5) — same endpoint, same loop,
      different starting user-prompt content; `null` for a fresh generation
- [X] T018 [US2] In `generate.rs`, detect a final `Content` response that isn't parseable as a plugin
      module (no `pluginInfo()` export found via a lightweight syntax check, not a full TS
      typecheck) and return `422 ai_output_not_code` with the raw output, per spec Edge Cases and the
      contract doc, rather than handing invalid content back as a usable draft
- [X] T019 [US2] In `useWizardSession.ts`, implement the FR-013 generation-order rule: when a batch
      includes both `login` and a type with `dependsOnLogin === true`, defer issuing that type's
      first `generate` call until the `login` `TypeSession` has a revision with a successful trial
      run; types with `dependsOnLogin !== true` are never blocked by this (data-model.md's State
      Transitions)
- [X] T020 [US2] Create `apps/frontend/src/pages/PluginWizard/GenerationStep.tsx`: triggers `POST
      /plugin-wizard/generate`, shows a loading state during the (possibly multi-tool-call) wait
      (respecting T004's in-flight guard), and on success appends a new `DraftRevision` (`origin:
      "ai-generated"`) to the type's `revisions` via `useWizardSession.ts`; on `422`/`503` shows the
      specific error per FR-024/spec Edge Cases, producing no leftover draft state
- [X] T021 [US2] Run `quickstart.md` scenario 3; confirm via logs/trace that `fetch_page` was
      actually invoked during generation, not just that a draft came back

**Checkpoint**: Generation works end-to-end for any single type, including the login-dependency
ordering rule — US1 + US2 together already let a user get from "I have a domain" to "I have a code
draft."

---

## Phase 5: User Story 3 - Trial-run to validate the generated result (Priority: P1)

**Goal**: Real, sandboxed execution of any draft against real target(s), with per-link results for
metadata/download and zero persistence in the real plugin list.

**Independent Test**: Trial-run a draft against 3 links where one is deliberately structured
differently; confirm independent per-link success/failure, and confirm the real plugin list is
unchanged before/after.

### Implementation for User Story 3

- [X] T022 [US3] Create `crates/lanrurugi-api/src/plugin_wizard/trial_run.rs`: on each call, write
      `code` to `plugins/custom/_wizard/<uuid>.ts` (new directory, distinct from `upload_plugin`'s
      own staging use of `custom/` — research.md §2), call `PluginPool::plugin_info()` against that
      namespace first (same zero-permission probe `upload_plugin` does) to catch a syntactically
      invalid draft early
- [X] T023 [US3] In `trial_run.rs`, for `plugin_type: "metadata" | "download"`, call
      `PluginPool::execute()` once per supplied link (reusing the one staged file, per research.md
      §2's "stage once, call N times" note), collecting each link's own success/failure + data/error
      into the `per_link` array shape from `contracts/plugin-wizard-api.md`
- [X] T024 [US3] In `trial_run.rs`, for `plugin_type: "login"`, call `PluginPool::execute()` once
      with the supplied `credentials` in the call's `hostArgs`, returning the single
      success/failure + sanitized detail shape — `credentials` never logged, never included in any
      value returned to the caller beyond what the plugin's own login result legitimately reports
      (FR-012)
- [X] T025 [US3] In `trial_run.rs`, guarantee the staged file at `plugins/custom/_wizard/<uuid>.ts`
      is deleted on every return path (success, per-link partial failure, `plugin_info()` probe
      failure, or a mid-call panic-safe cleanup) — never left behind regardless of outcome
- [X] T026 [US3] In `trial_run.rs`, return `400 invalid_plugin_code` (contract doc shape) when the
      `plugin_info()` probe itself fails, distinguishing "the code doesn't parse/isn't a valid
      plugin" from a legitimate metadata/download/login runtime failure
- [X] T027 [US3] Create `apps/frontend/src/pages/PluginWizard/TrialRunResult.tsx`: renders
      per-link results independently (US3 AC4 — no single pass/fail verdict masking individual
      links) for metadata/download, or the single outcome for login; appends the result to the
      active `DraftRevision.trialRuns` via `useWizardSession.ts` (respecting T004's in-flight guard)
- [X] T028 [US3] Add the US3 AC6 save-gate check in `useWizardSession.ts`/the save UI (built out
      fully in US6, T030-T031): a type cannot be confirm-saved until its active revision has at
      least one trial-run result for every required target (all supplied links for metadata/
      download, or the one login attempt) — implemented here as the underlying state check US6's
      UI task consumes
- [X] T029 [US3] Run `quickstart.md` scenarios 4–5, including the explicit `GET /api/plugins/
      metadata` before/after diff proving a trial run leaves no trace

**Checkpoint**: US1 + US2 + US3 form the complete MVP loop — look up, generate, validate for real.
This alone already delivers a usable (if manual-save-free) wizard.

---

## Phase 6: User Story 6 - Confirm-save the generated plugin (Priority: P1)

**Goal**: Turn a validated draft into a real, installed plugin, safely.

**Independent Test**: Save a validated draft; confirm it's immediately callable like any other
plugin, and that a namespace conflict is rejected without touching the existing file.

### Implementation for User Story 6

- [X] T030 [US6] Create `crates/lanrurugi-api/src/plugin_wizard/save.rs`: implement `POST
      /plugin-wizard/save` by extracting and reusing `upload_plugin`'s own namespace-conflict-check
      (FR-021) and move-into-`plugins/<category>/`-with-rollback-on-failure logic (FR-022) as a
      shared helper both `upload_plugin` and this new handler call, rather than duplicating that
      logic (per constitution's "near-identical logic... factored into a shared helper" rule) — see
      contracts/plugin-wizard-api.md for exactly which parts of `upload_plugin`'s flow are reused vs.
      skipped (no from-scratch staging/validation needed since the code already passed a real trial
      run)
- [X] T031 [US6] Add a confirm-save action to the wizard UI (e.g. in `TrialRunResult.tsx` or a
      dedicated `SaveButton.tsx`), disabled until T028's save-gate check passes, showing the `200`
      success / `409` conflict (with a rename-and-retry affordance) / `400`/`500` failure states
      from `contracts/plugin-wizard-api.md`
- [X] T031a [US6] Add FR-026's `generated_by_wizard?: boolean` field to `PluginInfoResult`
      (`crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`) and `PluginInfo`
      (`crates/lanrurugi-plugin/src/protocol.rs`); instruct `generate.rs`'s system prompt to always
      set `generated_by_wizard: true` in generated code; thread the field through
      `list_plugins`'s response JSON (`plugins.rs`) and the frontend `PluginInfo` type
      (`api/types.ts`)
- [X] T031b [US6] Add FR-027's wizard-created badge to `PluginCard.tsx` (a small icon next to the
      plugin name, shown when `plugin.generated_by_wizard` is true)
- [X] T031c [US6] Implement FR-028's `GET /plugins/export?namespace=...` endpoint
      (`plugins.rs::export_plugin`) that zips the plugin's own `.ts` file and returns it as a
      download; add an export button/link to `PluginCard.tsx` for every plugin (not just
      wizard-created ones)
- [X] T031d [US6] Implement FR-029: keep each `TypeSession`'s draft/official state visible
      throughout the wizard session, not only at the save moment — `TrialRunResult.tsx` already
      renders a distinct "Installed as {namespace}" state once `savedNamespace` is set (T031's
      `onSaved` wiring), replacing the draft actions entirely for that type
- [X] T031e [US6] Implement FR-031: show a dismissible persistence reminder (pointing at T031c's
      export action) every time a confirm-save succeeds, since the host can't reliably tell
      whether `plugins_dir` is host-mounted or ephemeral (FR-030) — `TrialRunResult.tsx`'s
      `triggerSave()` now fires this as a second toast after the save-success toast
- [X] T032 [US6] Run `quickstart.md` scenarios 8–9, including the explicit before/after byte-diff of
      the existing plugin file on a `409` conflict, plus a new scenario verifying T031a's
      `generated_by_wizard` marker round-trips through save → `GET /plugins/{type}` →
      `PluginCard.tsx`'s badge, and that `GET /plugins/export` returns a valid zip for both a
      wizard-created and a hand-written plugin

**Checkpoint**: All four P1 stories (US1/US2/US3/US6) complete — a user can go from a bare domain to
a real, working, installed plugin. This is the feature's full MVP.

---

## Phase 7: User Story 4 - Manually edit the code and re-validate (Priority: P2)

**Goal**: In-wizard code editing with immediate re-trial-run, no export/import round-trip.

**Independent Test**: Edit a draft's code, save the edit, and trigger a new trial run without
leaving the page; confirm prior trial-run history for the earlier revision is untouched.

### Implementation for User Story 4

- [X] T033 [US4] Create `apps/frontend/src/pages/PluginWizard/CodeEditor.tsx`: thin wrapper around
      `@uiw/react-codemirror` configured with `javascript({ typescript: true })` (T001's
      dependencies), controlled by the active `DraftRevision.code`
- [X] T034 [US4] Wire `CodeEditor.tsx` edits into `useWizardSession.ts`: an edit creates a new
      `DraftRevision` (`origin: "manual-edit"`, empty `trialRuns`) rather than mutating the existing
      one in place (US4 AC1 — prior trial-run history stays attached to its own revision,
      unaffected), and exposes a "re-run trial" action against the new revision reusing T022's
      endpoint
- [X] T035 [US4] Run `quickstart.md` scenario 6

**Checkpoint**: Manual editing available as an alternative/complement to AI generation for any
already-generated draft.

---

## Phase 8: User Story 5 - AI auto-fixes a failed trial run (Priority: P2)

**Goal**: One-click AI fix from a failed trial run's error, auto-re-run, with a 3-attempt cap that
never discards history.

**Independent Test**: Trigger auto-fix on a failure repeatedly; confirm the cap disables the action
without clearing prior rounds, and any round remains selectable/savable.

### Implementation for User Story 5

- [X] T036 [US5] Add an "AI auto-fix" action (e.g. in `TrialRunResult.tsx`) that calls `POST
      /plugin-wizard/generate` (T015-T017) with `previous_code` set to the active revision's code
      and `previous_error` set to the most recent trial-run failure detail, then auto-triggers a
      trial run (T022) against the resulting new `DraftRevision` (`origin: "ai-auto-fix"`) with no
      further user action required (US5 AC1)
- [X] T037 [US5] In `useWizardSession.ts`, track `TypeSession.autoFixAttemptsUsed` (incrementing on
      each consecutive auto-fix, per data-model.md's "resets only if a manual edit/fresh generation
      breaks the chain" rule) and disable the auto-fix action once it reaches 3 (FR-018) — critically,
      without touching `revisions` at all when the cap is hit (US5 AC2, this session's earlier
      `/speckit-clarify` outcome)
- [X] T038 [US5] Create `apps/frontend/src/pages/PluginWizard/DraftHistoryPanel.tsx`: lists every
      revision for the active type (code + trial-run outcomes, US5 AC3) and lets the user set any
      one as `activeRevisionIndex` — usable, and savable via T031, independent of whether it's the
      newest one (US5 AC4)
- [X] T039 [US5] Run `quickstart.md` scenario 7

**Checkpoint**: Both P2 fix paths (manual edit, AI auto-fix) available; full draft history browsable
and independently actionable.

---

## Phase 9: User Story 7 - Add a login plugin afterward, based on an AI suggestion (Priority: P2)

**Goal**: When a metadata/download trial run fails, let AI (not a local heuristic) judge whether it's
login-related, and if so guide the user to add/associate a login plugin without restarting the
wizard.

**Independent Test**: Trial-run against a fixture page requiring login without having selected the
login type up front; confirm an AI-sourced (not hardcoded) suggestion appears, and that accepting it
results in a working association and a passing re-run.

### Implementation for User Story 7

- [X] T040 [US7] In `trial_run.rs`, after any metadata/download run that has ≥1 failed link, make a
      second `tool_chat()` call (no tools needed — pass an empty `tools` vec — a plain
      classification prompt over the failed link(s)' `error`/`redirectTrail` data) asking whether
      the failure is plausibly login/permission-related; attach the result as `login_suggestion` in
      the response per `contracts/plugin-wizard-api.md` — never a local status-code/keyword
      heuristic (FR-010)
- [X] T041 [US7] Add the "this might need login" suggestion UI (e.g. in `TrialRunResult.tsx`),
      shown only when `login_suggestion.relevant === true`, with a dismiss option (spec Edge Cases —
      the user must be able to ignore it) and an "add a login plugin" entry point
- [X] T042 [US7] In `useWizardSession.ts`, implement the FR-025 acceptance flow: if
      `DomainLookupResult.login.covered`, set `loginAssociation = { source: "existing", namespace }`
      directly; otherwise spin up a `login` `TypeSession` (reusing US1's per-type input collection
      and US2's generate flow) the same way the up-front FR-013 path does, setting `loginAssociation
      = { source: "generated-this-session", namespace }` once that new login draft passes its own
      trial run
- [X] T043 [US7] Wire `loginAssociation` into a fresh `generate` call for the originally-failing
      type (new `DraftRevision`, carrying the `login_from` association declaration per FR-008), and
      expose a "re-run the previously failed link(s)" action against it (US7 AC5)
- [X] T044 [US7] Run `quickstart.md` scenarios 10–11 — scenario 11 specifically requires capturing
      real outbound HTTP request bodies to `api.deepseek.com` (e.g. via a local proxy or temporary
      request logging) to directly confirm test credentials never appear in any LLM request, not
      just reading the code and assuming FR-012 holds (constitution's verification-discipline bullet
      — this is a security property, not a UI detail, but the same "observe it actually holding"
      standard applies)

**Checkpoint**: All 7 user stories complete and independently verified.

---

## Phase 10: Polish & Cross-Cutting Concerns

**Purpose**: Automated test coverage and release-readiness checks spanning multiple stories.

- [X] T045 [P] `cargo test` unit tests for `tool_chat()`'s message-loop plumbing
      (`crates/lanrurugi-llm/`) against a mocked HTTP responder covering both a plain-content
      response and a `tool_calls` response
- [X] T046 [P] `cargo test` unit tests for `fetch_with_redirect_trail`
      (`crates/lanrurugi-api/src/plugin_wizard/fetch.rs`) covering a normal redirect chain, a
      no-redirect direct hit, and the cap-exceeded path, against a local test HTTP server
- [X] T049 Run `mise run check` (rust-check + frontend-lint) and fix any failure before considering
      this feature complete
- [X] T050 Update `README.md`/`README.en.md`/`README.ja.md` (three-language sync, per this
      repository's own pre-push convention) to mention the AI plugin creation wizard under
      "Improvements over LANraragi" — issue #78 is milestone `m0`, so this is release-facing, not
      optional polish

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately.
- **Foundational (Phase 2)**: Depends on Setup — BLOCKS all user stories.
- **User Stories (Phase 3-9)**: All depend on Foundational completion.
  - P1 stories (US1 → US2 → US3 → US6) have a natural implementation order (each is the input the
    next needs to be useful) but each remains independently testable per its own Independent Test.
  - P2 stories (US4, US5, US7) each build on top of the P1 MVP being complete, but are independent
    of each other and can be built in any order (US4/US5/US7 as listed only reflects a reasonable
    default, not a hard dependency).
- **Polish (Phase 10)**: Depends on all desired user stories being complete.

### User Story Dependencies

- **US1 (P1)**: No dependencies beyond Foundational.
- **US2 (P1)**: Functionally needs US1's collected inputs to have something to generate from, but
  its own endpoint/logic (T015-T019) has no code dependency on US1's frontend tasks.
- **US3 (P1)**: Needs a draft to trial-run — practically follows US2, no hard code dependency.
- **US6 (P1)**: Needs a trial-run result to gate saving (T028) — follows US3.
- **US4 (P2)**: Needs an existing draft (US2) to edit; otherwise independent.
- **US5 (P2)**: Reuses US2's `generate` endpoint (T015-T017, via the `previous_code`/
  `previous_error` fields already built there) and US3's trial-run endpoint — no new backend
  endpoint of its own.
- **US7 (P2)**: Extends US3's `trial_run.rs` (T040) and reuses US1's per-type input collection +
  US2's generate flow for the supplementary login plugin (T042) — the one story with the most
  cross-story reuse, by design (it's explicitly a recovery path composed from existing pieces, not
  new capability).

### Parallel Opportunities

- T001/T002 (Setup) in parallel.
- T006/T007/T008 (Foundational) in parallel — three different files, no dependencies among them.
  T003 (policy CSV) and T004/T005 (shared `useWizardSession.ts` guard / `lanrurugi-llm`) are not
  marked `[P]`: T003 is a distinct, quick file edit but is listed first as the security-critical
  item to land before anything else in this phase; T004 and T007 both touch
  `useWizardSession.ts`, so they're sequential edits to one file despite being logically separable.
- Within US3, T023/T024 (metadata/download vs. login trial-run branches) touch the same file
  (`trial_run.rs`) so are not marked `[P]` despite being logically independent — sequential edits to
  one file, not a real parallelization opportunity.
- T045-T046 (Polish tests) in parallel — two independent test files.

---

## Parallel Example: Foundational Phase

```bash
# Launch the three independent-file Foundational tasks together:
Task: "Create fetch.rs (+ timeout constants) in crates/lanrurugi-api/src/plugin_wizard/"
Task: "Define WizardSession types in apps/frontend/src/pages/PluginWizard/useWizardSession.ts"
Task: "Create apps/frontend/src/pages/PluginWizard/index.tsx page shell"

# T003 (route_policy.csv) and T005 (tool_chat()) are quick, independent single-file edits too,
# but land T003 first — it's the security-critical item for this phase.
```

---

## Implementation Strategy

### MVP First (User Stories 1, 2, 3, 6 — all P1)

1. Complete Phase 1: Setup
2. Complete Phase 2: Foundational (CRITICAL — blocks everything, including the Session-only access
   control in T003)
3. Complete Phase 3: User Story 1 (domain lookup + selection)
4. Complete Phase 4: User Story 2 (generation)
5. Complete Phase 5: User Story 3 (trial run)
6. Complete Phase 6: User Story 6 (save)
7. **STOP and VALIDATE**: run all of `quickstart.md`'s scenarios 1-5, 8-9 — this is a complete,
   shippable wizard even without US4/US5/US7.

### Incremental Delivery

1. Setup + Foundational → foundation ready.
2. US1 → US2 → US3 → US6 → full MVP loop working, demoable.
3. US4 (manual edit) → adds a fix path for technical users.
4. US5 (AI auto-fix) → adds a fix path for non-technical users.
5. US7 (login recovery) → adds the "didn't know I needed login" recovery path.
6. Polish → automated coverage + release-readiness.

Each P2 story is a genuinely optional, independently valuable increment on top of the P1 MVP — none
of them block shipping the MVP first if time is constrained, though all three are in scope for
milestone `m0` per issue #78 and should be completed before release, not deferred to `m1`.

---

## Notes

- [P] tasks = different files, no dependencies.
- [Story] label maps task to specific user story for traceability.
- Every endpoint task cites the exact `contracts/plugin-wizard-api.md` section it implements —
  cross-check against that doc, not just this file, when implementing.
- Commit after each task or logical group, per this repository's own commit conventions (feature-
  level commits, not one commit per task).
- Run `mise run check-crate -- lanrurugi-api lanrurugi-llm` (fast, crate-scoped) after each backend
  task, not just at the end (T049) — catches compile errors in seconds per this repository's own
  `cargo check`-before-`mise run dev-rebuild` convention.
