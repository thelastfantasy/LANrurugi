# Implementation Plan: Download Plugin Progress, Concurrency & Rate Limiting

**Branch**: `005-download-plugin-progress` | **Date**: 2026-07-19 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/005-download-plugin-progress/spec.md`

## Summary

Enhance the Phase 1 download-plugin pipeline so that the actual byte-level HTTP transfer of a
downloaded archive — which today happens nowhere at all (`execDownload`'s `download_url` result is
stored as-is by the Rust host and never fetched; the real fetch, if any, happens inside the Deno
plugin process itself and is invisible/uncontrollable from Rust) — moves into Rust itself via
streaming `reqwest` requests. This is what makes real progress reporting, per-domain concurrency
limiting, and rate limiting possible at all: none of the three can be meaningfully implemented
where the transfer isn't actually happening.

Concretely: `execDownload`'s return contract gains a `downloads: {url, method?, headers?,
filename_hint?}[]` field (one element = single-file download; more than one = a multi-resource
download, e.g. Pixiv's per-page images). A new, parallel `pluginOptions()` export lets a download
plugin declare its own default per-domain concurrency limit, rate limit, and (for multi-resource
plugins) whether to bundle the downloaded resources into one archive or catalog each separately.
User overrides of these settings are persisted in Redis and surfaced via a new settings UI. A new
Rust-side download-manager component enforces per-domain concurrency (LastPass-style exact/
wildcard domain rules, exact taking precedence) and rate limiting (same domain-rule mechanism,
plus a general fallback) for every Rust-initiated download, and streams live
`downloaded_bytes`/`total_bytes` progress into the existing `JobRegistry`, which the existing
`useJobs()`-polling Jobs page renders as a real progress bar. The pre-existing `file_path` return
field remains as an unmanaged fallback escape hatch. Three existing hand-written plugins
(`chaika.ts`, `ehentai.ts`, `pixiv.ts`) are migrated to the new contract as part of this feature —
Pixiv in particular drops its own Deno-side `Archive::Zip`-equivalent per-page zipping in favor of
returning per-page `downloads[]` entries and letting Rust do the bundling.

## Technical Context

**Language/Version**: Rust (stable channel, pinned via `mise` — matching 001's existing pin) for
the new download-manager/streaming-download backend work; TypeScript (Deno-targeted, matching
001's existing plugin runtime) for the three updated plugins and the new `pluginOptions()` SDK
surface; TypeScript (React 19) for the new frontend settings UI and Jobs-page progress bar.

**Primary Dependencies**:
- Backend: `reqwest` (already a workspace dependency per `Cargo.toml`, currently `default-features
  = false` with `json`/`rustls-native-certs`/`multipart` — this feature is the first real
  `lanrurugi-api` caller and needs streaming response bodies, so the `stream` feature must be
  added; see research.md for the exact feature-flag delta) for the actual outbound HTTP GET/other-
  method download; a Rust zip-writing library (choice finalized in research.md) for the
  `bundle_as_archive: true` multi-resource case; `tokio::sync::Semaphore` (per-domain concurrency)
  and a token-bucket rate limiter (crate choice finalized in research.md) for the download-manager
  component; `deadpool_redis` (already used throughout `lanrurugi-storage`) for persisting
  `pluginOptions()` user overrides.
- Plugin SDK: no new runtime dependency — `pluginOptions()` is a plain additional `export function`
  in the same Deno-executed `.ts` plugin files, following the exact pattern `pluginInfo()` already
  establishes (`crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`).
- Frontend: no new dependency — the settings form and Jobs-page progress bar reuse the existing
  React 19 + TanStack Query + Tailwind stack and the existing `useJobs()` polling hook
  (`apps/frontend/src/api/hooks.ts`); TanStack Query already re-renders on each poll tick, so a
  progress bar driven by `downloaded_bytes`/`total_bytes` needs no new state-management mechanism.

**Storage**: Redis (reused as-is, per constitution Principle I) — a new hash/key namespace for
per-plugin `pluginOptions()` user overrides, alongside the existing `JobRegistry` in-memory job
tracking (already Redis-independent — jobs are process-lifetime, not persisted across restarts,
per existing `lanrurugi-core::jobs` behavior; this feature does not change that).

**Testing**: `cargo test` (new unit tests for domain-rule matching precedence, the token-bucket
rate limiter, and the download-manager's concurrency-gating logic; integration tests driving a
real streaming download against a local test HTTP server); Deno's own `deno check`/`deno test` for
the three updated plugins (matching the existing `mise run test-frontend-*`-adjacent plugin-
verification convention already established for `plugins/*.ts` — see `scripts/convert-plugins.sh`'s
own `deno check` gate); Playwright, per 003-ui-test-automation's now-established E2E layer, for the
new settings-UI and Jobs-page-progress-bar user journeys.

**Target Platform**: Linux server (unchanged from 001 — this feature adds to the existing single
binary/single deployable, no new target).

**Project Type**: Web application (unchanged from 001 — adds to the existing Rust backend +
React SPA frontend, not a new deployable).

**Performance Goals**: SC-001 (at least 3 distinct intermediate progress states visible during a
≥50MB download — implies a progress-update interval on the order of low seconds, not a single
end-of-transfer callback); SC-005 (sustained throughput within 10% of a configured rate-limit cap
over a download's full duration).

**Constraints**: Constitution Principles I (no destructive Redis-data change — new
`pluginOptions()` override storage is purely additive, existing plugin/job data untouched), III
(the new download-manager's concurrency/rate-limiting MUST be implemented as genuinely async Tokio
work — per-domain semaphores and a token-bucket limiter are exactly the kind of I/O-bound
concurrency control Principle III already mandates `tokio` for, not a CPU-bound `rayon` concern),
IV (this feature does not change the Deno plugin sandbox's permission model — `pluginOptions()` is
additive metadata a plugin exports, not a new permission grant; the plugin itself never performs
the real byte-level fetch once migrated to `downloads[]`, so no new `--allow-net` surface is needed
per plugin beyond what already exists for `execDownload`'s own logic).

**Scale/Scope**: 4 user stories (P1 progress, P2 concurrency, P3 rate limiting, P2 settings UI), 18
functional requirements (FR-001–FR-018), 3 key entities (Download Job extension, Download Plugin
Settings, Domain Rule), 3 existing plugins migrated (chaika.ts, ehentai.ts, pixiv.ts). Single-owner/
single-instance deployment scope, matching 001 — no multi-tenant concerns.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Legacy Data & User-Trust Compatibility | New Redis keys (plugin settings overrides) must be additive, no existing key shape changed | **PASS** — `Download Plugin Settings` overrides are a new, separate Redis namespace; `JobStatus`'s new `downloaded_bytes`/`total_bytes` fields are additive to an existing in-memory (non-Redis-persisted) struct, not a stored-data shape change |
| II. API Contract Fidelity (Phase 1) | Existing `/api/*` endpoints (notably `download_url`) must keep working unmodified for existing third-party clients | **PASS** — `POST /download_url`'s request/response shape is unchanged; this feature only changes what happens *inside* the job it already creates, plus adds new, additive endpoints (plugin-settings CRUD) rather than altering the existing one |
| III. Resource-Conscious, Genuinely Concurrent Architecture | New download-manager concurrency/rate-limiting must be genuinely async (Tokio), not thread-per-download or blocking | **PASS** — per-domain `tokio::sync::Semaphore` + async token-bucket limiter + `reqwest`'s async streaming API are the design (see Technical Context); no new OS-thread-per-download or blocking I/O introduced |
| IV. Sandboxed, Language-Agnostic Plugin Extensibility | Plugin sandbox model / permission grants must not be weakened | **PASS** — `pluginOptions()` is inert metadata returned by the same sandboxed subprocess; migrating `execDownload` to return `downloads[]` instead of doing its own `fetch()` *reduces* what a download plugin needs `--allow-net` for, not expands it |
| V. Secrets & Network Trust Boundaries | N/A (Phase 2-only) | **N/A** — this feature is Phase 1 scope only, no LLM/cloud-provider trust boundary involved |
| VI. Phased Scope Discipline | Must not smuggle Phase 2 concerns in, must not block Phase 1 | **PASS** — this is a Phase 1 addendum (like 002/003), independent of Phase 2's OCR/translation work; does not block or get blocked by 004 |

No violations requiring Complexity Tracking.

## Project Structure

### Documentation (this feature)

```text
specs/005-download-plugin-progress/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-api/
│   └── src/
│       ├── plugins.rs           # existing download_url handler — extended to consume
│       │                        # `downloads[]`/pluginOptions() instead of storing download_url
│       │                        # as-is; new plugin-settings CRUD endpoints
│       └── download_manager/    # NEW — per-domain concurrency (Semaphore pool + wildcard/exact
│           ├── mod.rs           # domain-rule matching), token-bucket rate limiting, and the
│           ├── domain_rules.rs  # streaming reqwest download + progress-callback plumbing into
│           └── rate_limit.rs    # JobRegistry::set_download_progress
├── lanrurugi-core/
│   └── src/
│       └── jobs.rs              # JobStatus gains downloaded_bytes/total_bytes; new
│                                 # set_download_progress(id, downloaded, total) method
├── lanrurugi-plugin/
│   └── dispatcher/
│       └── plugin-sdk.ts        # DownloadResult gains `downloads[]`; new PluginOptionsResult
│                                 # type + `pluginOptions()` documented alongside `pluginInfo()`

plugins/download/
├── chaika.ts                     # download_url: string -> downloads: [{url}]
├── ehentai.ts                    # download_url: string -> downloads: [{url}]
└── pixiv.ts                      # drops Deno.writeFile/zip-equivalent logic; returns
                                  # downloads: [{url, headers: {Referer}}, ...] per page;
                                  # declares pluginOptions() with bundle_as_archive: true default

apps/frontend/
└── src/
    ├── pages/
    │   └── Jobs.tsx              # download-type jobs render a real progress bar from
    │                             # downloaded_bytes/total_bytes instead of the current jump
    ├── pages/Plugins/            # (or wherever the existing plugin-management page lives —
    │                             # confirmed in research.md) new per-plugin settings form,
    │                             # shown only when pluginOptions() returns configurable fields
    └── api/
        ├── hooks.ts              # new hooks for reading/writing persisted plugin settings
        └── types.ts              # new types mirroring PluginOptionsResult + extended JobStatus
```

**Structure Decision**: Adds to the existing Phase 1 Cargo workspace/frontend app per constitution
Technology Stack Constraints (no new crate at the top level required — the download-manager is a
new module inside the existing `lanrurugi-api` crate, since it's tightly coupled to that crate's
existing `AppState`/job-registry/plugin-pool wiring rather than a reusable standalone library).
Three existing plugin files are edited in place; no new plugin files created.

## Complexity Tracking

*No Constitution Check violations — table omitted.*
