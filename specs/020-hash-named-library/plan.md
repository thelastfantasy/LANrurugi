# Implementation Plan: Hash-Named Library

**Branch**: `020-hash-named-library` | **Date**: 2026-11-08 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/020-hash-named-library/spec.md`

## Summary

An archive's bytes stop being stored under a name derived from its title. Ingest computes the content-derived id first
and stores the file at `<library>/<ab>/<id>.<ext>`, where `ab` is the first two characters of the id; the human name
becomes the `original_filename` metadata field, and the title defaults to that filename with release markers stripped by
the server itself (`lanrurugi-scanner`'s existing `release_markers` module) rather than by a plugin. The whole
filename-conflict state machine — `PendingFilenameConflict`, `stage_pending_rename`,
`sweep_stale_pending_renames`, `unique_dest_path`, `ConflictPolicy::AutoRename`, the rename popover, the conflict menu
and the filename template — is deleted, together with `PUT /api/archives/{id}/rename` (an endpoint legacy never had).

Each stored archive gets an atomically written sidecar `<ab>/<id>.json` recording what the database knows (id, original
filename, original relative path, title, extension, size), so a flushed Redis can be rebuilt from the files alone.
Existing archives are moved, renamed to the same shape and given sidecars by an explicit, previewable, resumable,
reversible background job that relocates them to `<library>/.classic/<ab>/<id>.<ext>`. Because the id is content-derived
and does not change, categories, tankoubons, bookmarks, stamps and reading progress are untouched — this is a move, not
a re-import.

## Technical Context

**Language/Version**: Rust 2021 (workspace toolchain from `.mise.toml`), TypeScript 5.x + React 19

**Primary Dependencies**: Tokio, Axum, deadpool-redis, serde/serde_json, `sha1` (via `lanrurugi-storage`),
`lanrurugi-scanner::release_markers`, `lanrurugi_core::jobs` (the existing in-process job registry); frontend:
TanStack Query, Zustand, i18next, Vite, Tailwind (all present)

**Storage**: Redis for archive records (one new optional field on each archive hash: `original_filename`; no new key
namespace) and the library filesystem for bytes, sidecars and the migration journal. No second datastore.

**Testing**: `mise run test-crate -- lanrurugi-api` / `-- lanrurugi-scanner` / `-- lanrurugi-storage` while iterating,
`mise run test` before the PR; Vitest for the upload-queue logic that remains; Playwright for the migration settings
flow and the "archive edit page has no filename field" assertion.

**Target Platform**: Linux server (Docker image, native packages), self-hosted single-admin deployment; library on a
local filesystem or ZFS dataset, optionally shared over SMB.

**Project Type**: Web service (Axum) + SPA (React), one binary and one frontend app.

**Performance Goals**: ingest must add no measurable delay beyond one `rename` (the id is computed from the staged
file that is already read for hashing); scanning 100,000 archives must stay within the current full-scan budget, with
bucket lookups replacing flat readdir; the migration must move ~10,000 archives per minute on the same filesystem
(rename-only) and must not block request handling.

**Constraints**: No rekey — ids, categories, tankoubons, progress and share references must be unchanged by the
migration. The only name the application constructs is `{id}.{ext}` plus a two-character directory, so no
filesystem name-length limit can be reached. The watcher and the sweep must never treat a sidecar or a journal as an
archive. The migration must be safe to interrupt at any point and must be reversible from its journal.

**Scale/Scope**: 10,000–100,000 archives in one library; 256 buckets (~400 entries each at 100k); one administrator;
one migration at a time.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | How this plan satisfies it |
|---|---|---|
| I. Legacy data & user-trust compatibility | PASS (documented, user-approved behaviour change) | Ids and every ID-keyed reference are unchanged; legacy archive records keep their fields and gain one optional `original_filename`. The on-disk name changes for existing files, which is the user's explicit decision, and the migration is previewable and reversible. Legacy backup import switches from matching by on-disk name to matching by `original_filename`, which the migration records for every archive. |
| II. API contract fidelity | PASS (one removal, documented) | `PUT /api/archives/{id}/rename` is removed — it is additive API this project introduced and legacy never served it (`crates/lanrurugi-api/src/archives.rs:1837`). No legacy-shaped response changes. The three migration endpoints are additive and go through the docgen sidecar. |
| III. Single-process, resource-conscious, genuinely concurrent | PASS | The migration is a job in the existing in-process registry, not a new process; renames are I/O-bound and dispatched on the blocking pool with a bounded concurrency (the workspace's ~30% cap), and it yields between batches. |
| IV. Plugin extensibility | PASS | Title cleaning stops depending on a plugin but the plugin protocol is untouched; plugins may still override title/tags through the existing metadata path. |
| V. Secrets & network trust boundaries | PASS | No credential moves; sidecars contain only metadata already served to any reader of the library. |
| VI. Phased scope discipline | PASS | Phase 1 archive-storage work; no OCR/translation/LLM concern. |
| VII. Frontend discipline & legacy fidelity | PASS | The change removes UI rather than adding a ported screen; remaining dialogs follow the focus/Escape/focus-return rules; strings go through i18next (en/zh/ja). |
| Tech stack: Redis-only, newtype IDs, shared helpers | PASS | `ArchiveId` newtype stays the key; the sidecar carries the id as a string only at the file boundary; JSON helpers are the shared ones already used by `download_queue`. |

No violations; Complexity Tracking stays empty.

## Project Structure

### Documentation (this feature)

```text
specs/020-hash-named-library/
├── plan.md              # This file
├── research.md          # Phase 0 output — decisions and alternatives
├── data-model.md        # Phase 1 output — record fields, sidecar schema, migration journal
├── quickstart.md        # Phase 1 output — runnable validation scenarios
├── contracts/
│   ├── http-api.md      # migration endpoints, removed rename endpoint, download naming
│   └── sidecar.md       # on-disk sidecar format and its compatibility rules
└── checklists/requirements.md
```

### Source Code (repository root)

```text
crates/lanrurugi-storage/src/
├── id.rs                    # unchanged algorithm; add bucket/relative-path helpers
├── repository.rs            # archive record: original_filename field; retire filename-based lookups used by import
├── sidecar.rs               # NEW — sidecar read/write (atomic), schema version, ignore rules
└── keys.rs                  # LRR_FILEMAP semantics unchanged; documented

