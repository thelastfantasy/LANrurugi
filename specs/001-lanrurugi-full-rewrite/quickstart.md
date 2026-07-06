# Quickstart: Validating LANrurugi Phase 1

Prerequisites: a working legacy LANraragi instance with a real (or representative sample) Redis
data store and archive folder, per constitution Principle I. `mise install` to get the pinned
Rust/Node/Deno toolchain (Technology Stack Constraints). Docker available for the Debian-slim
image build if validating the containerized path.

## 1. Build and point at an existing library (US1)

```
mise run build          # or: cargo build --release && (cd frontend && npm run build)
lanrurugi serve --redis-url <same Redis instance the legacy system uses> \
                 --library-path <same archive folder the legacy system uses>
```

**Expected**: every archive already known to the legacy system appears in LANrurugi's UI with its
prior title/tags/categories/groupings/reading-progress intact (SC-001). No conversion step is
required before this works. See `data-model.md` for the entity shapes being read as-is.

## 2. Ingest new archives without false merges (US2)

```
cp two-archives-sharing-a-cover-but-different-content*.zip <library-path>/
```

**Expected**: within 60 seconds both appear as two distinct, independently taggable entries
(SC-002, SC-003) — not collapsed into one. Confirm via `GET /archives` (`contracts/rest-api.md`).

## 3. Existing third-party client compatibility (US3)

Point any existing LANraragi-compatible client (or a saved set of recorded legacy API
requests/responses) at LANrurugi's `/api` using a previously-issued API key.

**Expected**: list/search/fetch-page/update-tag flows behave identically (SC-004). Any deviation
from `contracts/rest-api.md`'s "Existing contract" section is a Phase 1 regression, not an
acceptable behavior change.

## 4. Metadata enrichment plugin (US4)

Enable a sample metadata plugin (declares `net` permission to a fake/local metadata endpoint per
`contracts/plugin-protocol.md`), run it against an untagged archive.

**Expected**: tags/title/summary populate from the plugin's result. Then deliberately point the
plugin at an unreachable/timeout-inducing endpoint and confirm the failure is isolated (rest of
the system keeps working) and reported, not silently swallowed or crash-inducing.

## 5. Backup and restore (US5)

```
POST /database/backup      # trigger
GET  /database/backup/{jobid}   # poll until done, download resulting JSON
```

Restore that JSON onto a fresh LANrurugi instance pointed at the same archive files.

**Expected**: tags/categories/groupings/reading-progress on the fresh instance match the backup
source exactly (SC-009). Shape reference: `data-model.md`'s "Backup/Export document".

## 6. Duplicate repair (US6)

Against a library known (or seeded) to contain a legacy false-merge pair:

```
POST /database/rebuild-index
GET  /minion/{jobid}   # or the job-status path this maps to, per contracts/rest-api.md
```

**Expected**: the previously-hidden archive appears as a new, unfilled entry; the originally
tracked archive keeps its existing tags/progress unchanged (SC-005). See data-model.md's
"Rebuild/Reindex operation" for exactly what should and shouldn't change.

## 7. Interface localization (US7)

Switch the UI language setting to any of the 14 legacy-supported languages (verified list:
`en`, `ja`, `zh`, `zh_Hant`, `ko`, `fr`, `de`, `es`, `it`, `pt`, `vi`, `id`, `nb_NO`, `as`).

**Expected**: menus/labels/messages render in that language (SC-010); a deliberately-untranslated
string falls back to English rather than appearing blank.

## 8. Concurrency benchmark (US8)

```
lanrurugi bench --library-scale 100000   # or a smaller --library-scale for a fast local run
```

**Expected**: a report matching `contracts/benchmark-report.md`'s shape is produced, covering at
least full-library-scan/ingestion and duplicate-repair-reindex, with the new system's numbers
faster than the legacy run on the same hardware (SC-011). On a single-core machine, the command
still completes and produces a report (Edge Cases in spec.md), just with a smaller margin.

## Non-goals for this quickstart

Nothing from User Stories 9–10 (Phase 2 translation) is exercised here — that gets its own
quickstart once Phase 2 has its own plan, per constitution Principle VI.
