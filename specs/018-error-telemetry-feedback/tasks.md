---
description: "Task list for feature 018-error-telemetry-feedback"
---

# Tasks: Error Telemetry and Feedback Bundles

**Input**: Design documents from `specs/018-error-telemetry-feedback/`

**Prerequisites**: [plan.md](./plan.md), [spec.md](./spec.md), [research.md](./research.md),
[data-model.md](./data-model.md), [contracts/diagnostics-api.md](./contracts/diagnostics-api.md),
[quickstart.md](./quickstart.md)

**Tests**: included where they prove a requirement cheaply (stores, scrubbing, fingerprints, window
expiry, cap handling). No TDD requirement; write them with the code they cover.

**Organization**: by user story, so each story is independently deployable. US1 + US2 are the MVP.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: different files, no dependency on an incomplete task
- Paths are repository-relative

## Path Conventions

Rust workspace under `crates/`, SPA under `apps/frontend/`, docs under `docs/`.

---

## Phase 1: Setup (Shared Infrastructure)

- [ ] T001 Verify the current stable versions of `sentry`, `sentry-tower`, `sentry-tracing`,
      `sentry-panic`, `sourcemap` (crates.io) and `@sentry/react` (npm) and add the Rust ones to
      `Cargo.toml` `[workspace.dependencies]` plus `crates/lanrurugi-api/Cargo.toml`
- [ ] T002 [P] Add `@sentry/react` to `apps/frontend/package.json` and refresh `pnpm-lock.yaml` from the
      repo root with `pnpm install`; note in the task thread that a dev image rebuild **and**
      `podman volume rm lanrurugi_lrr-dev-frontend-node-modules` are required afterwards
- [ ] T003 [P] Create `crates/lanrurugi-api/src/diagnostics_settings.rs` registering every field from
      `data-model.md`'s `DiagnosticsSettings`, following `crates/lanrurugi-api/src/share_settings.rs`
- [ ] T004 [P] Create the module skeleton `crates/lanrurugi-api/src/diagnostics/{mod,routes,store,ingest,
      issue,bundle,text,forward,logs,symbolicate}.rs` and mount `diagnostics::router()` in
      `crates/lanrurugi-api/src/lib.rs`
- [ ] T005 [P] Add `p, token_admin, /api/diagnostics/*, *, deny` and the matching `guest_visitor` deny to
      `crates/lanrurugi-api/policy/route_policy.csv` (session-only, never token or guest)
- [ ] T006 [P] Add `__GIT_SHA__` to `apps/frontend/vite.config.ts` `define`, fed by `LANRURUGI_GIT_SHA`
      with a `dev` fallback, and surface it in the existing version dialog
- [ ] T007 [P] Add the empty settings section `apps/frontend/src/diagnostics/DiagnosticsSection.tsx` and
      mount it in `apps/frontend/src/pages/Settings/SettingsPage.tsx` after `<SharingSection />`

---

## Phase 2: Foundational (Blocking Prerequisites)

**⚠️ CRITICAL**: no user story work until this phase is complete.

- [ ] T008 Create `crates/lanrurugi-storage/src/diagnostics.rs` with the newtype ids
      (`DiagnosticEventId`, `DiagnosticIssueId`) and the `DiagnosticEvent`/`DiagnosticIssue`/
      `Breadcrumb` entities exactly as `data-model.md` defines them
- [ ] T009 Implement the repository in `crates/lanrurugi-storage/src/diagnostics.rs`: insert event +
      issue upsert, get by id, list by fingerprint/range/level, range delete, byte counter, and the
      three-step eviction (age → count → bytes) from `data-model.md`
- [ ] T010 Add `#[cfg(test)]` coverage in `crates/lanrurugi-storage/src/diagnostics.rs` for eviction
      order (oldest first), the byte counter staying consistent, and an issue surviving its events
- [ ] T011 Implement envelope decoding and projection in
      `crates/lanrurugi-api/src/diagnostics/ingest.rs` (header line, item headers, event item,
      breadcrumbs, tags, request id; unknown items preserved verbatim)
