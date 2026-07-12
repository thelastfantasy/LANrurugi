<!-- SPECKIT START -->
Three feature specs exist; implementation has not started on 002 or 003 yet.

**Phase 1 — `001-lanrurugi-full-rewrite`** (build this first): plan at
`specs/001-lanrurugi-full-rewrite/plan.md`. User Stories 1–8 — library continuity, non-merging
ingestion, third-party API compatibility, plugin metadata enrichment, backup/export, duplicate
repair, UI localization, concurrency benchmarking vs. the legacy Perl system. Design artifacts:
`specs/001-lanrurugi-full-rewrite/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 1 addendum — `002-job-console`** (additive to Phase 1, planned but not yet implemented):
plan at `specs/002-job-console/plan.md`. Background job management console (a
Minion-admin-console equivalent) surfacing the existing in-process job registry
(`lanrurugi_core::jobs`) — list/monitor/inspect/clear queued, active, finished, and failed jobs
(thumbnail regen, backup/restore, rescans, duplicate scans, index rebuilds) via new additive
`/api/jobs*` endpoints, deliberately separate from the legacy-mimicking `/api/minion/*` contract.
Retry is explicitly out of scope. Design artifacts:
`specs/002-job-console/{research.md,data-model.md,contracts/,quickstart.md}` (no `tasks.md` yet).

**Phase 2 — `003-ocr-manga-translation`** (depends on Phase 1; independent plan, must not block
or be blocked by it — constitution Principle VI): plan at
`specs/003-ocr-manga-translation/plan.md`. On-page manga translation — OCR detection/merging,
user-selectable translation backend (cloud, proxied server-side, vs. locally-hosted, browser-
originated), volume-level font-matching cache, sliding-window prefetch with cost-aware budgeting.
Design artifacts:
`specs/003-ocr-manga-translation/{research.md,data-model.md,contracts/,quickstart.md}` (no
`tasks.md` yet).

Stack: Rust (Tokio/Axum/Rayon) backend as a Cargo workspace under `crates/` producing one binary
(`lanrurugi-server`, with `serve`/`rebuild-index`/`bench` subcommands), Redis reused as-is from
the legacy deployment, React 19 + TypeScript + Vite + Tailwind + Zustand + TanStack Query
frontend under `apps/frontend/`, Deno-subprocess plugin runtime, `crates/lanrurugi-bench/` for the
cross-system performance comparison harness. Phase 2 adds `crates/lanrurugi-ocr`,
`crates/lanrurugi-fontcache`, `crates/lanrurugi-translate`, and
`apps/frontend/src/translation/` to that same workspace/app — not a separate deployable. Governing
rules: `.specify/memory/constitution.md`.

For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan for whichever phase you
are working on.
<!-- SPECKIT END -->

## Language

始终使用中文回答（Always respond in Chinese）。