crates/lanrurugi-scanner/src/
├── pipeline.rs              # ingest path: id-first destination, title from release_markers
├── full_scan.rs             # walk: skip sidecars/journals/temp; bucket-agnostic
├── watcher.rs               # is_watched_archive_path: exclude *.json sidecars and journal files
├── patch.rs                 # patch_path_for works unchanged for {id}.patch.zip
└── title.rs                 # NEW — title_from_filename() using release_markers, with empty-result fallback

crates/lanrurugi-api/src/
├── download_manager/ingest.rs   # remove conflict state machine; id-first catalogue; write sidecar
├── download_queue.rs            # remove conflict resolution endpoints and error variants
├── archives.rs                  # remove PUT /archives/{id}/rename; delete_one_archive keeps working
├── migration.rs                 # NEW — preview/execute/rollback endpoints + job
├── search.rs                    # unchanged (already hides empty file)
└── lib.rs                       # router registration

crates/lanrurugi-server/src/main.rs   # migration job wiring; rebuild-index reads sidecars

apps/frontend/src/
├── pages/Upload/FilenameTemplateEditor.tsx   # DELETED (template + conflict menu + rename popover)
├── pages/Upload/QueueItemRow.tsx             # conflict affordances removed
├── pages/Library/ArchiveEdit*                # filename field removed; title editing kept
├── pages/Settings/MigrationSection.tsx       # NEW — preview, run, progress, rollback
└── api/                                      # migration hooks; rename API removed
```

**Structure Decision**: storage keeps owning records and ids; the scanner owns how a file becomes an archive; the API
owns HTTP and the job; the frontend loses the conflict UI and gains one settings section. No new crate.

## Complexity Tracking

None. The net change is a deletion of one state machine plus one new job and one small module.

## Phase 0 — Research (see [research.md](./research.md))

Load-bearing decisions: two-character fanout; per-archive sidecar over a single map; `.classic` archives also renamed to
`{id}.{ext}`; the sample-hash id algorithm is kept and duplicate content is rejected; titles are cleaned in Rust with
`release_markers`; hand-dropped files stay in place; the migration is a journaled rename job, not a copy.

## Phase 1 — Design (see [data-model.md](./data-model.md), [contracts/](./contracts/), [quickstart.md](./quickstart.md))

Entities: `Archive` (id, `original_filename`, title, extension, path), `Sidecar` (versioned document beside the bytes),
`MigrationPlan` (move/merge/skip entries), `MigrationJournal` (one line per completed rename, enough to reverse it).
Contracts: the three migration endpoints and their error shapes; the removed rename endpoint; the sidecar format and
its forward-compatibility rule.

## Post-Design Constitution Re-check

Re-evaluated after the Phase 1 artifacts: unchanged and still PASS. The design removes one endpoint and one state
machine, adds one optional archive field, one storage module, one job and one settings section; it adds no dependency,
no process and no second datastore, and it changes no id.