- [ ] T012 Implement the scrubbing pass in `crates/lanrurugi-api/src/diagnostics/scrub.rs` (share codes
      both forms, `?r=`, `X-Share-Token`, `rt=`, authorization/cookie, `api_key|token|secret`, POSIX and
      Windows absolute paths, long hex) with unit tests for each rule
- [ ] T013 Implement fingerprinting and message normalization in
      `crates/lanrurugi-api/src/diagnostics/issue.rs` (event-supplied fingerprint wins; else
      type + top user frame + normalized message) with unit tests for normalization stability
- [ ] T014 Implement the timestamped log-line reader in `crates/lanrurugi-api/src/diagnostics/logs.rs`
      (RFC3339 prefix parsing, continuation lines attached to the entry above, category selection,
      range slicing) with unit tests for a multi-line stack
- [ ] T015 Implement settings load/save plus the runtime switches in
      `crates/lanrurugi-api/src/diagnostics/mod.rs`, reading `diagnostics_settings.rs`'s fields with the
      documented defaults
- [ ] T016 Implement the usage/drops projection (events, issues, bytes, `retention`/`rate_limit`/
      `too_large`/`store_error`) in `crates/lanrurugi-api/src/diagnostics/store.rs`
- [ ] T017 Implement `GET /api/diagnostics/config`, `GET/PUT /api/diagnostics/settings` and the usage
      payload in `crates/lanrurugi-api/src/diagnostics/routes.rs`, mounted from `app.rs`'s API router
- [ ] T018 Implement the single background writer task (bounded `mpsc` of envelopes, batched Redis
      pipeline writes, bounded queue that drops-with-a-counter when full) in
      `crates/lanrurugi-api/src/diagnostics/mod.rs`, started from
      `crates/lanrurugi-server/src/main.rs` and flushed on shutdown
- [ ] T019 Add the per-instance ingest rate limiter (`governor`, 60/min and 1,000/h) in
      `crates/lanrurugi-api/src/diagnostics/ingest.rs`, counting rejections in `drops.rate_limit`
- [ ] T020 Implement `POST /api/diagnostics/envelope` in `crates/lanrurugi-api/src/diagnostics/routes.rs`
      with the status-code contract from `contracts/diagnostics-api.md` (200/204/401/413, gzip accepted,
      `X-Sentry-Auth` ignored)
- [ ] T021 Implement `apps/frontend/src/diagnostics/sdk.ts`: init only when `/config` says enabled, DSN
      from `/config`, release from `__GIT_SHA__`, `beforeSend` size cap, breadcrumbs for navigation /
      interaction / request / state, no form values
- [ ] T022 [P] Mount the SDK init in `apps/frontend/src/main.tsx` and forward
      `ErrorBoundary.componentDidCatch` captures through `apps/frontend/src/diagnostics/sdk.ts`
- [ ] T023 [P] Add the `diagnostics.*` i18n keys (skeleton, filled per story) to
      `apps/frontend/src/i18n/locales/{zh,en,ja}.json`
- [ ] T024 [P] Add `apps/frontend/src/diagnostics/api.ts` query hooks for issues, events, settings,
      window and bundle endpoints (TanStack Query, following `apps/frontend/src/share/api.ts`)

**Checkpoint**: store, scrubbing, settings, ingest and client wiring exist; stories can start.

---

## Phase 3: User Story 1 — capture and read locally (Priority: P1) 🎯 MVP

**Goal**: failures from both sides appear in the settings page within seconds, grouped and readable, on
an instance with no network.

**Independent Test**: with outbound networking disabled, provoke a client and a server error and read
both in the settings page; repeat one error and watch the count grow.

- [ ] T025 [US1] Initialise `sentry` in `crates/lanrurugi-server/src/main.rs` with an in-process
      transport that writes into the diagnostics store, `sentry-panic` enabled, `sentry-tracing`
      filtered to error level, and the release from `LANRURUGI_GIT_SHA`
