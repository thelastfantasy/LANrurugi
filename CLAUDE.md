<!-- SPECKIT START -->
Two feature plans exist; implementation has not started on either yet.

**Phase 1 — `001-lanrurugi-full-rewrite`** (build this first): plan at
`specs/001-lanrurugi-full-rewrite/plan.md`. User Stories 1–8 — library continuity, non-merging
ingestion, third-party API compatibility, plugin metadata enrichment, backup/export, duplicate
repair, UI localization, concurrency benchmarking vs. the legacy Perl system. Design artifacts:
`specs/001-lanrurugi-full-rewrite/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 2 — `002-ocr-manga-translation`** (depends on Phase 1; independent plan, must not block
or be blocked by it — constitution Principle VI): plan at
`specs/002-ocr-manga-translation/plan.md`. On-page manga translation — OCR detection/merging,
user-selectable translation backend (cloud, proxied server-side, vs. locally-hosted, browser-
originated), volume-level font-matching cache, sliding-window prefetch with cost-aware budgeting.
Design artifacts:
`specs/002-ocr-manga-translation/{research.md,data-model.md,contracts/,quickstart.md}` (no
`tasks.md` yet).

Stack: Rust (Tokio/Axum/Rayon) backend as a Cargo workspace under `crates/` producing one binary
(`lanrurugi-server`, with `serve`/`rebuild-index`/`bench` subcommands), Redis reused as-is from
the legacy deployment, React 19 + TypeScript + Vite + Tailwind + Zustand + TanStack Query
frontend under `frontend/`, Deno-subprocess plugin runtime, `bench/` for the cross-system
performance comparison harness. Phase 2 adds `crates/lanrurugi-ocr`, `crates/lanrurugi-fontcache`,
`crates/lanrurugi-translate`, and `frontend/src/translation/` to that same workspace/app — not a
separate deployable. Governing rules: `.specify/memory/constitution.md`.

For additional context about technologies to be used, project structure,
shell commands, and other important information, read the current plan for whichever phase you
are working on.
<!-- SPECKIT END -->
