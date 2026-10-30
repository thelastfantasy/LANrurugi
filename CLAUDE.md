<!-- SPECKIT START -->
Seventeen feature specs exist. Status below is read from each spec's own `tasks.md` checkbox counts,
not from memory — a spec's header status is worth re-checking against its task list whenever this
block is regenerated, because it has gone stale before.

- **Implemented (all tasks complete)**: `001-lanrurugi-full-rewrite` (96/96), `002-job-console`
  (23/23), `003-ui-test-automation` (52/52), `005-download-plugin-progress` (41/41),
  `006-ai-plugin-wizard` (53/53), `011-api-docs-site` (this feature's own list),
  `013-ebook-formats` (31/31).
- **Code implemented, some tasks outstanding** — in every case the remainder is real-deployment
  or real-asset verification rather than unbuilt code: `004-ocr-manga-translation` (70/76,
  awaiting the `quickstart.md` scenarios that need real OCR model files),
  `007-guest-restricted-access` (52/54, awaiting its `quickstart.md` run against a real
  deployment), `010-subscription-orchestrator` (88/98), `012-large-page-translation` (20/21 —
  implemented and verified end to end against a real `lanrurugi-gpu-worker` with real LaMa weights,
  including a byte-for-byte pre-change/post-change capture of the erase output for a page that
  already worked; its one open task is the repo-wide `mise run clippy` gate, blocked by
  pre-existing `result_large_err` warnings in crates this feature never touched. Its `tasks.md`
  Implementation Record states exactly what was and was not verified — notably that the
  reader-facing HTTP end of its quickstart scenario 4 and all of scenario 5 were not exercised).
- **Code implemented, release verification outstanding**: `015-native-platforms` — native packages for
  Linux x86_64/aarch64, macOS arm64 and FreeBSD x86_64; the Linux package is verified locally in a clean
  Debian container, while macOS, FreeBSD and Linux aarch64 are only verified by
  `.github/workflows/native-release.yml`, which has not run yet (see its `tasks.md`).
- **Code implemented, full-suite run outstanding**: `014-ai-harness-hardening` (52/53 — the open task is
  the user-run `mise run test`). Every model call goes through `lanrurugi_llm`'s `CallSpec` seam
  (deadline, retry-once, cancellation, usage ledger `LANRURUGI_LLM_USAGE:*` read by `GET /api/llm/usage`);
  the plugin wizard's fetches pass `plugin_wizard/fetch.rs`'s outbound policy (credentials only to the
  target site, hops off the user's sites end the chain), trial runs need user approval for off-site
  declared hosts, and each generation is recorded at `LANRURUGI_GENERATION_RECORD:*`. Its `tasks.md`
  Implementation Record lists what was and was not verified, and where the code departs from plan.md.
- **Code implemented, user verification outstanding**: `016-tankoubon-grouping-harness` (30/30 —
  single-crate gates run; the full `mise run test` and the modal's visual sign-off are the user's).
  See its addendum below.
- **Code implemented, decisions and user verification outstanding**: `017-archive-sharing`
  (100/103; issue #119 — shares with import/read/download permissions, iroh + pkarr transport,
  LANrurugi nodes, helpers, tankoubon zip download). Open: T038 (Playwright spec not run), T084
  (iroh's QAD probe shows ALPN `/iroh-qad/0` to on-path DPI while a relay is in use — fork, keep, or
  drop QAD is the user's call), T102 (workspace `check`/`clippy`). Its `tasks.md` Implementation
  Record lists the network matrix (15 environments, all importing), the defects it found and fixed,
  and every departure from plan.md. `scripts/share-network-matrix.sh` / `share-traffic-check.sh`
  rerun them.
- **Plan only, no `tasks.md` yet**: `008-subfolder-tankoubon`, `009-archive-split-suggestions`.

**Phase 1 — `001-lanrurugi-full-rewrite`** (build this first): plan at
`specs/001-lanrurugi-full-rewrite/plan.md`. User Stories 1–8 — library continuity, non-merging
ingestion, third-party API compatibility, plugin metadata enrichment, backup/export, duplicate
repair, UI localization, concurrency benchmarking vs. the legacy Perl system. Design artifacts:
`specs/001-lanrurugi-full-rewrite/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 1 addendum — `011-api-docs-site`** (additive to Phase 1, implemented): plan at
`specs/011-api-docs-site/plan.md`. Two deliverables, one pipeline: (a) `crates/lanrurugi-api-docgen`,
a build-time binary that recovers the route table by parsing `crates/lanrurugi-api/src/**` with `syn`
— axum's `Router` cannot be asked what was registered, so the source is the source of truth — and
merges it with the vendored upstream LANraragi OpenAPI document (`docs/api/legacy-openapi.yaml`,
which supplies field-level documentation for the ~55 paths the two projects share) and hand-written
fragments (`docs/api/sidecar/`), emitting one OpenAPI 3.1 JSON document; (b) a Starlight +
`@scalar/starlight` documentation site under `docs/`, published from CI to Cloudflare Pages and
shipped inside the image at `/docs` (`LANRURUGI_DOCS_DIR`) for offline reading. The route scan is
applied *last* as the completeness authority, which makes "documented but not served" and "served but
not documented" structurally impossible; the gates catch the residue (an unresolvable route, an
unknown mount prefix, a sidecar entry for a route that does not exist). `cors.rs`'s
`Access-Control-Allow-Headers` gained `Content-Type` here (FR-016) — without it `application/json`
is not a CORS-safelisted value, so every JSON-bodied cross-origin request was preflighted and
refused. Design artifacts:
`specs/011-api-docs-site/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 1 addendum — `016-tankoubon-grouping-harness`**: plan at
`specs/016-tankoubon-grouping-harness/plan.md`. "AI smart tankoubon" grouping is local recall plus an
LLM review. `tankoubon_grouping.rs` scores every pair once into a `PairEvidence` (the grouping
decision, the "why" panel and the evaluation all read it) behind `GroupingSettings`. Deterministic
gates: a creator conflict (artist + circle as one set) blocks a pair unless either side is an
anthology or they share a cosplayer; `release_markers.rs` strips 中国翻訳/DL版-style tokens; tag
overlap is IDF-weighted over the candidate pool. `tankoubon_grouping/review.rs` reviews only the card
in view plus the next two, through `FeatureId::TANKOUBON_GROUPING_REVIEW`. It may only *remove*
members (validated, one repair, else `fell_back`), skips groups that already share a creator or
cosplayer (`settled`), and sends a compact payload (≤ 12 members, cleaned titles, ≤ 6 rare tags).
Reviews are cached by suggestion id + prompt version + content hash. Creating or dismissing a
suggestion writes pair labels (`LANRURUGI_GROUPING_LABELS`). `lanrurugi-server eval-grouping` replays
those labels to report precision and recall before any weight, gate or prompt change. Its prompt
lives in `llm_prompts.rs`, like every model-facing string.

**Phase 1 addendum — `015-native-platforms`**: plan at `specs/015-native-platforms/plan.md`. Native
release packages next to the Docker image. `lanrurugi-server::install_layout` runs first in `main`
(before the Tokio runtime exists — it calls `set_var`) and fills every **unset** location variable
(`LANRURUGI_STATIC_DIR`/`PLUGINS_DIR`/`DOCS_DIR`/`PDFIUM_LIB`, `ORT_DYLIB_PATH`, model dirs) from the
install prefix when that path exists, so explicit settings and the container image are never
overridden; `capabilities::log_summary` logs once which optional components were found. Packages are
assembled by `scripts/package-native.sh` from `scripts/build-native-assets.sh` output and checked by
`scripts/smoke-native.sh`; locally `mise run package-native` builds the Linux one. Keep new
Linux-only code behind `cfg(target_os = "linux")` with a portable fallback, and keep any such
fallback compiled and unit-tested on Linux too (macOS/BSD are only built in CI).

**Phase 1 addendum — `013-ebook-formats`** (implemented): plan at `specs/013-ebook-formats/plan.md`.
PDF, EPUB (fixed-layout/reflowable), MOBI/AZW/AZW3, DjVu, FB2 and TXT are catalogued as archives
whose pages are synthetic entries (`0001.jpg`, `0002.webp`) — `lanrurugi-scanner::archive_format`
dispatches every book to `crates/lanrurugi-book` (client: worker pool, info/render caches keyed by
file identity plus `CACHE_VERSION` — **bump it whenever the worker would answer differently for the
same file**, or stale page lists/outlines are served), which talks to `crates/lanrurugi-book-worker`
subprocesses (PDFium, DjVuLibre CLI, own EPUB/MOBI/FB2/TXT parsers, cosmic-text typesetting).
Stored pictures are served as their stored bytes; text books are typeset server-side. Outlines are
imported into an empty ToC only. Rendered pages live under `<temp>/book/` and count against
`tempmaxsize`. Design artifacts:
`specs/013-ebook-formats/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 1 addendum — `002-job-console`** (additive to Phase 1, implemented): plan at
`specs/002-job-console/plan.md`. Background job management console (a
Minion-admin-console equivalent) surfacing the existing in-process job registry
(`lanrurugi_core::jobs`) — list/monitor/inspect/clear queued, active, finished, and failed jobs
(thumbnail regen, backup/restore, rescans, duplicate scans, index rebuilds) via new additive
`/api/jobs*` endpoints, deliberately separate from the legacy-mimicking `/api/minion/*` contract.
Retry is explicitly out of scope. Design artifacts:
`specs/002-job-console/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 1 addendum — `003-ui-test-automation`** (additive to Phase 1, implemented): plan at `specs/003-ui-test-automation/plan.md`. Two-layer automated frontend test
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
`specs/004-ocr-manga-translation/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}` —
tasks.md records 70 of 76 complete; the remainder is `quickstart.md` verification against real OCR
model files, not unbuilt code. A 2026-09-06
`/speckit-analyze` re-check against constitution 1.8.0 (ratified 2026-08-03, after this spec's
2026-07-06 authoring) found and fixed two real gaps: `data-model.md`'s ID fields needed a note
requiring the newtype-ID constraint added by that constitution version, and tasks.md's T009 needed
its `rayon`-batching wording tightened against that same version's new loop-of-single-item-
`spawn_blocking` anti-pattern bullet — see tasks.md's own 2026-09-06 revision note for detail.

**Phase 1 addendum — `005-download-plugin-progress`** (additive to Phase 1, implemented): plan at `specs/005-download-plugin-progress/plan.md`. Moves the download-plugin
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
`specs/005-download-plugin-progress/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 1 addendum — `006-ai-plugin-wizard`** (additive to Phase 1, implemented): plan at `specs/006-ai-plugin-wizard/plan.md`. A wizard that generates login/metadata/
download plugins from a natural-language description of a target site, gated by an up-front domain
lookup (reusing `find_matching_plugin`'s `url_pattern` matching) so a user only ever fills in the
types genuinely missing for that domain. Generation runs through a new `lanrurugi-llm::tool_chat()`
(DeepSeek tool-calling, model `deepseek-v4-pro` — `deepseek-chat`/`-reasoner` were announced
discontinued 2026-07-24 and are deliberately not used for this feature's new call sites, see
research.md §1) that lets AI request the system fetch a real page or a same-domain reference sample
mid-generation rather than guessing from text alone; the system executes every access itself and
forwards raw results — AI does all semantic judgment (what to generate, why a trial run failed,
whether a failure is login-related), never the reverse. Draft trial runs stage to
`plugins/custom/_wizard/` and reuse `PluginPool`'s existing two-phase-permission execution
unmodified, always discarded afterward (never promoted without an explicit confirm-save, which
reuses `upload_plugin`'s own validate/move/rollback path). Login test credentials are never sent to
the LLM under any circumstance (FR-012) — only a sanitized outcome is. All wizard session/draft-
history state is frontend-only (no new Redis schema, no server-held session) per the spec's own
assumption that history need not survive a page refresh. Design artifacts:
`specs/006-ai-plugin-wizard/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

**Phase 1 addendum — `007-guest-restricted-access`** (additive to Phase 1, code implemented; its
`quickstart.md` real-deployment run is the outstanding task): plan at `specs/007-guest-restricted-access/plan.md`. Replaces the legacy
`enablepass`/`nofunmode` on/off password toggles (which only ever produced "fully open" or "fully
locked", with no in-between) with a strict, non-configurable password requirement for every
administrative function, paired with a new opt-in **restricted guest access** mode: a site-wide
"guest mode" switch plus per-`Category` `visible_to_guest` marking together determine whether an
unauthenticated visitor is routed into a scoped, read-only browsing experience (list, read,
search/tag-filter — confined to guest-visible categories; no bookmarking, no progress-saving, no
raw-file download, no admin functions) instead of being redirected to `/login`. The new
`guest_visitor` Casbin subject is governed entirely through `route_policy.csv` (route-level
allow/deny, reusing `token_guest`'s existing GET-only-plus-deny-list shape verbatim, plus one
`guest_visitor`-only deny for the raw-download endpoint); category-level content scoping — which
Casbin's RBAC model has no mechanism for — is enforced via a new
`SearchParams.restrict_to_archive_ids` field that reuses the search engine's existing
retain-a-candidate-set filtering pattern rather than a Casbin ABAC rule or a post-pagination
result filter (the latter was considered and rejected — it would desync
`recordsFiltered`/`recordsTotal` from the actually-returned result count, itself a form of the
information leakage this feature is designed to prevent). Out-of-scope archive access returns 404
(indistinguishable from nonexistent), never 403. `devmode` (confirmed to have zero server-side
behavior — only suppressed the frontend's GitHub-releases update check) is removed from the
Settings page entirely and replaced by a deploy-time `--disable-update-check` CLI flag /
`LANRURUGI_DISABLE_UPDATE_CHECK` env var, following the exact `--no-pass`/`LANRURUGI_NO_PASS`
pattern already established in this codebase. `/api/info`'s `has_password` field stays present
(now hardcoded `true`) and `nofun_mode` is removed entirely — a documented, spec-mandated
constitution Principle II exception (research.md §5), not an oversight. Design artifacts:
`specs/007-guest-restricted-access/{research.md,data-model.md,contracts/,quickstart.md,tasks.md}`.

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

## Issue tracking: never open a new issue

**Do not create GitHub issues — under any circumstances** (stated directly by the user,
2026-10-09: "任何情况都不要新开issue"). A problem that surfaces mid-session but isn't resolved in
that same turn is either fixed now or reported in the conversation itself; "file it and move on" is
not an option, and neither is a local memo/scratch file.

Adding a comment to, or closing, an *already existing* issue is not opening one — but ask before
touching issue bookkeeping at all, since the point of this rule is to stop spending turns on it
rather than to move that work somewhere else. Any content that does end up on an existing issue
must be in Chinese.

Earlier revisions of this file told the agent to open an issue per mid-session problem (and to
prefer appending to an existing one); that instruction is superseded by the rule above.

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
`gh issue list --milestone m1 --state open` is backlog, not urgent. Reading these lists is fine;
*adding* to them is not — see the rule above, which supersedes the milestone-at-creation-time
instruction this paragraph used to carry.

## Dev container rebuild — frontend-only changes don't need one

The dev container (`compose.dev.yaml`) runs the frontend via `vite dev` with `apps/frontend`
bind-mounted from the host, not a pre-built static bundle — so a pure frontend edit (`.tsx`/`.ts`/
`.css` under `apps/frontend/`) is picked up by Vite's HMR immediately on save. `plugins/` is
*also* bind-mounted (`compose.dev.yaml`'s own comment on that line explains why: `discover_
namespaces` rescans the directory on every request rather than caching it once, and each plugin
invocation spawns a fresh Deno subprocess that reads the `.ts` file straight off disk with no
persistent compile cache to invalidate) — so a plugin `.ts` edit (new/changed `pluginInfo()`/
`pluginOptions()`/`execDownload()`/etc.) also takes effect on the very next plugin call, no
rebuild needed either. Verified live (2026-08-15): editing `chaika.ts`'s `pluginOptions()` and
immediately curling `GET /api/plugins/options?namespace=download/chaika` from the running dev
container returned the new value with zero rebuild step. `mise run dev-rebuild && dev-down &&
dev-up` is only needed when a change touches something actually baked into the image at build
time: Rust code (`crates/`), or the Dockerfile/compose files themselves (which also determine
what ends up bind-mounted vs. baked in, in production the same `plugins/` files *are* baked in
via `COPY` — this bind-mount-takes-precedence behavior is dev-only). Don't reflexively rebuild
after every change — check whether the edit was frontend/plugin-only first. This includes
`apps/frontend/index.html` itself — `vite.config.ts`'s `injectServerTheme` plugin (the dev-mode
half of the server-side anti-flash-of-default-theme mechanism — see that file's own docs) uses
Vite's own `transformIndexHtml` hook, which re-reads the file from disk on every request, same as
any other Vite-served frontend file; no build step of its own to go stale.

**Exception: adding a new frontend dependency DOES need a rebuild, plus one extra step beyond
the usual `dev-rebuild && dev-down && dev-up`.** `apps/frontend/node_modules` is a **named volume**
(`lrr-dev-frontend-node-modules`, `compose.dev.yaml`'s own comment on that line explains the
"poke a hole back through the bind mount" reasoning), not part of the `apps/frontend` bind mount
itself. Podman/Docker only ever seed a named volume from the image's contents at that path *once*
— the first time the volume is created. On every later `dev-rebuild`, the freshly-built image *does*
contain the new dependency (a real `pnpm install` ran during the image build), but the running
container keeps using the **old, already-existing volume**, which still only has whatever was
installed the last time that volume was actually seeded — the new dependency is silently missing
from what `vite dev` actually sees, surfacing as a browser/devtools module-resolution error that
looks like nothing was rebuilt at all. This has recurred every time a new frontend dependency was
added (e.g. `@uiw/react-codemirror`/`@codemirror/lang-javascript`/`codemirror` for
`006-ai-plugin-wizard`'s `CodeEditor.tsx`) — the actual fix is deleting the stale volume so a
rebuild re-seeds it from scratch, not just rebuilding the image again:

```sh
mise run dev-down
docker volume rm lanrurugi_lrr-dev-frontend-node-modules   # or: podman volume rm ...
mise run dev-rebuild
mise run dev-up
```

Also remember to regenerate the root `pnpm-lock.yaml` (`pnpm install` from repo root) *before*
`dev-rebuild` whenever `apps/frontend/package.json` gains a new dependency — `Dockerfile.dev`'s
`pnpm install --frozen-lockfile` step fails outright (not silently) if the lockfile doesn't already
list the new package, which is a separate, earlier failure mode than the stale-volume one above but
easy to conflate with it since both surface around the same "I just added a dependency" moment.

## Rust edits — check before rebuild, and always through the resource-capped script

After editing any Rust code under `crates/`, run `cargo check` (from the workspace root) BEFORE
`mise run dev-rebuild`. A rebuild that fails inside the container wastes time; `cargo check`
catches compilation errors in seconds instead of minutes. Only proceed to rebuild after check
passes clean.

**Always run Rust builds/tests through a `mise run` task** — `mise run test` / `mise run clippy` /
`mise run fmt-check` / `mise run fmt` / `mise run build` / `mise run check-crate -- <crate> [<crate>
...]` (the last one for fast single-crate iteration instead of `--workspace`), never a bare
host-side `cargo` and never `scripts/cargo-container-run.sh` invoked directly — that script now
refuses to run at all unless `$MISE_TASK_NAME` is set (i.e. it was reached through `mise run`), so
a direct call fails fast with a pointer to the right task instead of silently skipping the
guardrails below. Those guardrails: CPU quota per invocation; real memory/priority capping
(`memory.max`/`cpu.weight`) enforced unconditionally at the host level via a personal,
non-repo-tracked `~/.config/containers/containers.conf` (`[containers] cgroup_conf`) plus a
`~/.local/bin/cargo` shim for any bare host `cargo` invocation that isn't containerized at all; a
PSI (`/proc/pressure/memory`) check that refuses to start a new build while the host is already
under real memory pressure; a cooldown between invocations so back-to-back calls don't pile
pressure on top of each other before it's had a chance to settle; and an `flock` mutex so two
invocations can never actually run concurrently (they queue, not race) — see those files' own
comments. Added after a real OOM crash (2026-07-21), a `systemd-oomd` pressure-kill of an unrelated
process during a workspace build (2026-08-13), and two more `systemd-oomd` kills of the VSCode
window itself on 2026-08-14 (the second confirmed caused by several of these invocations
overlapping — each individually passing its own point-in-time PSI check — before the cooldown/mutex
above existed to prevent it).

**`memory.high` in that containers.conf must sit above where a build settles, not at it.** Diagnosed
2026-10-11 after `mise run test-crate` started dying with exit 137 and no completion record: the
setting was `memory.high=3G`, and a build's cgroup footprint — which is *ceiling-bound*, since
`memory.current` counts page cache and cache expands to fill whatever room exists — parked on that
ceiling for the entire run. Continuous cgroup-local reclaim followed (~9,990 throttle events in one
run, with `memory.max` events at **0**), and because reclaim stalls propagate to every ancestor,
`user@1000.service`'s PSI hit 70.98% against systemd's default `ManagedOOMMemoryPressureLimit` of 50%
for 20s, so `systemd-oomd` killed the build container. The limit meant to contain the build was what
killed it. Raising it to 6G took the same full `mise run test` to a clean pass (1,312 cases, 44
binaries) with session PSI at 36.6%/11.0% (avg10/avg60) instead of 77.6%/41.6%. Two lessons worth
keeping: don't size that value from a run that was itself capped, and don't read "the build peak is
~3GB" (a real figure, but for build-script anonymous memory) as the cgroup's total footprint.
`scripts/cargo-container-run.sh` now prints these pointers itself on any exit 137.

## `mise run test`/`dev-rebuild` must run detached from the agent/VSCode process tree

A `cargo test --workspace`/`dev-rebuild` invocation launched as a normal foreground or
backgrounded-in-the-same-shell child of the coding agent's own process is still a descendant of
the VSCode/agent process tree — when `systemd-oomd` picks a victim under real memory pressure, it
can (and, confirmed live, did) kill the *agent's own shell* or a `conmon`/container-monitor process
in that same tree even though the actual heavy work is properly cgroup-capped inside its own
container. **Why:** two real incidents on 2026-08-25 during one session — a `mise run test`
launched via the agent's own backgrounding wrapper got silently killed with "no completion record
found... may have been stopped" and its containers' `conmon` processes were later found already
killed when cleanup was attempted, and a second, independent `mise run test` invocation was killed
the same way moments later. Both happened *after* the guardrails in the section above (PSI check,
cooldown, flock mutex, cgroup caps) were already in place and functioning correctly — those
guardrails cap what the *test workload itself* can consume, they do nothing to protect the
*launching shell/process* from `systemd-oomd` picking it as a victim by different criteria (process
tree membership, OOM score) when overall host pressure is high for any reason, including reasons
unrelated to this specific invocation. **How to apply:** when a Rust build/test needs to actually
run (not just `cargo check`), launch it as a real detached top-level process — outside the agent's
own process tree entirely — rather than as a background job of the current session, e.g. `setsid
nohup mise run test > /tmp/test-output.log 2>&1 < /dev/null &` followed by `disown`, or hand the
command to the user to run themselves in their own terminal, and read the resulting log file
afterward rather than waiting on the process directly. Do not use the agent harness's own
background-task mechanism for this — that still parents the process under the agent's own tree.
This is stricter than "just run it through `mise run`" above: that rule is about which *script*
runs the workload (the resource-capped one, not bare `cargo`); this rule is about which *process
tree* launches that script in the first place.

## CPU-bound parallel work must cap its own resource usage

New `rayon` parallel work must not default to the uncapped global pool — cap at ~30% of cores
(`lanrurugi-api::recommend_precompute::precompute_worker_budget`, or a local duplicate of that
formula for crates that can't depend on `lanrurugi-api`). Background jobs additionally get
`LoadThrottle`-style live backoff; a short interactive operation just needs the fixed cap.

## Comments: keep it short

Doc comments explain the non-obvious "why" in one to a few sentences, not a full narrative of
alternatives considered, prior bugs, or verbatim user quotes. If it needs a paragraph, it's too long.

## UI-only changes require human verification before commit — and the user does that verification, not the agent

When a change is purely UI (visual layout, spacing, styling, tooltip/overlay behavior — no
backend/logic change riding along), do not `git commit` it yourself, and do not spend a round of
browser-tool screenshots/hovers/snapshots trying to self-verify the visual result either — the
user explicitly does not want that loop (confirmed live: after a z-index fix, told directly "我说过ui确认你不用做的" when the agent kept reopening the modal to hover-test it). Implement the
change, run the relevant type-check/lint, then stop and hand it directly to the user to look at
and confirm — do not interpose a self-verification pass in between. This is stricter than the
general "only commit when explicitly asked" rule elsewhere — even an explicit earlier "go ahead
and commit your fixes" for a session does not cover a purely-visual change encountered later in
that same session; ask again. A change that's mostly logic but happens to touch a `.tsx` file's
styling too is not "purely UI" — this rule is about changes where the entire diff is visual polish
with no functional risk to independently verify against. (Non-UI changes — new backend logic, data
flow, a new API contract — still warrant the agent's own functional verification, e.g. curling an
endpoint or checking Redis state; this rule is specifically about *visual* polish, where "does this
look right" is a judgment only the user watching their own screen can actually make.)

## Custom colors must be theme-aware, never a hardcoded value

Any new UI element that needs its own color (a highlight, a status background, an accent —
anything beyond what an existing legacy-derived class already supplies) MUST adapt across all 5
themes, not just whichever one happens to be active during development. Concretely:

- Add a real CSS class carrying the color to **each** of the 5 real theme files under
  `apps/frontend/public/legacy/themes/` (`g.css`, `ex.css`, `modern.css`, `modern_red.css`,
  `modern_clear.css`), one rule per file, each using a color that fits *that* theme (in practice:
  reuse that same theme's own existing accent hue for an analogous "this is special" state —
  e.g. `.msm-selected`'s row-highlight color — for visual consistency within the theme, not a
  color invented from scratch). Reference the class from the component via `className`, not an
  inline `style` with a literal color value — an inline style can't vary per theme at all.
- This is a deliberate exception to the general "don't edit the copied theme files, they mirror
  legacy verbatim" rule elsewhere in this doc: that rule protects existing legacy-derived rules so
  they stay diffable against real legacy. A *new* class for a concept legacy doesn't have at all
  (no legacy equivalent to stay diffable against) doesn't conflict with that — it's additive,
  appended at the end of each file, never modifying anything legacy actually wrote.
- Do NOT reach for the `--theme-*` CSS custom properties defined in `apps/frontend/src/index.css`
  for this — that block's own doc comment marks it an explicitly transitional fallback, kept only
  for components not yet migrated to legacy's own real classnames, meant to be deleted (not grown)
  as migration proceeds. A page already written against legacy's own classnames (`.checklist`,
  `.favtag-btn`, `.stdbtn`, etc. — e.g. `Categories.tsx`) is exactly the "already migrated" case
  that variable block is waiting to lose its last reader, so adding new usage of it there would be
  a step backward.
- Precedent: `.tankoubon-member-row` (a Categories-page checklist-row highlight for an archive
  that's also a Tankoubon member) — added to all 5 theme files this way, reusing each theme's own
  `.msm-selected` accent color, after an initial attempt hardcoded a single `rgba(...)` value that
  only looked right on the one theme it was eyeballed against.

## Base UI reference docs

When building or reviewing a `@base-ui/react`-based component (e.g.
`apps/frontend/src/components/common-ui/`), check https://base-ui.com/llms.txt first — it indexes
the per-component doc pages (anatomy/recommended composition) and the styling/composition/
customization handbook pages. The official-recommended state-styling pattern is Tailwind
`data-[attr]:class` variants (e.g. `data-highlighted:bg-neutral-950`) reading Base UI's own
`data-*` state attributes directly in CSS — reach for that first. A `style`-as-a-function-of-state
prop (what `common-ui/Form/Select.tsx`'s `SelectItem` uses) is only justified when the value itself
is runtime/per-theme data (`useMenuPalette()`'s colors) that a static Tailwind class can't express,
not as a default habit.

## Pre-push checks required

After completing each batch of edits, run `mise run check` and confirm all checks
pass (rust-check ✔️ + frontend-lint ✔️). Fix any failures before proceeding. Never
offer `--no-verify` as a workaround.

## `--no-verify` is forbidden

Never use `git commit --no-verify` or `git push --no-verify`. If the pre-push/pre-commit hook
fails, debug the actual error — do not bypass the hook. There is no situation where skipping
hooks is acceptable.

## Before pushing — check whether README.md needs updating

Before any `git push`, review what the commit(s) being pushed actually changed and ask: does this
add/fix/remove something one of these already makes a claim about, or should now make a claim about?

- `README.md`'s "能做什么" bullets (the only feature list it still carries — the long
  comparison moved out; see next bullet) and its 快速开始 / 文档 sections,
- `docs/comparison-with-lanraragi.md` (the long-form item-by-item comparison, including the
  deliberately-not-an-improvement entries),
- `CONTRIBUTING.md` (toolchain, build/test commands, pre-push gates) when a command or gate changes.

A shipped feature, a real bug fix with user-facing impact, or a spec moving from
planned-but-unimplemented to fully implemented (verify via that spec's own `tasks.md` checkbox
completion, not assumption) are all candidates. Not every push needs a change to any of them —
routine internal refactors, test-only changes, or mid-flight work-in-progress commits usually don't
— but skipping this check silently lets them drift out of date the same way the "002/003/005 are
planned but not yet implemented" line above did until it was caught and corrected during a real
README-writing pass.

## Real adult-work titles/URLs must never be hardcoded — use env vars instead

Any test, fixture, comment, or example that needs a *real* archive title, author/circle handle, or
gallery URL (as opposed to synthetic/placeholder data) for an adult work must not have that string
committed to source at all — read it from an environment variable (or an env-var-supplied external
file path for larger structured fixtures) instead, following this repo's existing
`TEST_REAL_DOWNLOAD_URL`/`TEST_REAL_EHENTAI_GALLERY_URL` convention in `.env.example`/`.env.local`:
document the variable (name, purpose, what skips if it's unset) in `.env.example` with the value
left blank, put the real value only in the gitignored `.env.local`, and have the consuming code
treat an unset/missing var as "skip this test, don't fail" rather than requiring it. This applies
retroactively too — if you discover a real title/handle/URL already hardcoded anywhere (source,
tests, fixture JSON files, doc comments), scrub it out to an env var the same way, including from
git history if it's already been committed (`git filter-repo --replace-text`, verified against a
disposable clone before ever touching the real repo or force-pushing). Precedent: issue #70's
model-switch work found a 60-title real e-hentai search-result fixture (`tests/fixtures/
series_titles.json`) and three individual real titles in `embedding.rs`'s smoke test hardcoded in
source and already pushed to two prior commits; both were moved to
`LANRURUGI_TEST_FIXTURE_SERIES_TITLES_PATH` (external file, since the fixture also carries
structured series/volume annotations no set of individual env vars could practically hold) and
`LANRURUGI_TEST_TITLE_SAME_SERIES_A`/`_B`/`LANRURUGI_TEST_TITLE_CROSS_SERIES` respectively, and the
already-pushed history was rewritten to scrub the real strings out of every prior commit too.

## Commit as one batch — never split a pending changeset into multiple commits by feature

When asked to commit, stage and commit every pending file in one commit (minus anything that
should genuinely be gitignored) — do not attempt to split a batch of changes into several commits
grouped by feature/concern, even when the changeset spans multiple unrelated features. Confirmed
live, 2026-08-27: asked to commit+push a large pending changeset (007-guest-restricted-access +
issue #97 + several unrelated mobile/UX fixes), spent significant time trying to hand-split it into
per-feature commits by reverting/reapplying hunks in files where two features' changes landed
adjacent to each other in the same function (`settings.rs`'s `BOOL_FIELDS`, `types.ts`'s `Settings`
interface, `SettingsPage.tsx`'s state block) — high risk of mis-splitting, and the user explicitly
cut this off ("别整你那些花活了" / "1个commit赶紧完事") after watching the back-and-forth. If a
changeset is large enough to make per-feature attribution valuable, that's a signal to commit more
often *during* the work (right after each feature lands, before the next one starts touching the
same files) — not a reason to reverse-engineer feature boundaries out of an already-tangled
worktree after the fact.