- [ ] T026 [US1] Apply `sentry-tower`'s axum layer in `crates/lanrurugi-server/src/app.rs` so a 5xx and
      its request context become a captured event
- [ ] T027 [US1] Add the request-identity middleware (short `req=` id written to the request's log
      lines, returned as `X-Request-Id`, included in error JSON bodies) in
      `crates/lanrurugi-api/src/common.rs`-adjacent middleware
- [ ] T028 [US1] Implement `GET /api/diagnostics/issues` and `GET /api/diagnostics/issues/{fingerprint}`
      in `crates/lanrurugi-api/src/diagnostics/routes.rs` with the filters and projections from the
      contract, including the `log_excerpt` matched by request id
- [ ] T029 [US1] Implement `POST /api/diagnostics/issues/{fingerprint}/state` (open/resolved/ignored,
      idempotent) and reopen-on-new-event in `crates/lanrurugi-api/src/diagnostics/issue.rs`
- [ ] T030 [US1] Implement `GET /api/diagnostics/events` (merged timeline of events and log entries) in
      `crates/lanrurugi-api/src/diagnostics/routes.rs`, reusing `logs.rs` for the log side
- [ ] T031 [US1] Implement `apps/frontend/src/diagnostics/IssuesModal.tsx` list: level chips
      (info/warning/error), timestamps, filters (level/time/source/release/free text), sort (recent /
      frequent), multi-select with select-all / invert / "only errors" and a visible selection count
- [ ] T032 [US1] Implement the issue detail view inside `IssuesModal.tsx`: stack, breadcrumbs timeline,
      tags, request id, log excerpt, event list
- [ ] T033 [US1] Wire resolve/ignore and the drop/usage counters into
      `apps/frontend/src/diagnostics/DiagnosticsSection.tsx`
- [ ] T034 [P] [US1] Add Rust tests in `crates/lanrurugi-api/src/diagnostics/issue.rs` for grouping
      stability, count aggregation and reopening a resolved issue
- [ ] T035 [P] [US1] Add `vitest` tests for the filter/selection reducer in
      `apps/frontend/tests/unit/diagnosticsSelection.test.ts`
- [ ] T036 [US1] Add `crates/lanrurugi-server/tests/diagnostics_capture.rs`: post a real envelope as a
      session, then assert the issue list, the count after a second identical post, and that a guest is
      refused by policy

**Checkpoint**: US1 is deployable on its own — the intranet diagnostics surface works.

---

## Phase 4: User Story 2 — feedback bundle and bilingual text (Priority: P1)

**Goal**: select a range, preview, download a scrubbed zip, copy an issue text in Chinese or English.

**Independent Test**: generate a bundle for a one-hour window, unzip it, verify the files, the scrubbed
content and both language texts.

- [ ] T037 [US2] Implement `SelectionSpec` parsing/validation and freezing in
      `crates/lanrurugi-api/src/diagnostics/bundle.rs` (rejects an empty selection and a range longer
      than the retention)
- [ ] T038 [US2] Implement `POST /api/diagnostics/bundles/preview` in
      `crates/lanrurugi-api/src/diagnostics/routes.rs`: per-file entry counts and byte estimates,
      `over_cap`, and the truncations a fitting bundle would need
- [ ] T039 [US2] Implement the streamed zip writer in `crates/lanrurugi-api/src/diagnostics/bundle.rs`
      following `crates/lanrurugi-api/src/tankoubon_zip.rs`: `manifest.json`, `README.txt`,
      `server/<category>.log`, `frontend/events.jsonl`, `frontend/env.json`, `diagnostics.json`,
      optional `config.redacted.json`
- [ ] T040 [US2] Implement the bilingual text generator in
      `crates/lanrurugi-api/src/diagnostics/text.rs` (title + body in Chinese and English, from the
      frozen selection: version, release, range and counts, top fingerprint, request id, included
      categories, truncations) with unit tests
- [ ] T041 [US2] Implement `POST /api/diagnostics/bundles/text` in
      `crates/lanrurugi-api/src/diagnostics/routes.rs`
