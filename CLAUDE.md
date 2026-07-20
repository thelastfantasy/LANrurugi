<!-- SPECKIT START -->
Five feature specs exist. Phase 1 (001) is implemented; 002, 003, 004, and 005 are planned but not
yet implemented.

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

**Phase 1 addendum — `003-ui-test-automation`** (additive to Phase 1, planned but not yet
implemented): plan at `specs/003-ui-test-automation/plan.md`. Two-layer automated frontend test
coverage the Phase 1 plan called for but never implemented — Vitest + React Testing Library for
fast unit-level logic (reader settings/navigation hooks, cross-archive navigation resolution,
metadata formatting/decoding helpers), and Playwright (Chromium + Firefox) for end-to-end coverage
of key user journeys, reproducing specific defects already found and fixed once through manual
QA (category pinned-field save failure, upload body-size limit, archive-delete orphaned
search-index entries, reader icon-spacing/dead-whitespace layout bugs), plus systematic fixture-
archive coverage across every format `lanrurugi-scanner` supports and higher-risk shapes
(multi-volume, encrypted, non-ASCII filenames). Both layers run automatically in CI. Design
artifacts: `specs/003-ui-test-automation/{research.md,data-model.md,quickstart.md,tasks.md}` (no
`contracts/` — this feature adds no new external interface).

**Phase 2 — `004-ocr-manga-translation`** (depends on Phase 1; independent plan, must not block
or be blocked by it — constitution Principle VI): plan at
`specs/004-ocr-manga-translation/plan.md`. On-page manga translation — OCR detection/merging,
user-selectable translation backend (cloud, proxied server-side, vs. locally-hosted, browser-
originated), volume-level font-matching cache, sliding-window prefetch with cost-aware budgeting.
Design artifacts:
`specs/004-ocr-manga-translation/{research.md,data-model.md,contracts/,quickstart.md}` (no
`tasks.md` yet).

**Phase 1 addendum — `005-download-plugin-progress`** (additive to Phase 1, planned but not yet
implemented): plan at `specs/005-download-plugin-progress/plan.md`. Moves the download-plugin
pipeline's actual byte-level HTTP transfer (currently performed nowhere — `execDownload`'s
`download_url` result is stored as-is and never fetched) into Rust itself via streaming `reqwest`,
which is what makes real progress reporting, per-domain concurrency limiting, and rate limiting
possible at all. `execDownload`'s contract gains `downloads: {url, method?, headers?,
filename_hint?}[]` (one element = single-file download; more = a multi-resource download, e.g.
Pixiv's per-page images, optionally bundled into one archive by Rust rather than the plugin's own
Deno-side zipping); a new, parallel `pluginOptions()` export lets a plugin declare its own default
per-domain concurrency/rate-limit rules (LastPass-style exact/wildcard matching, exact taking
precedence) and multi-resource bundling preference, user-overridable and persisted in Redis via
new `/api/plugins/{namespace}/options` endpoints. `JobStatus` gains `downloaded_bytes`/
`total_bytes`, rendered as a real progress bar on the existing Jobs page. Three existing
hand-written plugins (`chaika.ts`, `ehentai.ts`, `pixiv.ts`) are migrated to the new contract as
part of this feature. Design artifacts:
`specs/005-download-plugin-progress/{research.md,data-model.md,contracts/,quickstart.md}` (no
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
