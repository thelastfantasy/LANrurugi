[简体中文](./README.md) | English | [日本語](./README.ja.md)

# LANrurugi

A from-scratch Rust + React rewrite of [LANraragi](https://github.com/Difegue/LANraragi), a
self-hosted manga/doujinshi library manager. Aims for feature parity with, and full data/API
compatibility with, existing LANraragi installations, while fixing a known duplicate-detection
defect and adding genuine multi-core concurrency where the legacy Perl implementation had none.

**Status**: Phase 1 (`specs/001-lanrurugi-full-rewrite/`) is implemented — all eight user stories
(library continuity, non-merging ingestion, third-party API compatibility, plugin metadata
enrichment, backup/export, duplicate repair, UI localization, and a concurrency benchmark against
the legacy system) are done; see `specs/001-lanrurugi-full-rewrite/tasks.md` for the full
breakdown. Three additive-to-Phase-1 addenda are also fully implemented: `specs/002-job-console/`
(background job management UI), `specs/003-ui-test-automation/` (Vitest + Playwright frontend test
coverage), and `specs/005-download-plugin-progress/` (real byte-level download progress,
per-domain concurrency/rate limiting). Only Phase 2 (`specs/004-ocr-manga-translation/`, on-page
manga translation) remains a plan with no implementation yet, deliberately kept independent per
constitution Principle VI so it never blocks or is blocked by Phase 1.

## Stack

- **Backend**: Rust (Tokio async runtime, Axum web framework, Rayon for CPU-bound parallelism),
  one Cargo workspace under `crates/` producing a single binary, `lanrurugi-server`, with
  `serve` / `rebuild-index` / `bench` subcommands.
- **Datastore**: Redis, reused as-is from the legacy deployment (five logical DBs — see
  `crates/lanrurugi-storage/src/redis.rs`).
- **Frontend**: React 19 + TypeScript + Vite + Tailwind + Zustand + TanStack Query, under
  `apps/frontend/`.
- **Plugins**: sandboxed Deno subprocesses, one per plugin namespace (`crates/lanrurugi-plugin/`).
- **Benchmark harness**: `crates/lanrurugi-bench/` — a synthetic-library generator, `criterion`
  microbenchmarks, and a cross-system comparison harness that drives both this binary and an
  actual legacy LANraragi instance side by side.

## Improvements over LANraragi

This isn't just "the same features in a different language." Below is a concrete inventory of
functional/architectural improvements — each one either fixes a real, named defect in legacy or
adds a capability legacy never had, not a "used Rust so it's automatically better" claim. Sections
marked **parity** are deliberate compatibility decisions, called out so this list doesn't overclaim.

### Data integrity & duplicate detection

- **A real fix for legacy's false-duplicate-merge defect.** Legacy computes an archive's ID as
  `SHA-1` of only the first 512,000 bytes of the file
  (`~/LANraragi/lib/LANraragi/Utils/Database.pm::compute_id`) — two different files that happen to
  share the same leading content (e.g. identical cover pages, different pages after that) hash
  identically and get silently collapsed into one library entry, clobbering whichever one's
  metadata got overwritten. LANrurugi's default ID algorithm
  (`crates/lanrurugi-storage/src/id.rs::size_aware_id`) folds the real file size into the hash
  input, so two archives sharing a prefix but differing in length no longer collide — while the
  original legacy ID is still computed and kept forever for read-compatibility with existing
  libraries (constitution Principle I). SHA-1 (not a newer hash) was deliberately kept rather than
  switched to something like BLAKE3, specifically because legacy code elsewhere enumerates archive
  IDs via a fixed-40-hex-character Redis key-glob pattern — changing the digest length would have
  silently broken every one of those glob patterns.
- **A one-time repair tool for libraries already damaged by that defect** (`lanrurugi
  rebuild-index` / `POST /database/rebuild-index`, User Story 6) — recomputes every archive's ID
  with the new algorithm, and any previously-hidden file that a historical false-merge had
  swallowed reappears as its own, independently taggable archive, without disturbing the
  already-correctly-tracked one's tags/reading progress. Legacy has no equivalent repair mechanism
  at all — once two archives were merged, they stayed merged.
- **Filename conflicts and content conflicts are no longer treated the same way.** A `QueueError`
  carries a `DuplicateReasonKind` (`ContentHash` vs. `Filename`) so the two genuinely different
  situations get genuinely different handling: a real content duplicate is unconditionally
  rejected (no "overwrite" bypass — overwriting identical content was previously dead work that
  also lost the original `date_added`), while a filename collision on *different* content stages
  the downloaded bytes to a temp file and offers a real choice via two new endpoints —
  `POST /download_queue/{id}/overwrite` and `POST /download_queue/{id}/rename` — letting the new
  download either replace the existing archive or be catalogued under a new name as a fully
  independent, coexisting archive. Legacy has no equivalent; this class of conflict was previously
  either undetected or silently overwritten. An hourly background sweep reclaims any staged file
  left unresolved for 24+ hours and downgrades the queue item to a distinct "expired, please
  re-download" state instead of continuing to offer actions on bytes that no longer exist.

### Concurrency & architecture

- **One consolidated process instead of three.** Legacy runs the main Mojolicious app, `Shinobu`
  as a separate file-watcher process, and Minion as a separate job-queue worker — three
  independent OS processes. LANrurugi is one `clap`-based binary (`lanrurugi-server`) with
  `serve`/`rebuild-index`/`bench` subcommands; `serve` runs the HTTP API, the `notify`-based file
  watcher, and the Deno plugin pool all as tasks inside one Tokio runtime, not separate processes.
- **A real fix for a concurrent-download race that could silently corrupt files.** Filename-
  conflict detection used to be a one-time directory scan with no lock between "checked, no
  conflict" and "wrote the file" — two downloads racing to the same resolved filename could both
  pass the check and then one would silently clobber the other's bytes on disk while both left a
  catalog record pointing at the same (now half-wrong) file. Fixed with a per-filename async lock
  (`AppState::lock_filename`, `crates/lanrurugi-api/src/state.rs`) held across the whole
  check-then-write window, with an RAII guard that releases correctly even across early-return
  error paths.
- **Cooperative download cancellation, not a forceful process-level abort.** A per-queue-item
  `CancellationToken` lets the download loop notice a stop request at the same point it already
  checks for network errors, reusing the identical partial-file cleanup path rather than leaving
  an untraceable orphaned temp file behind the way an `AbortHandle`-style hard-kill would (dropping
  mid-`write_all()` with no chance to clean up). Legacy has no per-download stop mechanism at all.
- **Explicit, correct CPU/async bridging.** All CPU-bound work (hashing, image decode/resize) runs
  via `rayon`, bridged into the async runtime through `tokio::task::spawn_blocking` — deliberately
  never inline on an async worker thread, so a bulk scan/reindex can't stall requests the HTTP
  server is concurrently serving. Legacy's single-threaded-per-request Perl model has no equivalent
  concept of "don't block the reactor" to get right or wrong in the first place; this is new,
  genuine multi-core parallelism for exactly the bulk operations (full library scan, duplicate-
  repair reindex) the benchmark suite below measures.
- **Request coalescing for expensive, frequently-repeated work.** Concurrent requests for the same
  missing thumbnail, or the same archive page, collapse onto one regeneration/read instead of each
  independently re-scanning the archive file from scratch — a class of duplicate-work legacy's
  per-request model never had to (or could) avoid.

### Download pipeline

- **Real, byte-level download progress.** Previously, a download plugin's actual HTTP transfer
  happened invisibly inside the Deno-sandboxed plugin process, with no way for the host to observe
  progress at all. The real transfer moved into Rust (streaming `reqwest`), so `downloaded_bytes`/
  `total_bytes` are real and stream into a genuine progress bar, instead of a queue item jumping
  straight from 0% to 100%.
- **Per-domain concurrency limits and rate limiting**, configurable per plugin (exact-hostname and
  wildcard domain rules, token-bucket rate limiting) and user-overridable through a settings UI —
  something legacy's download plugins never had any way to express or enforce at all.
- **A real, verified bug fix for non-ASCII download filenames.** Some real download servers send a
  legitimate UTF-8 filename directly in a plain `Content-Disposition: filename="..."` header
  (technically not RFC 6266-compliant, but common in practice) — the original filename-parsing
  code used `HeaderValue::to_str()`, which silently fails on any non-ASCII byte, so these downloads
  fell back to a meaningless gallery-ID string as the saved filename. Fixed by reading the header's
  raw bytes, decoding as UTF-8 first and falling back to a Latin-1 byte mapping (which can never
  itself fail) only for genuinely non-UTF-8 servers.
- **Download cancellation is a real, persisted state**, not just a frontend illusion — a
  `Cancelled` state was added to the actual queue-item state machine (surviving a page refresh,
  distinct from `Queued`/`Error`, with its own "已取消/Cancelled" UI treatment), after an earlier
  frontend-only-optimistic-state attempt was found to vanish on refresh.
- **Duplicate in-flight downloads of the same URL are now rejected** (`409`) instead of silently
  allowed to run concurrently with no detection at all, and a running download's queue record can
  no longer be deleted out from under it (previously silently orphaning the background task with
  no way to observe or stop it afterward).

### Error handling & internationalization

- **Every download-queue error is now structured and translatable, never a raw string.**
  `QueueError` (`crates/lanrurugi-core/src/queue_error.rs`) is a closed enum with zero free-text
  fields — only structured, interpolatable data. Every variant gets a stable numeric code (real
  HTTP-equivalent codes like 409/422 where applicable, ≥1000 for pure business errors with no HTTP
  analog), and the frontend renders each `kind` as a real translated string — including turning a
  duplicate-archive error's `existing_id` into an actual clickable link to the existing archive,
  something legacy's plain error text never offered.
- **The plugin SDK gained the same structured-error treatment.** Every plugin-side
  `throw new Error("...")`/`{error: "a string"}` (roughly 40 call sites across the shipped plugin
  set) was converted to a `{error_code, data}` shape, where `error_code` doubles as an i18n lookup
  key rather than opaque English text a non-English-reading user would otherwise be stuck with.

### Plugin sandboxing

- **Per-plugin least-privilege permissions — legacy has no permission model for plugins at all.**
  Each plugin namespace gets its own Deno subprocess, started only with the exact
  network/read/write permissions that plugin itself declares (queried via a throwaway
  zero-permission startup probe before the real worker is spawned) — never a shared process that
  would otherwise grant every plugin the union of every other plugin's permissions.
- **Real path-traversal hardening** on the plugin-namespace parameter, which arrives from an
  unauthenticated HTTP query string (`POST /plugins/use?plugin=...`) — rejects `..`-traversal and
  absolute paths, covered by dedicated tests.
- **One plugin's failure can't take down another's.** A crashed or timed-out plugin call drops
  just that plugin's own worker (respawned lazily on next use); every other plugin's pool is
  unaffected.

### Frontend

- **Centralized route management** (`apps/frontend/src/routes.ts`) replacing previously scattered,
  hand-written path strings — this closed real dead-link bugs, not just a refactor: several pages
  used a `#/reader/{id}`-style hash fragment in an app that uses `BrowserRouter` (not
  `HashRouter`), which only "worked" via a left-click `onClick` handler intercepting the browser's
  default navigation — middle-click-to-open-in-new-tab, right-click-copy-link, and the app's own
  "copy link" button were all genuinely broken until this was centralized and fixed.
- **A rebuilt tag editor** with real removable chips (click-to-remove, Enter/comma-to-commit,
  paste-splits-on-comma, duplicate rejection, autocomplete) replacing a degenerated plain
  `<textarea>` — and, in the process, a real behavioral bug fix: running a metadata plugin now
  correctly saves pending edits first (matching legacy's own real save-then-fetch sequencing),
  which the prior implementation skipped entirely.
- **Search filter state round-trips through the browser's real back/forward history** via React
  Router's own `navigate()`, rather than a `replaceState`-only implementation that couldn't be
  navigated with the back button at all.
- **A full-page zoom preview (lightbox) on the reader overview.** Clicking a magnifying-glass icon
  on any page-grid thumbnail opens a large preview + current-page info (page number, filename,
  resolution, size, chapter) plus a fast-scrubbing, hover-to-preview horizontal filmstrip gallery
  — none of it disturbing the underlying overview modal's own scroll position or actual reading
  progress. Legacy's own thumbnail grid is too small to judge chapter start/end purely by content;
  legacy has no equivalent feature at all.
- **Several real chapter (ToC) management improvements**: delete/edit now lists every chapter in
  the archive to pick from, instead of legacy's own real limitation of only ever operating on
  "whichever chapter the reader is currently scrolled into" (`getCurrentChapter()`, not a porting
  gap); one-click presets (cover/back cover/table of contents/color pages/omake/afterword/
  illustration + a chapter-N dropdown) or number-key shortcuts (0 = table of contents, 1–9 = that
  chapter) for common chapter types; preset-set chapter titles are now stored as reserved internal
  identifiers (`toc`/`c1`–`c20`) and deduped by re-write (re-setting "Chapter 4" moves it instead
  of leaving a stale duplicate behind), with the frontend mapping them back to real localized
  display text — these preset entries can't be renamed through the manual-edit dialog (that would
  break the dedup semantics) and must be deleted and re-applied via a preset instead.
- **`date_added` now supports calendar-day search** (`date_added:2026-07-20`, resolved against a
  server timezone configurable on the Settings page) as the timestamp range covering that day
  00:00–24:00 in that timezone; legacy's own `date_added` is a bare Unix-seconds tag with no
  concept of "by day" search at all. Tag display and its search link now consistently show/resolve
  as `yyyy-mm-dd` in that same timezone.
- **A "Mark as Read"/"Mark as Unread" context-menu item on Library grid cards**, setting reading
  progress directly to the last page or 0 — legacy has no way to manually toggle read status
  outside of actually paging through the archive.
- **AI-assisted Tankoubon editing**, suggesting a title, chapter names, and reading order from
  member archive titles (DeepSeek-backed, multiple candidates, one-click apply), and **AI-assisted
  Tankoubon creation**, analyzing archives not yet in any Tankoubon and suggesting groups that
  likely belong to the same series (local embedding model only, no LLM key required).

### Testing infrastructure

- **A two-layer automated test suite that Phase 1 shipped without.** Vitest + React Testing
  Library for fast, no-backend unit coverage of hooks/logic, and Playwright (Chromium + Firefox)
  for real end-to-end coverage against a live backend + Redis, with per-worker Redis isolation.
  Several suites specifically encode real bugs this project's own manual QA already found and
  fixed, plus systematic fixture coverage across every archive format the scanner supports.
- **A dead test was found and fixed by actually wiring up the test infrastructure it depended on.**
  A contract test kept silently passing (reported `ok`) while actually just skipping, because the
  Redis connection string it needed had never been wired into the containerized test runner — true
  of every Redis-dependent test in the suite until this was fixed. Fixing the wiring (not just the
  one stale test) immediately surfaced that the specific test in question asserted against an
  endpoint that no longer exists (the underlying feature had migrated to a different mechanism),
  which was then replaced with a real equivalent test.

### Job console

- Surfaces the job registry already used internally for backup/restore, thumbnail regeneration,
  duplicate scans, and index rebuilds as a real browsable admin page — live state/progress,
  per-job result/error inspection, filtering — something legacy exposes only through Minion's own
  admin UI, on a separate, additive `/api/jobs*` contract that leaves the legacy-mimicking
  `/api/minion/*` API untouched (Principle II).

### What's deliberately *not* claimed as an improvement (parity by design)

- SHA-1 was kept, not upgraded to a newer hash — see the archive-ID section above; the improvement
  is the size-aware *input*, not the hash primitive itself.
- RAR/7z archives are still handled by shelling out to `unrar`/`7z`, matching legacy's own
  pragmatic approach, not reimplemented from scratch.
- The REST API contract is derived directly from legacy's own OpenAPI spec, additive-only — the
  explicit goal is *not breaking existing third-party clients*, not a redesigned API.
- The search engine is a direct port of legacy's Redis-based model (sorted sets, tag filtering),
  not a new search technology — parity was the actual goal here, evaluated and confirmed sufficient
  at this project's target scale.

## Building and running

Toolchain versions are pinned in `.mise.toml` (`mise install` reproduces them exactly: Rust,
Node, Deno, pnpm, plus `sccache`/`mold` for build acceleration).

```sh
# Backend
cargo build --release -p lanrurugi-server
./target/release/lanrurugi-server serve --redis-url redis://127.0.0.1:6379 \
  --library-path /path/to/existing/library

# Frontend (dev server, proxies /api to the backend above)
cd apps/frontend && pnpm install && pnpm run dev
```

Or via Docker (bundles the built frontend into the same image):

```sh
docker build -t lanrurugi .
docker run -p 3000:3000 -v /path/to/library:/library lanrurugi
```

A fresh instance (or one migrated from a legacy install that never changed its password) starts
with legacy LANraragi's own default admin password still in place. **Change it immediately after
first login** via the Settings page — don't leave a default-password instance reachable from
outside your local network.

### CLI subcommands

- `lanrurugi serve` — runs the HTTP API, static frontend, file watcher, and plugin pool in one
  process.
- `lanrurugi rebuild-index` — recomputes every archive's ID with the size-aware algorithm and
  discovers any previously-invisible files a historical false-merge had hidden (User Story 6).
- `lanrurugi bench` — generates a synthetic library and runs the concurrency/throughput
  comparison against an already-running legacy instance (User Story 8;
  see `specs/001-lanrurugi-full-rewrite/quickstart.md` §8).

## Testing

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
LANRURUGI_TEST_REDIS_URL=redis://127.0.0.1:16379 cargo test --workspace
```

Redis-backed tests are skipped gracefully if `LANRURUGI_TEST_REDIS_URL` is unset; point it at a
scratch Redis instance (e.g. `docker run -d --rm -p 16379:6379 redis:7-alpine`) to run them.

### Frontend tests (`specs/003-ui-test-automation/`)

```sh
mise run test-frontend-unit   # Vitest + React Testing Library — fast, no backend required
mise run test-frontend-e2e    # Playwright — real backend + Redis, Chromium + Firefox
```

`test-frontend-e2e` builds the backend, then starts its own isolated Redis instance, backend
process, and frontend preview server per test worker (see `apps/frontend/tests/e2e/fixtures.ts`),
always starting from a clean state. Set `KEEP=1 mise run test-frontend-e2e` to skip teardown for
one run and inspect its environment afterward (Redis/library state) — this never persists past
that single run. See `specs/003-ui-test-automation/quickstart.md` for the full validation guide.

## Documentation

- [`specs/001-lanrurugi-full-rewrite/`](./specs/001-lanrurugi-full-rewrite/) — Phase 1 spec, plan,
  research decisions, data model, API contracts, and `quickstart.md` (end-to-end validation steps
  for all eight user stories).
- [`specs/002-job-console/`](./specs/002-job-console/) — Phase 1 addendum (additive, implemented):
  background job management console surfacing the existing in-process job registry.
- [`specs/003-ui-test-automation/`](./specs/003-ui-test-automation/) — Phase 1 addendum (additive,
  implemented): Vitest + Playwright automated frontend test coverage — see `## Testing` above.
- [`specs/005-download-plugin-progress/`](./specs/005-download-plugin-progress/) — Phase 1
  addendum (additive, implemented): real byte-level download progress, per-domain concurrency
  limits, and rate limiting for the download-plugin pipeline.
- [`specs/004-ocr-manga-translation/`](./specs/004-ocr-manga-translation/) — Phase 2 (depends on
  Phase 1, does not block it, not yet implemented): optional on-page manga translation via OCR
  detection/recognition, a user-selectable translation backend (cloud or locally-hosted), and
  volume-level font matching.
- [`.specify/memory/constitution.md`](./.specify/memory/constitution.md) — project governance,
  architectural principles, and technology stack decisions.

## License

MIT — see [LICENSE](./LICENSE).