- [ ] T042 [US2] Implement `POST /api/diagnostics/bundles` in
      `crates/lanrurugi-api/src/diagnostics/routes.rs` with the 20 MB cap, the `truncate: true` path and
      the `Content-Disposition` filename
- [ ] T043 [US2] Implement `apps/frontend/src/diagnostics/BundleModal.tsx`: category toggles, preview
      file list and size, generate/download, progress for slow bundles
- [ ] T044 [US2] Implement the text panel in `BundleModal.tsx`: both languages, editable title, copy
      either in one action
- [ ] T045 [P] [US2] Add Rust tests in `crates/lanrurugi-api/src/diagnostics/bundle.rs` for the cap and
      the truncation report
- [ ] T046 [US2] Add `crates/lanrurugi-server/tests/diagnostics_bundle.rs`: generate a bundle from a
      seeded store and assert the entry list, the manifest's frozen selection, and **no** share code,
      token, path or archive title inside any entry

**Checkpoint**: MVP complete — local diagnostics plus a shareable report.

---

## Phase 5: User Story 3 — time-boxed forwarding (Priority: P2)

**Goal**: point at a self-hosted tracker and forward only inside a window that cannot outlive itself.

**Independent Test**: open a 5-minute window, forward two errors, stop it, and prove the next events stay
local over two minutes.

- [ ] T047 [US3] Add tracker DSN parsing and storage to `crates/lanrurugi-api/src/diagnostics/forward.rs`
      (validated, never returned by the settings route, clearing means empty)
- [ ] T048 [US3] Implement the window state machine and the send path in
      `crates/lanrurugi-api/src/diagnostics/forward.rs`: deadline checked before every send, envelope
      POSTed with `X-Sentry-Auth` rebuilt from the DSN, sent/failed counters and `last_error`
- [ ] T049 [US3] Add `GET/POST/DELETE /api/diagnostics/window` in
      `crates/lanrurugi-api/src/diagnostics/routes.rs` (only 300/3600/86400 accepted; 409 without a
      tracker)
- [ ] T050 [US3] Feed the forwarder from the writer task in
      `crates/lanrurugi-api/src/diagnostics/mod.rs` so forwarding never adds latency to a request
- [ ] T051 [US3] Implement the forwarding panel in
      `apps/frontend/src/diagnostics/DiagnosticsSection.tsx`: DSN input, duration select, countdown,
      stop button, sent/failed/last error, and re-read the deadline on mount
- [ ] T052 [US3] Add `crates/lanrurugi-server/tests/diagnostics_forward.rs` with a small axum fake
      tracker: events arrive while open; zero arrive after stop and after expiry; an unreachable
      endpoint leaves local copies intact and records the failure
- [ ] T053 [P] [US3] Add a Rust unit test in `crates/lanrurugi-api/src/diagnostics/forward.rs` that a
      restart does not extend an expired deadline
- [ ] T054 [P] [US3] Add `vitest` coverage for the countdown (deadline in the past renders closed, no
      restart on remount) in `apps/frontend/tests/unit/diagnosticsWindow.test.tsx`

**Checkpoint**: remote forwarding works, offline behaviour unchanged.

---

## Phase 6: User Story 4 — release identity and symbolication (Priority: P2)

**Goal**: a client stack resolves to file/line/function when that release's maps are available, and
otherwise still names the release.

**Independent Test**: report an error from a built release, read a resolved stack; remove the maps and
re-read the raw one.

- [ ] T055 [US4] Switch the production build to hidden source maps in `apps/frontend/vite.config.ts` and
      copy them into a non-served artifact directory in `Dockerfile` keyed by `LANRURUGI_GIT_SHA`
- [ ] T056 [US4] Assert in `crates/lanrurugi-server/tests/diagnostics_capture.rs` (or a static check)
      that the served `dist` contains no `.js.map` and that `/sourcemaps/**` 404s
- [ ] T057 [US4] Implement frame resolution in `crates/lanrurugi-api/src/diagnostics/symbolicate.rs`
      using the `sourcemap` crate: release → directory, per-chunk lazy load and cache, frame rewrite,
      `sourcesContent` never serialized out
