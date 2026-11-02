# Implementation Plan: Recycle Bin and Safety Password

**Branch**: `019-recycle-bin-safety-password` | **Date**: 2026-10-31 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/019-recycle-bin-safety-password/spec.md`

## Summary

Deleting an archive stops being a one-way destruction. The delete path (`archives.rs::delete_one_archive`) is
split into a reusable "purge the record and its caches" tail plus a new "move the file into the bin and write a
recoverable entry" front half. Bin payloads live in a reserved directory inside the library
(`<library>/.lanrurugi-recycle/<archive_id>/`), moved by rename, with a Redis hash as the authoritative
inventory and a per-entry `meta.json` as an on-disk journal for crash recovery. Restore moves the file back,
re-creates the archive record from the snapshot, and re-applies the categories, bookmarks, stamps and reading
progress that deletion currently destroys.

A four-digit safety password — bcrypt-hashed via the existing `lanrurugi_core::password` helper, never echoed —
gates the two irreversible actions (permanent delete, empty bin), verified on every request, with a
per-session-family lockout after five failures. It can be set or changed from Settings, reset with the login
password, or cleared from a new `lanrurugi reset-safety-password` CLI subcommand.

The bin is bounded by two new numeric settings (`recycle_retention_days`, default 30, 0 = forever;
`recycle_max_size_gb`, default 0 = unlimited) enforced by an hourly in-process sweep plus immediate
enforcement when a setting is tightened. The web UI gains a `/recycle-bin` page, a nav entry, an
"undo" action on the delete toast, and a Settings section showing usage and the two limits.

## Technical Context

**Language/Version**: Rust 2021 (workspace toolchain pinned by `.mise.toml`), TypeScript 5.x + React 19

**Primary Dependencies**: Tokio, Axum, deadpool-redis, `bcrypt` (via `lanrurugi_core::password`), `serde`/
`serde_json`, `tracing`; frontend: TanStack Query, Zustand, i18next, Vite, Tailwind (all already present)

**Storage**: Redis only (constitution: no second store). New additive keys — `LANRURUGI_RECYCLE_BIN` on the
`archive` logical DB, safety-password fields in `LRR_CONFIG` plus short-TTL keys on the `config` DB — and the
filesystem for payload bytes inside the library tree.

**Testing**: `mise run test` (cargo test workspace), `mise run check-crate -- <crate>` while iterating; Vitest +
React Testing Library for UI units; Playwright (Chromium + Firefox) for the delete → undo → restore → PIN →
empty journey; `lanrurugi-api-docgen`'s existing route-scan gate for the new endpoints.

**Target Platform**: Linux server (Docker image and native packages), self-hosted single-admin deployment

**Project Type**: Web service (Axum) + SPA (React), one binary and one frontend app

**Performance Goals**: delete and restore are single renames (no byte copy) and must stay under 2 s for a 1 GiB
archive on local disk; the bin listing is a single `HGETALL` plus a directory stat per entry and must stay under
200 ms for 200 entries; the sweep is O(n) over entries and must not block request handling.

**Constraints**: Existing archive-delete response shapes keep every current field (new information additive
only); no second data store; no new long-running process; the bin directory must be invisible to the file
watcher, the full scan and both subfolder scripts; a recycled id must behave exactly like a nonexistent one
everywhere except the bin endpoints.

**Scale/Scope**: One library, one administrator, up to a few thousand recycled entries in pathological use;
defaults bound the bin to a 30-day window with no size cap.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | How this plan satisfies it |
|---|---|---|
| I. Legacy data & user-trust compatibility | PASS | Additive `LANRURUGI_*` keys only; archive records, tags, categories and reading progress are never re-keyed. The bin snapshot re-creates records with their original identity (content hash unchanged) and restores legacy-shaped fields verbatim. No new identity algorithm. |
| II. API contract fidelity | PASS (documented behaviour change) | `DELETE /api/archives/{id}` and `DELETE /api/archives` keep their exact response fields; `recycled: true` is added. All new routes are additive and documented through the 011 docgen sidecar, which fails the gate if a route is served but undocumented. The on-disk effect of deleting changes (the file is retained in the bin) — explicitly recorded in spec Assumptions and here, per the "compatibility-affecting changes require sign-off" rule. |
| III. Single-process, resource-conscious, genuinely concurrent | PASS | The sweep is another `tokio::time::interval` task in `main.rs`, no new process. Delete/restore are I/O-bound and run on the async runtime; the only CPU work is hashing a 4-digit PIN at bcrypt cost, off the hot path. |
| IV. Plugin extensibility | PASS (untouched) | No plugin protocol, pool or permission change. |
| V. Secrets & network trust boundaries | PASS | The safety password is stored only as a bcrypt hash in `LRR_CONFIG`, never returned by any endpoint, never logged, never put in an activity snapshot. Verification is server-side only. |
| VI. Phased scope discipline | PASS | A Phase 1 archive-management feature; no OCR/translation/LLM concerns. |
| VII. Frontend discipline & legacy fidelity | PASS | New page follows the `pages/<Name>/index.tsx` + sibling-components rule; no inline breakpoint styles; strings go through i18next with the 14 locale files; this is a new page, not a port of a legacy screen, so no computed-style comparison applies; dialogs implement focus trap/Escape/focus return. |
| Tech stack: Redis-only, newtype IDs, shared helpers | PASS | `RecycleEntry` is keyed by `ArchiveId` (newtype, `#[serde(transparent)]`); the repository follows the existing `api_tokens.rs`/`download_queue.rs` shape and reuses the shared list/JSON helpers rather than hand-rolling a fourth variant. |

