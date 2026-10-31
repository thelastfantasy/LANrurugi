---

description: "Task list for 005-download-plugin-progress"
---

# Tasks: Download Plugin Progress, Concurrency & Rate Limiting

**Input**: Design documents from `/specs/005-download-plugin-progress/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md (all present)

**Tests**: Not explicitly requested in spec.md; no test-task phases included, matching the
established convention in this repo's other feature task lists (001/002/003 also omit a separate
test-task phase). Verification is via quickstart.md's 12 runnable scenarios plus the project's
standing `cargo test`/`cargo clippy`/`deno check`/lefthook quality gates.

**Organization**: Tasks are grouped by user story (spec.md's US1–US4) to enable independent
implementation and testing of each story.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3, US4)
- Every task includes its exact file path(s)

## Path Conventions

Web application (Rust backend + React SPA frontend), matching 001's established structure:
- Backend: `crates/lanrurugi-core/`, `crates/lanrurugi-api/`, `crates/lanrurugi-plugin/`
- Plugins: `plugins/download/*.ts`
- Frontend: `apps/frontend/src/`

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Dependency additions and module scaffolding needed before any user story's real
implementation work can start.

- [X] T001 Add `"stream"` to the `reqwest` feature list in the workspace `Cargo.toml` (research.md
  §3) — re-verify at implementation time that `0.13.4` (or whatever is then the actual latest
  stable) is still current per the constitution's "verify at implementation time" rule, rather
  than hard-pinning the version cited in research.md.
- [X] T002 [P] Add the `zip` crate (research.md §1 — latest stable, MIT-licensed) as a dependency
  of `crates/lanrurugi-api/Cargo.toml` (re-verify current latest version at implementation time).
- [X] T003 [P] Add the `governor` crate (research.md §2 — latest stable, MIT-licensed) as a
  dependency of `crates/lanrurugi-api/Cargo.toml` (re-verify current latest version at
  implementation time).
- [X] T004 Create the `crates/lanrurugi-api/src/download_manager/` module directory with an empty
  `mod.rs`, `domain_rules.rs`, and `rate_limit.rs` (per plan.md's Project Structure), and add
  `mod download_manager;` to `crates/lanrurugi-api/src/lib.rs`.

**Checkpoint**: Workspace builds with the three new dependencies wired in and the empty module
skeleton in place.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Core data-model/SDK/protocol changes every user story depends on. No user story's
acceptance scenarios can be demonstrated until this phase is complete, since even the simplest
progress-bar story (US1) requires the SDK contract change, the real Rust-side download path, and
at least one migrated plugin to exercise it against.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [X] T005 [P] Extend `DownloadResult`/add `PluginOptionsResult` types in
  `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts` per
  `contracts/plugin-download-protocol.md` (`downloads[]`, `file_path` staying as-is, new
  `pluginOptions()` export documentation and `PluginOptionsResult`/`DomainRule` types).
- [X] T006 [P] Add the `downloaded_bytes: Option<u64>` and `total_bytes: Option<u64>` fields to
  `JobStatus` in `crates/lanrurugi-core/src/jobs.rs`, plus a new
  `set_download_progress(&self, id: &str, downloaded: u64, total: Option<u64>)` method on
  `JobRegistry` (data-model.md's Download Job extension) — mirror `set_progress`'s existing
  clamping/locking pattern.
- [X] T007 Add a `domain_rules.rs` implementation in
  `crates/lanrurugi-api/src/download_manager/domain_rules.rs`: a `DomainRule` struct
  (`pattern: Option<String>`, `max_concurrent: Option<u32>`, `max_bytes_per_sec: Option<u64>`) and
  a `resolve<'a>(rules: &'a [DomainRule], hostname: &str) -> ResolvedLimits` function implementing
  exact-hostname-beats-wildcard-beats-general-fallback precedence (data-model.md's Domain Rule
  matching rules, spec FR-007/FR-009). Depends on T004.
- [X] T008 Implement the per-domain concurrency gate in
  `crates/lanrurugi-api/src/download_manager/mod.rs`: a `DownloadManager` struct holding a
  `tokio::sync::Mutex<HashMap<String, Arc<Semaphore>>>` (no new crate needed — this project's
  existing `tokio`/`std` dependencies are sufficient; `dashmap` was considered during analysis and
  rejected to avoid an unneeded new dependency) keyed by the *resolved* domain-rule pattern (not
  raw hostname, so wildcard-sharing works per spec US2 Acceptance Scenario 2), with an
  `acquire_concurrency_permit(&self, hostname: &str, rules: &[DomainRule]) -> OwnedSemaphorePermit`
  method. **Note (FR-006 correctness)**: `Semaphore`'s capacity is fixed at construction — if a
  user changes `max_concurrent` for a domain via `PUT /api/plugins/{namespace}/options` (T026),
  the existing `Arc<Semaphore>` for that resolved pattern MUST be replaced with a freshly
  constructed one at the new capacity (not resized in place, which `tokio::sync::Semaphore` has no
  API for), or the override silently has no effect. Depends on T003, T007.
- [X] T009 Implement the per-domain rate limiter in
  `crates/lanrurugi-api/src/download_manager/rate_limit.rs` using `governor`'s
  `RateLimiter::until_n_ready` with N = bytes-per-chunk (research.md §2), keyed the same way as
  T008's concurrency map so a rule's `max_bytes_per_sec` and `max_concurrent` are resolved
  together. Depends on T003, T007.
- [X] T010 Implement the core streaming-download function in
  `crates/lanrurugi-api/src/download_manager/mod.rs`: given one `{url, method, headers,
  filename_hint}` (from `contracts/plugin-download-protocol.md`'s `downloads[]` shape), performs
  the real HTTP request via `reqwest` (default GET, per-request method override), reads the
  response via `bytes_stream()`, resolves the response's `Content-Length` into `total_bytes` when
  present (spec FR-002), calls `JobRegistry::set_download_progress` per chunk (throttled to avoid
  excessive lock churn), applies T009's rate limiter per chunk, and writes bytes to a staging path
  in `state.library.temp_dir` (mirroring `upload.rs::upload_archive`'s existing staging-path
  pattern). **Filename resolution** (contracts/plugin-download-protocol.md's `filename_hint`
  field docs): parse the response's `Content-Disposition` header (`filename=`/`filename*=`) first
  when present — matching legacy `Model::Upload.pm::download_url`'s own real-file-name behavior —
  and fall back to the plugin-supplied `filename_hint` only when that header is absent or
  unparseable; if neither is available, fall back to deriving a name from the URL path, matching
  `upload.rs::upload_archive`'s own existing filename-sanitization convention. Depends on T006,
  T008, T009.
- [X] T011 Wire T010's single-resource download into `catalogue_new_archive`/`ingest_file`: after
  a successful single-URL download, call `lanrurugi_scanner::pipeline::ingest_file` on the staged
  file exactly as `upload.rs::upload_archive` already does, including the same
  rename-into-`archive_dir`/`LRR_FILEMAP`-fixup steps (spec FR-004 — no half-cataloged archive on
  failure, so `ingest_file` is only called after the full byte transfer succeeds). Depends on T010.
- [X] T012 Rewrite `download_url()` in `crates/lanrurugi-api/src/plugins.rs` to: call
  `exec_download` as today, then branch on the (now-extended) result — `downloads` present →
  drive each entry through T010/T011 respecting T008/T009's per-domain gating (one entry: direct
  single-file path; multiple entries: download all before proceeding to Phase 4's bundling
  decision) — `file_path` present → keep today's exact behavior unchanged (spec's explicit
  fallback-escape-hatch requirement) — `error` present → keep today's exact failure behavior
  unchanged. Depends on T005, T011.
- [X] T013 [P] Migrate `plugins/download/chaika.ts`: replace its
  `return { download_url: url + "/download" };` with `return { downloads: [{ url: url +
  "/download" }] };` (contracts/plugin-download-protocol.md — trivial single-element case).
  Depends on T005.
- [X] T014 [P] Migrate `plugins/download/ehentai.ts`: replace its
  `return { download_url: finalURL.href };` with `return { downloads: [{ url: finalURL.href }] };`.
  Depends on T005.
- [X] T015 Run `cargo check`/`cargo fmt --check` (backend) and `deno check` against
  `plugins/download/chaika.ts`/`ehentai.ts` (frontend/plugin gate, matching this project's
  established `mise run convert-plugins`-adjacent verification convention) to confirm the
  Foundational phase compiles/type-checks cleanly end to end. Depends on T001–T014.

**Checkpoint**: Foundation ready — a real single-URL download (Chaika/EHentai-style) now performs
an actual Rust-side streaming HTTP fetch, reports byte progress into `JobRegistry`, and catalogs
the result via the existing `ingest_file` pipeline. User story implementation can now begin.

---

## Phase 3: User Story 1 - Real download progress on the Jobs page (Priority: P1) 🎯 MVP

**Goal**: A user watching the Jobs page during an in-flight download sees real, incrementally-
updating byte progress instead of a queued→finished jump (spec FR-001–FR-004).

**Independent Test**: Trigger a large-file download via Chaika or EHentai (already migrated in
Foundational) and watch `/jobs`; per quickstart.md §1, confirm intermediate progress states via
both the UI and `GET /api/jobs` directly. No concurrency/rate-limit/settings-UI *configuration*
work is needed for this story — note that T009's rate-limiter and T008's concurrency-gate code
paths already exist by this point (wired into T010 during Foundational), but with no domain rules
ever configured yet (US2/US3/US4 not implemented until later phases), they resolve to "no limit"
for every download, so this story's downloads are observably identical to having no limiter code
at all.

### Implementation for User Story 1

- [X] T016 [US1] Extend the `GET /api/jobs` response serialization in
  `crates/lanrurugi-api/src/jobs.rs` to include the new `downloaded_bytes`/`total_bytes` fields
  from `JobStatus` (contracts/download-settings-api.md's extended job shape) — omitted-when-absent,
  not null/zero sentinels.
- [X] T017 [US1] Add `downloaded_bytes`/`total_bytes` to the `Job` type in
  `apps/frontend/src/api/types.ts` (both optional).
- [X] T018 [US1] Render a real progress bar for a job with `downloaded_bytes`/`total_bytes`
  present in `apps/frontend/src/pages/Jobs.tsx`: a determinate bar (`downloaded_bytes /
  total_bytes`) when both are present, an indeterminate/spinner indicator when `downloaded_bytes`
  is present but `total_bytes` is absent (spec FR-002/Edge Case), and the existing
  queued/active/finished/failed rendering unchanged for every non-download job or a download job
  before byte transfer has started.
- [X] T019 [US1] Handle the multi-resource combined-progress case: when Phase 4 (US2/US3) isn't
  yet implemented, confirm (per quickstart.md §3) that even a naive "download every `downloads[]`
  entry sequentially, summing bytes into the same job's `downloaded_bytes`/`total_bytes`" already
  satisfies spec FR-003's "single indicator per job, not per-resource" — implemented in
  `crates/lanrurugi-api/src/plugins.rs`'s `download_url()` (T012) by accumulating progress across
  all `downloads[]` entries into one `job_id` rather than one job per entry. No separate code
  change if T012 already did this correctly; this task is the explicit verification/fix-up pass.
- [X] T020 [US1] Confirm failure handling end to end (spec FR-004, quickstart.md §4): a non-2xx
  response or unreachable host during T010's streaming download surfaces as `jobs.fail(...)` with
  a human-readable reason in `crates/lanrurugi-api/src/plugins.rs`, and the staged (partial) file
  from `state.library.temp_dir` is deleted rather than ever reaching `ingest_file`.

**Checkpoint**: User Story 1 fully functional and independently testable — quickstart.md §1–4 all
pass using only Chaika/EHentai, no settings UI or concurrency/rate-limit configuration needed.

---

## Phase 4: User Story 2 - Limit simultaneous downloads per source site (Priority: P2)

**Goal**: No more than a configured number of simultaneous downloads run against a given domain
(exact hostname or wildcard, exact taking precedence), with a plugin's own declared default
applying automatically when the user hasn't overridden it (spec FR-005–FR-008, FR-017).

**Independent Test**: Per quickstart.md §5–6 — configure a low per-domain concurrency limit,
trigger several simultaneous downloads against matching domains, and confirm via `/jobs`/timing
that no more than the configured number transfer bytes at once; confirm exact-hostname-over-
wildcard precedence separately.

### Implementation for User Story 2

- [X] T021 [US2] Wire T008's `acquire_concurrency_permit` into T010's streaming-download function
  in `crates/lanrurugi-api/src/download_manager/mod.rs` — the permit is acquired before the real
  HTTP request starts and held for the whole transfer, so a queued-but-not-yet-admitted download
  correctly shows as `active`-but-`downloaded_bytes`-absent on the Jobs page rather than appearing
  to have silently failed.
- [X] T022 [US2] Implement the `plugin_options` dispatcher method in
  `crates/lanrurugi-plugin/dispatcher/dispatcher.ts`, calling a plugin's exported
  `pluginOptions()` if present (parallel to the existing `plugin_info` handling), returning `null`/
  omitted when the plugin exports no such function (spec FR-015).
- [X] T023 [US2] Add a `plugin_options(&self, namespace: &str) -> Option<PluginOptionsResult>`
  method to `PluginPool` in `crates/lanrurugi-plugin/src/pool.rs`, mirroring the existing
  `plugin_info` call pattern (cheap, zero-extra-permission subprocess call).
- [X] T024 [US2] Implement Redis persistence for `Download Plugin Settings` user overrides (spec
  FR-013 — persisted across future downloads until explicitly changed again): a new
  `save_plugin_options_override`/`get_plugin_options_override` pair in
  `crates/lanrurugi-storage/src/repository.rs` (or a new small module alongside it) under an
  additive Redis key namespace (data-model.md), keyed by plugin `namespace`.
- [X] T025 [US2] Implement `GET /api/plugins/{namespace}/options` in
  `crates/lanrurugi-api/src/plugins.rs` (spec FR-012 — view current effective settings without
  editing a file/using a CLI): calls T023's `plugin_options`, merges with T024's stored override,
  returns the effective-settings shape from `contracts/download-settings-api.md` (including
  per-field `source: "plugin_default"|"user_override"`); `404` when the plugin exports no
  `pluginOptions()` at all.
- [X] T026 [US2] Implement `PUT /api/plugins/{namespace}/options` in
  `crates/lanrurugi-api/src/plugins.rs` (spec FR-006 — user override of a plugin's declared
  concurrency/rate-limit defaults): validates each provided field (FR-014 — positive-integer
  concurrency/rate-limit values only), persists via T024, returns `422` with a field-level message
  on validation failure, `404` under the same condition as `GET`.
- [X] T027 [US2] Implement `DELETE /api/plugins/{namespace}/options` in
  `crates/lanrurugi-api/src/plugins.rs`: clears the stored override via T024 (idempotent no-op
  when none exists), returns the now-all-defaults effective settings.
- [X] T028 [US2] Wire `download_url()` (T012) to resolve the acting plugin's effective
  `domain_rules` (T025's merge logic) **once, at the moment each download starts** — the resolved
  rules (and the concurrency permit/rate-limiter reference obtained from them) MUST be captured as
  a fixed snapshot for that download's entire lifetime, not re-read from the live settings store
  partway through (spec FR-016: a mid-download settings change must not retroactively alter an
  already-in-progress download). Pass this snapshot into T021's concurrency-gated download calls,
  replacing any placeholder/no-limit behavior from Phase 2.
- [X] T028a [US2] Verify FR-016 explicitly: start a download against a domain with a generous
  concurrency/rate limit, change that domain's `max_concurrent`/`max_bytes_per_sec` to a much
  stricter value via `PUT /api/plugins/{namespace}/options` while the download is still
  in-flight, and confirm the already-running download's observed behavior (permit held, transfer
  speed) is unaffected by the change — only a download started *after* the change picks up the
  new limits. Depends on T008's per-domain-pattern Semaphore-replacement behavior and T028's
  snapshot-at-start-time design.

**Checkpoint**: User Stories 1 AND 2 both work independently — quickstart.md §5–6 pass; existing
Chaika/EHentai downloads (US1) are unaffected when no `domain_rules` are configured (FR-008's
automatic default, FR-017's unmanaged-when-nothing-declared fallback); FR-016 verified via T028a.

---

## Phase 5: User Story 3 - Cap download bandwidth usage (Priority: P3)

**Goal**: An optional rate limit (per-domain-rule or general fallback) caps observed download
throughput; with none configured, downloads proceed at full speed exactly as before (spec
FR-009–FR-010).

**Independent Test**: Per quickstart.md §7–8 — set a rate limit, trigger a large download, and
confirm observed throughput stays within 10% of the cap (SC-005); clear it and confirm full-speed
behavior returns.

### Implementation for User Story 3

- [X] T029 [US3] Wire T009's rate limiter into T010's streaming-download function in
  `crates/lanrurugi-api/src/download_manager/mod.rs` — resolve the effective `max_bytes_per_sec`
  for the download's target hostname (reusing T007's `resolve` precedence logic, general fallback
  included per FR-009) before each chunk read, calling `until_n_ready` with that chunk's byte
  count; skip throttling entirely (today's unrestricted-speed behavior) when no rule resolves to a
  rate limit at all (FR-010).
- [X] T030 [US3] Extend T026's `PUT /api/plugins/{namespace}/options` validation to cover
  `max_bytes_per_sec` the same way as `max_concurrent` (positive integer only, `422` on violation)
  — most of this logic is likely already shared with T026 if implemented generically; this task
  is the explicit check/extension pass for the rate-limit field specifically, including the
  general (pattern-less) fallback rule shape.

**Checkpoint**: All three of US1/US2/US3 now independently functional — quickstart.md §1–8 all
pass.

---

## Phase 6: User Story 4 - Configure a download plugin's behavior from the UI (Priority: P2)

**Goal**: A user can view and change a download plugin's concurrency/rate-limit/bundling settings
from the existing Plugins page, with validation and persistence, and sees no settings UI at all
for a plugin that declares nothing configurable (spec FR-011–FR-016, FR-018).

**Independent Test**: Per quickstart.md §9–11 — open a download plugin's settings, change a
value, confirm persistence and effect on the next download; confirm invalid values are rejected;
confirm no settings affordance appears for a plugin with no `pluginOptions()`.

### Implementation for User Story 4

- [X] T031 [P] [US4] Add `PluginOptionsResult`/effective-settings response types to
  `apps/frontend/src/api/types.ts`, mirroring `contracts/download-settings-api.md`.
- [X] T032 [P] [US4] Add `usePluginOptions(namespace)`/`useUpdatePluginOptions(namespace)`/
  `useResetPluginOptions(namespace)` hooks to `apps/frontend/src/api/hooks.ts`, calling T025–T027's
  new endpoints (TanStack Query, matching this project's existing hook conventions).
- [X] T033 [US4] Add a per-plugin "Settings" affordance to each download-plugin card in
  `apps/frontend/src/pages/Plugins.tsx` (the existing per-plugin cards noted in research.md §4 as
  not yet having settings wired in), shown only when `usePluginOptions` resolves a non-404 result
  (FR-015).
- [X] T034 [US4] Implement the settings form itself (new component, e.g.
  `apps/frontend/src/pages/Plugins/PluginOptionsForm.tsx`): renders each `domain_rules` entry
  (pattern, max_concurrent, max_bytes_per_sec, description) and the `bundle_as_archive` toggle
  (only shown when the plugin's result includes it — i.e. only for a multi-resource-capable
  plugin), each field showing whether its current value is `plugin_default` or `user_override`
  (T031's `source` field), with inline validation-error display wired to T032's mutation hooks'
  `422` handling (FR-014).
- [X] T035 [US4] Implement the FR-018/US3-adjacent category-association mechanism: extend
  `download_url()`'s existing `catid` parameter handling (already present per
  `crates/lanrurugi-api/src/plugins.rs`'s current `DownloadUrlParams`) so that when a
  multi-resource download is *not* bundled into one archive (`bundle_as_archive: false`), every
  resulting individually-cataloged archive is added to the same user-selected category — reusing
  the exact category-assignment call `upload.rs::upload_archive` already makes, not a new
  mechanism.
- [X] T036 [US4] Implement the `bundle_as_archive` branch itself in
  `crates/lanrurugi-api/src/download_manager/mod.rs` (or `plugins.rs`'s `download_url()`, wherever
  T012 put the multi-resource orchestration): when true, write every downloaded resource's bytes
  into a single in-memory `zip::ZipWriter<Cursor<Vec<u8>>>` inside `tokio::task::spawn_blocking`
  (research.md §1) before calling `ingest_file` once on the resulting archive; when false, call
  `ingest_file` once per resource, each tagged into T035's shared category.

**Checkpoint**: All four user stories independently functional — quickstart.md's full 12-scenario
suite passes.

---

## Phase 7: Polish & Cross-Cutting Concerns

**Purpose**: Final migration completeness and end-to-end verification across all stories.

- [X] T037 Migrate `plugins/download/pixiv.ts`: remove its
  `Deno.writeFile`/zip-equivalent per-page logic (the `image_res_to_zip`/`add_single_page_artwork_
  to_zip`/`add_multi_page_artwork_to_zip` helpers), instead returning `downloads: [{url, headers:
  {Referer: referer}}, ...]` for every page URL its existing metadata-fetching logic already
  discovers, and add an exported `pluginOptions()` declaring `bundle_as_archive: { default: true,
  description: "..." }` (contracts/plugin-download-protocol.md) — this is the largest single
  migration task in this feature and depends on the full Phase 2/4/6 pipeline (T012, T028, T036)
  already existing to download/bundle multiple resources correctly.
- [X] T038 Run `deno check` against the migrated `plugins/download/pixiv.ts` and confirm it's
  clean (matching this project's established plugin-conversion quality bar).
- [X] T039 [P] Run `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all
  --check` across all Rust changes from this feature.
- [X] T040 Execute all 12 scenarios in `quickstart.md` end to end against a real running
  `lanrurugi-server` with all three migrated plugins installed, confirming SC-001–SC-005.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: No dependencies — can start immediately.
- **Foundational (Phase 2)**: Depends on Setup completion — BLOCKS all user stories (T012's
  `download_url()` rewrite and at least one migrated plugin are needed before any story's
  acceptance scenarios can be demonstrated at all).
- **User Story 1 (Phase 3)**: Depends on Foundational completion only.
- **User Story 2 (Phase 4)**: Depends on Foundational completion; independent of US1's own tasks
  (different files: Jobs.tsx/jobs.rs vs. download_manager's concurrency gate/plugin-options
  endpoints), though T028 assumes T012's `download_url()` shape from Foundational.
- **User Story 3 (Phase 5)**: Depends on Foundational completion and reuses US2's `domain_rules`
  resolution (T007) and plugin-options endpoints (T025–T027) — genuinely independent of US2's own
  concurrency-gate code path, but shares the settings-persistence plumbing, so implementing US2
  first is the practical order even though the spec frames them as independently testable.
- **User Story 4 (Phase 6)**: Depends on US2's plugin-options endpoints (T025–T027) existing —
  it's a UI layer on top of them, not independent infrastructure.
- **Polish (Phase 7)**: Depends on Phase 2/4/6 (T012, T028, T036) for Pixiv's migration (T037) to
  have somewhere correct to send its multi-resource `downloads[]` result.

### Within Each Phase

- Foundational: T001–T004 (setup) → T005–T009 (parallel-safe additions) → T010 (needs T006/T008/
  T009) → T011 (needs T010) → T012 (needs T005, T011) → T013/T014 (parallel, only need T005) → T015
  (needs everything above).
- User Story 2: T021 (needs T008) → T022/T023 (parallel) → T024 → T025/T026/T027 (need T023, T024)
  → T028 (needs T012, T025) → T028a (needs T028, and T008's Semaphore-replacement behavior).
- User Story 4: T031/T032 (parallel) → T033 (needs T032) → T034 (needs T032, T033) → T035 (needs
  T028) → T036 (needs T012, T028).

### Parallel Opportunities

- T002, T003 (Setup) — different `Cargo.toml` dependency additions.
- T005, T006 (Foundational) — SDK types vs. `JobStatus` fields, no shared files.
- T013, T014 (Foundational) — different plugin files.
- T022, T023 (US2) — dispatcher vs. pool, different files, both needed before T024.
- T031, T032 (US4) — types vs. hooks, different files.
- T039 (Polish) — independent of T037/T038/T040's plugin-specific verification.

---

## Parallel Example: Foundational Phase

```bash
# Launch independent Foundational additions together once T001-T004 (Setup) are done:
Task: "Extend DownloadResult/PluginOptionsResult types in crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts"
Task: "Add downloaded_bytes/total_bytes to JobStatus in crates/lanrurugi-core/src/jobs.rs"

# Once T005 lands, migrate the two trivial plugins in parallel:
Task: "Migrate plugins/download/chaika.ts to the downloads[] contract"
Task: "Migrate plugins/download/ehentai.ts to the downloads[] contract"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 1: Setup.
2. Complete Phase 2: Foundational (CRITICAL — includes the real Rust-side streaming download and
   two migrated plugins; blocks every story).
3. Complete Phase 3: User Story 1.
4. **STOP and VALIDATE**: run quickstart.md §1–4 against Chaika/EHentai.
5. Deploy/demo if ready — real progress bars ship with zero settings UI, zero concurrency/rate-
   limit configuration, and zero Pixiv migration required yet.

### Incremental Delivery

1. Setup + Foundational → real single-file downloads work, progress-tracked, no limits.
2. Add User Story 1 → Jobs page shows real progress → validate independently → demo (MVP).
3. Add User Story 2 → per-domain concurrency limiting + the settings-storage/endpoint plumbing
   later stories build on → validate independently → demo.
4. Add User Story 3 → rate limiting (reuses US2's plumbing) → validate independently → demo.
5. Add User Story 4 → settings UI surfaces US2/US3's configuration → validate independently → demo.
6. Polish → migrate Pixiv onto the now-complete pipeline (the one plugin that actually needs
   multi-resource bundling), full quickstart.md pass.

### Suggested Team Split

With multiple developers, after Foundational completes:
- Developer A: User Story 1 (frontend-heavy: Jobs.tsx progress bar + jobs.rs serialization).
- Developer B: User Story 2 → User Story 3 (backend-heavy: download_manager's concurrency/
  rate-limit gates + plugin-options CRUD endpoints — natural sequential pairing since US3 reuses
  US2's persistence plumbing).
- Developer C: User Story 4, starting once Developer B's T025–T027 endpoints exist.
- Pixiv's migration (T037) is a natural final task for whoever finishes first, since it's the one
  piece that exercises the *entire* pipeline (multi-resource downloads, per-domain headers,
  bundling) end to end.

---

## Notes

- [P] tasks touch different files with no completed-task dependency between them.
- [Story] labels map each task to spec.md's US1–US4 for traceability; Foundational/Setup/Polish
  tasks carry no story label per the established task-format convention.
- Every task cites its exact file path(s) per this project's existing 001/002/003 task-list
  convention.
- No test-task phase per story (see **Tests** note above) — verification is quickstart.md's
  runnable scenarios plus the project's standing quality gates (`cargo test`, `cargo clippy -D
  warnings`, `cargo fmt --check`, `deno check`, lefthook pre-commit/pre-push).
- Commit after each task or logical group, matching this project's established workflow.
- Stop at any Checkpoint to validate a story independently before continuing.