- [ ] T058 [US4] Call the resolver from the issue detail projection and from the bundle writer in
      `crates/lanrurugi-api/src/diagnostics/{issue,bundle}.rs`, degrading to raw frames when the map is
      missing
- [ ] T059 [P] [US4] Add Rust tests in `crates/lanrurugi-api/src/diagnostics/symbolicate.rs` with a
      fixture map (resolved frame) and with a missing map (raw frame + release kept)
- [ ] T060 [US4] Document how to resolve a raw frame by release in
      `docs/src/content/docs/guide/troubleshooting.md` and its `en/` counterpart

**Checkpoint**: reports are readable across releases without shipping maps to users.

---

## Phase 7: User Story 5 — offline replay (Priority: P3)

- [ ] T061 [US5] Include `envelopes.ndjson.gz` in the bundle writer in
      `crates/lanrurugi-api/src/diagnostics/bundle.rs`
- [ ] T062 [US5] Implement `POST /api/diagnostics/replay` in
      `crates/lanrurugi-api/src/diagnostics/routes.rs` (409 without a tracker; reports sent/failed)
- [ ] T063 [P] [US5] Add a test in `crates/lanrurugi-server/tests/diagnostics_forward.rs` that a
      replay delivers the stored envelopes once and that a second replay does not double-count

**Checkpoint**: all five stories delivered.

---

## Phase 8: Polish & Cross-Cutting Concerns

- [ ] T064 [P] Remove the `/logs` entry from `apps/frontend/src/Layout.tsx` and link it from the
      diagnostics section as "raw log output"
- [ ] T065 [P] Add `docs/api/sidecar/diagnostics.yaml` for the new routes and confirm the docgen gates
      pass (`mise run api-docs` / the docs build)
- [ ] T066 [P] Write the user guide page (`docs/src/content/docs/guide/troubleshooting.md` + `en/`) and
      the README bullet
- [ ] T067 [P] Add the Playwright flow in `apps/frontend/tests/e2e/diagnostics.spec.ts`: issue list,
      select, preview, download
- [ ] T068 Run the seven `quickstart.md` scenarios on the dev instance (3000/3100), recording results
- [ ] T069 Run `mise run clippy`, `mise run check`, `mise run test-crate -- lanrurugi-api
      lanrurugi-storage lanrurugi-server`, and the frontend `tsc`/`eslint`/`vitest` gates in
      `apps/frontend`
- [ ] T070 Rebuild the release binary through `scripts/cargo-container-run.sh`, swap it into both dev
      instances, then re-run the capture and forwarding scenarios from
      `specs/018-error-telemetry-feedback/quickstart.md`

---

## Dependencies & Execution Order

- Setup (T001–T007) → Foundational (T008–T024) → stories.
- US1 depends on the store, scrub, fingerprint, ingest, settings and client bootstrap.
- US2 depends on US1's stored events and on `logs.rs`; it does not depend on forwarding.
- US3 depends on the writer task and the stored envelopes; independent of US2.
- US4 depends on US1's issue detail and US2's bundle writer for the two call sites.
- US5 depends on US3's send path plus US2's bundle writer.
- Polish last.

## Parallel Opportunities

- T002–T007 are separate files and can run together.
- T008–T024 have pairwise-independent files; T021/T022/T023/T024 are client-side and can run parallel
  with T011–T020.
- Within US1, T034/T035 run parallel with the implementation tasks.
- US3 and US4 can run in parallel once US1 lands; US5 is small and follows US3.

## Implementation Strategy

1. Setup + Foundational → the store, scrubber and ingest exist behind a disabled feature switch.
2. US1 → deployable intranet diagnostics (MVP half).
3. US2 → the feedback report (MVP complete; this is the user-visible replacement for `/logs`).
4. US3 → optional forwarding.
5. US4, US5, then polish and full validation.

## Implementation Record

*(Filled during implementation: what was verified, what was not, and every departure from this plan.)*