No violations; Complexity Tracking stays empty.

## Project Structure

### Documentation (this feature)

```text
specs/019-recycle-bin-safety-password/
├── plan.md              # This file
├── research.md          # Phase 0 output — the decisions below, with alternatives
├── data-model.md        # Phase 1 output — entry/JSON/keys/state machine
├── quickstart.md        # Phase 1 output — runnable validation scenarios
├── contracts/
│   ├── http-api.md      # endpoint table, request/response, errors, auth roles
│   └── ui.md            # page/dialog behaviour and deletion-flow integration
└── checklists/requirements.md
```

### Source Code (repository root)

```text
crates/lanrurugi-storage/src/
├── recycle.rs               # NEW — RecycleEntry, RecycleRepository (Redis hash + fs helpers)
├── activity.rs              # action types: archive.recycle/restore/purge, recycle.empty
└── keys.rs                  # new key constants

crates/lanrurugi-api/src/
├── recycle.rs               # NEW — endpoints, delete/restore/purge orchestration, sweep, reconcile
├── safety_password.rs       # NEW — set/change/reset, per-request verification, lockout
├── archives.rs              # delete_one_archive split: recycle vs purge, response field, thumbnail keep
├── state.rs                 # RecycleRepository + bin-mutation lock in AppState/Repositories
├── settings.rs              # recycle_retention_days / recycle_max_size_gb fields + enforcement hook
├── policy/route_policy.csv  # explicit denies for both token roles on the new routes
└── lib.rs                   # router registration

crates/lanrurugi-scanner/src/watcher.rs   # is_watched_archive_path excludes the bin directory
crates/lanrurugi-server/src/main.rs       # hourly sweep task; reset-safety-password subcommand

apps/frontend/src/
├── components/common-ui/Form/OTPField.tsx   # shared single-input segmented code field
├── pages/RecycleBin/        # index.tsx, RecycleRow.tsx, SafetyPasswordDialog.tsx, RecycleBinToolbar.tsx
├── pages/Settings/RecycleBinSection.tsx
├── api/hooks.ts             # recycle-bin + safety-password queries/mutations
├── api/recycle.ts           # request helpers/types
├── Layout.tsx, App.tsx      # nav link + route
├── dialog.tsx / toast.tsx   # undo action support for the delete toast
└── i18n/locales/*.json      # 14 locale files

apps/frontend/public/legacy/themes/*.css   # five theme blocks for the OTP field classes

docs/api/sidecar/recycle-bin.yaml   # docgen sidecar for the new routes
```

**Structure Decision**: Backend follows the existing per-domain module layout (`recycle.rs` beside
`archives.rs`/`categories.rs`); storage follows the one-repository-per-domain convention; frontend follows the
`pages/<Name>/index.tsx`-only-component rule from Principle VII.

## Complexity Tracking

None — no constitution violations, no new store, no new process, no new dependency.

## Phase 0 — Research (see [research.md](./research.md))

All technical unknowns are resolved there; the load-bearing decisions are: bin inside the library with a single
path-exclusion choke point, Redis inventory + `meta.json` journal, rename-then-record ordering with documented
crash windows, per-request PIN verification with session-family lockout, retention/capacity policy with
"newest never evicted", and thumbnail retention keyed by the recycled id.

## Phase 1 — Design (see [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md))

Entities: `RecycleEntry`, `SafetyPassword` (hash + lockout), `RecyclePolicy` (two settings). Contracts: HTTP
endpoint table with roles and error codes, and the UI behaviour contract covering the delete flow, undo,
permanent-delete/empty dialogs and the settings section.

## Post-Design Constitution Re-check

Re-evaluated after the Phase 1 artifacts: unchanged and still PASS. The design adds one storage module, one API
module, one CLI subcommand, one frontend page and one settings section; it adds no dependency, no process, no
second datastore, and changes no existing response shape beyond an additive field.
