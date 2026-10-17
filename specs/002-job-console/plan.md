# Implementation Plan: Background Job Console

**Branch**: `002-job-console` | **Date**: 2026-07-07 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/002-job-console/spec.md`

## Summary

Add a Minion-admin-console equivalent that surfaces the existing in-process job registry
(`lanrurugi_core::jobs::JobRegistry`, already used internally by backup/restore, thumbnail
regeneration, duplicate scans, index rebuilds, plugin execution, and URL-download ingestion — but
not archive rescans, which are synchronous today, see spec.md FR-005) as a real, browsable admin
UI: list every tracked
job with live state/progress, inspect a job's result or error, clear finished/failed entries, and
filter/search/paginate the list once it gets long (US4 — per-state counts, state filter, name
search, pagination, all derived client-side from the same `GET /jobs` response, no new backend
endpoint). No new persisted data, no new job-tracking mechanism — this is a visibility/management
layer over what Phase 1 already tracks internally but never exposed as a list. Deliberately does
not reproduce legacy Minion Admin's "Workers"/"Locks" stats — see spec.md Assumptions for why
those are architecturally inapplicable, not an oversight. Additive to Phase 1, does not touch
Phase 2.

## Technical Context

**Language/Version**: Rust (same pinned toolchain as Phase 1, via `mise`) for the backend;
TypeScript (strict mode) for the frontend — no new language/version introduced.

**Primary Dependencies**: None new. Backend reuses the existing `lanrurugi-core` job registry and
`axum`/`tokio` already in the workspace. Frontend reuses the existing TanStack Query + `react-i18n`
+ Tailwind stack and the `.stdbtn`/`.collapsible`/`itg` legacy-CSS classnames already established
by the Phase 1 UI-restoration work (`apps/frontend/src/theme.ts`, `public/legacy/*.css`).

**Storage**: None (in-memory only, per spec Assumptions — no Redis schema addition).

**Testing**: `cargo test` for `JobRegistry`'s new `list_all`/`clear`/`clear_finished`/eviction
behavior (unit tests in `crates/lanrurugi-core/src/jobs.rs`) and the new `crates/lanrurugi-api`
route handlers; a frontend test for the new page consistent with existing conventions.

**Target Platform**: Same as Phase 1 (Linux server, Debian-slim Docker image) — no new deployment
target.

**Project Type**: Web application — adds to the existing single Rust binary + React SPA, no new
deployable.

**Performance Goals**: SC-003 — console stays responsive after hundreds of accumulated jobs in a
single uptime, achieved via the FR-006 retention bound (research.md §2), not a performance
optimization of the read path itself (an in-memory `RwLock<HashMap>` read is already fast at this
scale).

**Constraints**: SC-001 (job state determinable within 5 seconds of opening the console — met via
polling interval choice, research.md §3); FR-008 (same auth as the rest of the admin UI, no
unauthenticated `/minion`-style bypass).

**Scale/Scope**: Single-owner, single-instance deployment (same as Phase 1). 4 user stories, 12
functional requirements, 5 success criteria — a small, additive feature entirely within Phase 1's
existing architecture.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Legacy Data & User-Trust Compatibility | Plan must not require a destructive migration or alter legacy-readable data | **PASS** — no Redis schema touched; job records are in-memory only, nothing legacy reads or writes them |
| II. API Contract Fidelity (Phase 1) | Plan must not alter existing `/api/minion/*` endpoints' shape/behavior | **PASS** — research.md §1: new endpoints (`/api/jobs`, `/api/jobs/{id}`) are additive, deliberately separate from the legacy-mimicking `/api/minion/*` contract, which is left untouched |
| III. Resource-Conscious, Genuinely Concurrent Architecture | Plan must not reintroduce a separate process or block the Tokio runtime | **PASS** — extends the existing in-process `JobRegistry` (already inside the single `lanrurugi-server` binary); list/clear operations are simple in-memory `RwLock` reads/writes, no blocking work |
| IV. Sandboxed, Language-Agnostic Plugin Extensibility | N/A — feature doesn't touch the plugin runtime | **PASS (N/A)** |
| V. Secrets & Network Trust Boundaries | N/A — feature doesn't touch Phase 2/LLM/network trust concerns | **PASS (N/A)** |
| VI. Phased Scope Discipline | Plan must stay within Phase 1 scope, not introduce Phase 2 concepts | **PASS** — explicitly scoped as a Phase 1 addendum (spec Input); no OCR/translation/font-cache concept appears anywhere in this plan |

No violations requiring justification — **Complexity Tracking is empty.**

**Post-design re-check** (after research.md, data-model.md, contracts/, quickstart.md): still
PASS across all principles — data-model.md adds no persisted entity (Principle I);
`contracts/jobs-api.md` is additive-only and explicitly documents why it does not reuse or modify
`/api/minion/*` (Principle II); the extended `JobRegistry` remains a plain in-process, `RwLock`-
guarded structure inside the existing single binary with no new blocking work on the async runtime
(Principle III); nothing here touches plugins, Phase 2 LLM backends, or network trust boundaries
(IV/V, both N/A); no Phase 2 concept was introduced (VI). No new Complexity Tracking entries.

## Project Structure

### Documentation (this feature)

```text
specs/002-job-console/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md         # Phase 1 output (/speckit-plan command)
├── quickstart.md         # Phase 1 output (/speckit-plan command)
├── contracts/            # Phase 1 output (/speckit-plan command)
│   └── jobs-api.md
└── tasks.md              # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
# Adds to the existing Phase 1 Cargo workspace + apps/frontend app — no new crate, no new app.

crates/
├── lanrurugi-core/
│   └── src/jobs.rs        # Extended: order index, MAX_TRACKED_JOBS eviction, list_all(),
│                          # clear(id), clear_finished() — see data-model.md
│                          # tests/ (or inline #[cfg(test)]): eviction + clear behavior
└── lanrurugi-api/
    └── src/jobs.rs         # Extended: new router entries GET /jobs, DELETE /jobs/{id},
                            # DELETE /jobs?state=finished — existing /minion/* handlers untouched
                            # (see contracts/jobs-api.md)

apps/frontend/
└── src/
    ├── api/
    │   ├── types.ts        # + JobRecord interface (native shape, distinct from existing
    │   │                   #   legacy-mimicking JobStatus)
    │   └── hooks.ts         # + useJobs(), useClearJob(id), useClearFinishedJobs()
    └── pages/
        └── Jobs.tsx          # New page: job list (state/progress), per-job detail
                              # (result/error), clear actions, per-state counts/filter/search/
                              # pagination (US4 — all derived client-side from the single
                              # `useJobs()` array already fetched, no extra request) — reuses
                              # `.itg`/`.stdbtn`/`CollapsibleSection` conventions already
                              # established by the Phase 1 UI-restoration work (theme.ts,
                              # Stats.tsx/Plugins.tsx)
```

Also touches (not new files, existing files edited):
- `apps/frontend/src/App.tsx` — add `<Route path="/jobs" element={<Jobs />} />` inside the
  existing `<Layout>` route group.
- `apps/frontend/src/pages/Settings/Settings.tsx` — the existing "Open Minion Console" button
  (Background Workers section) changes from its current stub `onClick` to `navigate('/jobs')`.

**Structure Decision**: No new crate and no new frontend app/package — this feature is small
enough, and tied closely enough to Phase 1's existing job-tracking code, that it's implemented as
extensions to the two crates that already own job state (`lanrurugi-core`) and job HTTP exposure
(`lanrurugi-api`), plus one new page in the existing `apps/frontend` app, matching constitution's
"every phase adds to the same workspace/app rather than creating a new one" constraint.

## Complexity Tracking

*No entries — Constitution Check produced no violations requiring justification.*
