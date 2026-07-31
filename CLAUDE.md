<!-- SPECKIT START -->
Five feature specs exist. Phase 1 (001) is implemented; 002, 003, and 005 (all additive addenda to
Phase 1) are also fully implemented (verified via each spec's own `tasks.md` — all checkboxes
complete, no outstanding items). Only Phase 2 (004) remains planned but not yet implemented.

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

## UI migration verification (mandatory)

When porting/reproducing a piece of real legacy UI (an icon, a layout region, a component's
markup structure — anything claimed to match `~/LANraragi` or a confirmed-consistent live
reference site), visual screenshot comparison alone is NOT sufficient sign-off. Every element
being ported MUST have its real computed style pulled from the ground-truth source (via
`getComputedStyle`/`getBoundingClientRect` in a live browser session, not guessed from reading
CSS source or from memory of an earlier check) and diffed field-by-field against the same
properties on the reproduction, at minimum: `font-size`, `font-weight`, `line-height`, rendered
height/width (`getBoundingClientRect`), `border-radius`, `padding`, `margin`, and any icon's actual
size class (e.g. Font Awesome `fa-2x`/`fa-3x`/`fa-4x` — these are easy to copy-paste wrong between
similar-looking icons in the same region and the visual difference at screenshot scale can be
subtle enough to miss by eye). A claim of "matches legacy" is not valid until this comparison has
actually been run and the values shown to agree — not assumed, not eyeballed from a screenshot.

## Cross-session issue tracking (GitHub Issues, not memos)

Problems that surface mid-session but aren't resolved in that same turn (a bug spotted while
working on something else, a design question deferred, a regression noticed but not yet
root-caused) get tracked in a GitHub issue via `gh`, not a local memo/scratch file — durable across
context compaction and visible outside this tool.

Before creating a new issue, run `gh issue list --state open` and check whether an existing open
issue can hold the new item instead — append to it (`gh issue edit <number> --body-file <path>` to
rewrite the checklist, or `gh issue comment` for supplementary notes/screenshots) rather than
opening a new issue per problem. Only create a new issue when no suitable open one exists.
Resolved items are checked off (`- [x]`) with a short note on the actual root cause, not deleted —
the issue is a running log, not a todo list that gets wiped clean.

All *future* issue content (new entries, comments, edits) must be written in Chinese — the user's
own working language. Do not retroactively translate existing English entries already in the
issue; only new content going forward needs to be in Chinese.

## GitHub milestones — `m0` (first release) and `m1` (future plan)

Two milestones organize all issues (created by splitting the original single running-log issue #2,
which is now archived/closed and superseded — do not add new content to #2):

- **`m0`** — first release: the baseline feature/bug-fix set that defines what "v1 shippable"
  means for this project. Includes issues #3–#45 (the ones already closed, #3–#40, form the
  completed record of what shipped in the initial implementation + QA-fix round; #41–#45 remain
  open as the currently-scoped remaining work before release).
- **`m1`** — future plan: longer-horizon features, non-blocking polish, and infrastructure that
  don't gate the first release. Issues #46–#56.

When the user references "an issue" without a number, or asks what's left before release, check
`gh issue list --milestone m0 --state open` first — that list *is* the release-blocking work.
`gh issue list --milestone m1 --state open` is backlog, not urgent. When triaging a *new* problem
found mid-session (per the tracking rule above), decide `m0` vs `m1` by the same test used when
the split was made: does this affect the baseline shippable state (→ `m0`), or is it an
independent/long-horizon addition (→ `m1`)? Assign the milestone at creation time
(`gh issue create --milestone m0` or `m1`), don't leave new issues unmilestoned.

## Dev container rebuild — frontend-only changes don't need one

The dev container (`compose.dev.yaml`) runs the frontend via `vite dev` with `apps/frontend`
bind-mounted from the host, not a pre-built static bundle — so a pure frontend edit (`.tsx`/`.ts`/
`.css` under `apps/frontend/`) is picked up by Vite's HMR immediately on save. `mise run
dev-rebuild && dev-down && dev-up` is only needed when a change touches something baked into the
image at build time: Rust code (`crates/`), plugin `.ts` files under `plugins/` (copied into the
image, not bind-mounted), or the Dockerfile/compose files themselves. Don't reflexively rebuild
after every change — check whether the edit was frontend-only first. This includes
`apps/frontend/index.html` itself — `vite.config.ts`'s `injectServerTheme` plugin (the dev-mode
half of the server-side anti-flash-of-default-theme mechanism — see that file's own docs) uses
Vite's own `transformIndexHtml` hook, which re-reads the file from disk on every request, same as
any other Vite-served frontend file; no build step of its own to go stale.

## UI-only changes require human verification before commit

When a change is purely UI (visual layout, spacing, styling, tooltip/overlay behavior — no
backend/logic change riding along), do not `git commit` it yourself after your own browser
verification. Implement it, verify it works with the browser tools as usual, then stop and hand it
to the user to look at and confirm before committing. This is stricter than the general "only
commit when explicitly asked" rule elsewhere — even an explicit earlier "go ahead and commit your
fixes" for a session does not cover a purely-visual change encountered later in that same session;
ask again. A change that's mostly logic but happens to touch a `.tsx` file's styling too is not
"purely UI" — this rule is about changes where the entire diff is visual polish with no functional
risk to independently verify against.

## Before pushing — check whether README.md needs updating

Before any `git push`, review what the commit(s) being pushed actually changed and ask: does this
add/fix/remove something `README.md`'s own "## Improvements over LANraragi" section (or the Status/
Stack/Documentation sections) already makes a claim about, or should now make a claim about? A
shipped feature, a real bug fix with user-facing impact, or a spec moving from
planned-but-unimplemented to fully implemented (verify via that spec's own `tasks.md` checkbox
completion, not assumption) are all candidates. Not every push needs a README change — routine
internal refactors, test-only changes, or mid-flight work-in-progress commits usually don't — but
skipping this check silently lets the README drift out of date the same way the "002/003/005 are
planned but not yet implemented" line above did until it was caught and corrected during a real
README-writing pass.
