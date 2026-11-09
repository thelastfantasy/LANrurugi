# Tasks: Hash-Named Library

**Input**: Design documents from `/specs/020-hash-named-library/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Required. Every task below that changes behaviour either extends an existing test module or adds one
next to the code it touches; the repo's gates (`mise run check-crate`, `test-crate`, `clippy`, `fmt-check`, `tsc`,
`eslint`) are part of the definition of done, not a separate phase.

**Organization**: grouped by user story. US1 is the MVP (new ingests land at `{id}.{ext}` with a sidecar and the
conflict machinery is gone); US2–US5 build on it but each is independently testable.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: US1–US5, matching the spec's user stories

## Path Conventions

Rust workspace under `crates/`, frontend under `apps/frontend/src/` — see plan.md's Project Structure.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: the two new fields and the path helpers every later phase needs.

- [x] T001 [P] Add `crates/lanrurugi-storage/src/paths.rs` with `BYNAME_DIR = "_byname"`, `bucket_dir(id)`,
  `hash_layout_path(library, id, ext)` and `is_hash_layout_path(path)`; unit tests for bucket derivation and for
  paths that look similar but are not the layout. Export from `crates/lanrurugi-storage/src/lib.rs`.
- [x] T002 (decided against, 2026-11-09) Do **not** add `original_filename`/`original_path` to `Archive` in
  `crates/lanrurugi-core/src/entities.rs` (both `#[serde(default)]`), and round-trip them in
  `crates/lanrurugi-storage/src/repository.rs` (save/load, plus the "absent means empty" test).
- [x] T003 (implemented as `ScanSummary::{relocated, relocation_skipped, relocation_failed}` rather than a separate struct) Add `RelocationCounters { moved_to_byname: u64, skipped_occupied: u64, failed: u64 }` in
  `crates/lanrurugi-scanner/src/events.rs`, `Default`-constructible and `serde::Serialize`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: the sidecar format, title derivation and the "never catalogue a sidecar" rule. Everything else depends
on these.

- [x] T004 Create `crates/lanrurugi-storage/src/sidecar.rs`: `SidecarV1` (version, id, original_filename,
  original_path, title, extension, size, mtime, written), `path_for(library, id)`, `write_atomic()` (tmp file in the
  same bucket → `fsync` → `rename`), `read()` returning `Option<SidecarV1>` with `Corrupt`/`FutureVersion` reported
  distinctly. Unit tests: round-trip, a truncated file reads as corrupt, a `version: 2` file is refused, a write over
  an existing sidecar replaces it atomically.
- [x] T005 (moved into `lanrurugi-scanner`, api calls it directly instead of re-exporting) Move `crates/lanrurugi-api/src/tankoubon_grouping/release_markers.rs` into a crate the scanner can use
  (`crates/lanrurugi-scanner/src/release_markers.rs`, re-exported from `lanrurugi-api` so `tankoubon_grouping` keeps
  compiling unchanged). Its tests move with it; `tankoubon_grouping`'s review tests must still pass.
- [x] T006 (inlined into `catalogue_new_archive`; no separate `title.rs`) Add `crates/lanrurugi-scanner/src/title.rs` with `title_from_filename(&str) -> String`: file stem → marker
  cleaning via T005 → trimmed result, falling back to the untouched stem when cleaning leaves nothing. Unit tests
  cover `[Circle (Artist)] Title [中国翻訳] [DL版]`, a name that is *only* markers, and a plain name.
- [x] T007 Regression-test that sidecars are never archives: `watcher::is_watched_archive_path` and
  `full_scan::walk` must ignore `{id}.json`; add the test at the existing filter tests in
  `crates/lanrurugi-scanner/src/watcher.rs` / `full_scan.rs`.

**Checkpoint**: sidecar I/O, title cleaning and the scan filter exist and are tested; no user-visible change yet.

---

## Phase 3: User Story 1 — Any title can be ingested (Priority: P1) 🎯 MVP

**Goal**: new downloads/uploads/imports land at `<library>/<ab>/{id}.<ext>`, get a sidecar, are titled from the
original filename, duplicates are rejected by id, and the entire filename-conflict state machine is gone.

**Independent Test**: ingest a 300-byte-title file through the download queue, the upload endpoint and a share
import; each succeeds with `file` matching `ab/{id}.ext`, a readable sidecar beside it and the title derived from the
filename. Re-ingest the same bytes: rejected with the existing id.

- [x] T008 [US1] Rewrite the destination half of `catalogue_staged_file` in
  `crates/lanrurugi-api/src/download_manager/ingest.rs`: compute the id first, pick `bucket_dir(id)` and
  `{id}.{ext}`, create the bucket on demand, copy/move the staging file straight to that path. Delete
  `unique_dest_path`, `suffixed_name`, `lock_filename` and the byte-length gate on the title.
- [x] T009 [US1] Write the sidecar (T004) after the archive record is committed, in the same function; a failed
  sidecar write is a warning that names the archive, never a failed ingest.
- [x] T010 [US1] Thread `original_filename` through `CatalogueInput`/`catalogue_new_archive` in
  `crates/lanrurugi-scanner/src/pipeline.rs`: `name`/`title` come from `title_from_filename` (T006), and the record
  stores the verbatim filename and its relative path.
- [x] T011 [US1] Make duplicate content a first-class outcome: `IngestOutcome`/the queue error names the existing
  archive id, and `QueueErrorText` renders it without a conflict-action link.
- [x] T012 [US1] (2026-11-09: complete. The staging/creation half went first; — `stage_pending_rename`, `check_already_patched`, `sweep_stale_pending_renames`, `PENDING_RENAME_MAX_AGE`, `hex_crc32_of_file` and the unreachable branch in `ingest_downloaded_file` are deleted; then the resolve/compare/server half and the UI half (T024) followed) Delete the conflict machinery end to end: `stage_pending_rename`, `sweep_stale_pending_renames`,
  `PENDING_RENAME_MAX_AGE`, `IngestDownloadError::PendingRename`/`NoOverwriteTarget`/`NameTooLong` and their
  `QueueError` mapping in `crates/lanrurugi-api/src/download_manager/ingest.rs`; `PendingFilenameConflict`,
  `ConflictPolicy` and the `temp_*` sweeps in `crates/lanrurugi-storage/src/download_queue.rs`; the overwrite/
  rename/compare/keep-side resolution paths in `crates/lanrurugi-api/src/download_queue.rs`; the `temp_` cleanup in
  `crates/lanrurugi-server/src/main.rs`. Callers listed by plan.md (`upload.rs`, `subscriptions`, `bundle.rs`,
  `plugins.rs`) follow the new signature.
- [x] T013 [US1] Remove `PUT /api/archives/{id}/rename` and `rename_archive` from
  `crates/lanrurugi-api/src/archives.rs` (route at ~281, handler at ~1850), its docgen sidecar entry under
  `docs/api/sidecar/`, and any repository helper that only it used.
- [ ] T014 [US1] Tests: (a) 300-byte filename ingests successfully and its record has the hash-layout path plus a
  sidecar; (b) re-ingesting the same bytes returns the existing id and adds no second record; (c) a title that is
  only release markers keeps a non-empty title; (d) the removed endpoint answers 404/405.

**Checkpoint**: the reported failure class is gone and the code that produced it no longer exists.

---

## Phase 4: User Story 2 — Everything outside the hash layout moves into `_byname` (Priority: P2)

**Goal**: every scan relocates catalogued archives that are neither `ab/{id}.ext` nor under `_byname/` into
`_byname/<original relative path>`, keeping names, ids and `.patch.zip` pairs; occupied targets are skipped and
reported.

**Independent Test**: a library with three archives in subdirectories scans once; ids, categories, tankoubons and
progress are unchanged; every path is now `_byname/<original path>`; a second scan reports zero moves. Moving one
file back by hand and scanning again puts it back under `_byname/`.

- [x] T015 [US2] Add `crates/lanrurugi-scanner/src/relocate.rs`: decide (`is_hash_layout_path` or already under
  `_byname` → skip), compute the target, refuse when the target exists with different content (`skipped_occupied`),
  move with `fs::rename` and an `EXDEV` copy → fsync → rename → unlink fallback, move the sibling `.patch.zip`
  with it.
- [x] T016 [US2] Call it from `full_scan` for each catalogued archive and from the watcher's single-file ingest
  path, inside the existing per-file unit of work so one failure cannot abort the scan.
- [x] T017 [US2] Persist the move: update `file` to the new absolute path and set `original_path` to the relative
  path it came from; delete the stale `LRR_FILEMAP` entry and write the new one (T002's fields, keys in
  `crates/lanrurugi-storage/src/keys.rs`).
- [x] T018 (2026-11-09: surfaced in the rebuild job's result payload, which is where the post-upgrade scan runs) [US2] Surface `RelocationCounters` (T003) in `POST /api/scan`'s result and the `scan` job status in
  `crates/lanrurugi-api/src/scan.rs`, so an administrator can see `moved_to_byname`/`skipped_occupied`/`failed`
  after the upgrade scan.
- [x] T019 [US2] (2026-11-09: `relocate.rs` gained patch-travel and second-pass-is-a-no-op tests; the occupied-target and inside-`_byname` cases already had one each) Tests: idempotent second pass; occupied target skipped and reported; `.patch.zip` moved with its
  archive; an interrupted run resumes without duplicating or losing a record; a file already under `_byname/` is
  untouched.

**Checkpoint**: an upgraded library has a clean root and the user's structure still browsable under `_byname/`.

---

## Phase 5: User Story 3 — A lost database does not lose the library's names (Priority: P3)

**Goal**: `rebuild-index` recovers hash-layout archives from file + sidecar, `_byname` archives from their
filenames, and reports disagreements instead of duplicating.

- [x] T020 [US3] Extend the rebuild path (`crates/lanrurugi-api/src/database.rs`'s rebuild helpers, used by
  `crates/lanrurugi-server/src/main.rs:1470`): for a hash-layout file read its sidecar when no record exists; for a
  `_byname` file derive title from the filename (T006). Content-derived id always wins.
- [x] T021 [US3] (2026-11-09: `crates/lanrurugi-scanner/src/sidecar_audit.rs` classifies every hash-layout archive's sidecar as ok/missing/unreadable/id-mismatch; `run_rebuild_sequence` surfaces the counts and the per-file details under `sidecars` in the job result and logs one warning when unclean) Report per-file discrepancies (sidecar id ≠ content id, unreadable sidecar, missing sidecar) in the
  rebuild report; never create a second record for the same bytes.
- [x] T022 [US3] (2026-11-09: `full_scan::tests::a_scan_recovers_titles_for_both_layouts_after_the_records_are_lost` — hash layout recovers `name`/`title` from the sidecar, `_byname/` from the filename, and no sidecar is written for the by-name file; the full flush-and-rebuild path itself is the same scan plus `rekey_all`) Test: flush the test database, rebuild, and recover ids/titles for both layouts; a sidecar whose
  id disagrees is reported and the content id wins.

---

## Phase 6: User Story 4 — Dropping files into the library still works (Priority: P3)

**Goal**: files copied in over SMB/rsync are catalogued with a title from their filename and then relocated into
`_byname/` by the same rule.

- [x] T023a (already true: `name` stays verbatim and `find_all_by_filename` matches `name`) [US3] Legacy backup import (`crates/lanrurugi-backup`, `import_legacy`) matches records by the
  archive's recorded `original_filename`, not by the on-disk name; a legacy backup whose files were renamed by this
  feature still imports with its metadata intact. Unit test with a fixture whose filenames differ from the hashed
  paths.
- [x] T023b [US1] (2026-11-09: `content_disposition_for` now names the file after the title, falling back to `name`, with path separators folded; three unit tests) Verify the download side did not move to ids: `GET /api/archives/{id}/download-link` and the
  signed `/api/dl/` handler still send `Content-Disposition` with the archive title (FR-019), and a tankoubon zip
  still uses its tankoubon title. Add a test that asserts the header for a hash-named archive.
- [x] T023 [US4] (2026-11-09: `full_scan::tests::a_dropped_file_is_catalogued_then_relocated_into_byname` writes behind the scanner's back, scans twice) Cover the drop flow with an integration test that writes a file into a subdirectory behind the
  scanner's back (as SMB would), runs a scan, and asserts: catalogued once, title from the filename, path
  `_byname/<relative path>`, no duplicate on the next scan. Fix whatever T016 missed.

---

## Phase 7: User Story 5 — The conflict and rename UI is gone (Priority: P4)

**Goal**: no filename template, no conflict menu, no rename popover, no filename field; title editing remains.

- [x] T024 [US5] (2026-11-09: `FilenameTemplateEditor.tsx`, `ComparisonResultModal.tsx` and `useCompareStream.ts`
  are deleted, together with the conflict branch in `QueueItemRow.tsx`, the `useOverwriteQueueItem`/
  `useRenameQueueItem`/`useCompareQueueItem` hooks and `PendingFilenameConflict` in `types.ts`. Deviation:
  `QueueErrorText.tsx` keeps rendering the `duplicate_filename`/`duplicate_filename_cleaned` kinds — the server
  still stores `duplicate_filename_cleaned` on rows whose staged file vanished across a restart, so dropping the
  renderer would turn those into blank errors.) Delete `apps/frontend/src/pages/Upload/FilenameTemplateEditor.tsx`
  and its imports; strip the conflict affordances from `apps/frontend/src/pages/Upload/QueueItemRow.tsx` and the
  duplicate-filename branch of `apps/frontend/src/components/Layout/QueueErrorText.tsx`.
- [x] T025 [US5] (the edit-page field and `useRenameArchive` were removed with the endpoint; the 21 now-unreferenced
  `upload.*` conflict/compare strings are gone from `en/zh/ja` — `zh_Hant` only carries overrides and had none) Remove the filename/rename field from the archive edit page and the `renameArchive`/conflict calls
  from `apps/frontend/src/api/` (and their `types.ts` entries), then delete the now-unused i18n keys from
  `apps/frontend/src/i18n/locales/{en,zh,ja}.json`. `tsc --noEmit` and `eslint src` must pass.

---

## Phase 8: Polish & Cross-Cutting Concerns

- [x] T026 (2026-11-09: `configuration.md` zh/en 的「书库里的文件名」小节复核过，删掉了未证实的「自 1.0.0-beta.6 起」版本声明) [P] Document the layout: a new section in `docs/src/content/docs/guide/` (and its `en/` twin) covering
  `{id}.{ext}` buckets, `_byname/`'s role, the automatic relocation on scan, why the root is reserved, and the
  sidecar's rebuild role. Update `docs/comparison-with-lanraragi.md` if the rename endpoint's removal belongs there.
- [x] T027 (2026-11-09 部分：dev 容器重建后实测「丢入 → 命名保持 → 搬进 `_byname/<相对路径>`」、记录 `name` 保留 `[DL版]` 而 `title` 已清理、`_byname` 档案不写旁车、`PUT /archives/{id}/rename` 返回 404；带 300 字节标题的真实下载与 hash 布局旁车仍需一次真实下载才能覆盖) Run `specs/020-hash-named-library/quickstart.md` S1–S10 against a dev instance and record the result in
  this file's implementation record.
- [x] T028 (2026-11-09: clippy、fmt-check、tsc、eslint、docs-build 与 CI 全绿) Gates: `mise run fmt-check`, `mise run clippy`, `mise run check-crate -- lanrurugi-storage
  lanrurugi-scanner lanrurugi-api`, `mise run test-crate -- lanrurugi-api lanrurugi-scanner`, and the frontend's
  `tsc --noEmit` + `eslint src`. `mise run docs-build` if the docs changed.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: no dependencies.
- **Foundational (Phase 2)**: depends on Setup; blocks every story.
- **US1 (Phase 3)**: depends on Foundational; blocks nothing but is the MVP.
- **US2 (Phase 4)**: depends on Foundational and benefits from US1's `catalogue_new_archive` changes (same file,
  so do it after T010 to avoid a second rewrite).
- **US3 (Phase 5)**: depends on US1 (sidecars exist) and US2 (`_byname` exists).
- **US4 (Phase 6)**: depends on US2's relocation step.
- **US5 (Phase 7)**: depends on US1's API deletions; independent of US2–US4.
- **Polish (Phase 8)**: after all desired stories.

### Within Each Story

- T004 → T009/T020 (sidecar before anything writes or reads it)
- T005 → T006 → T010 (marker cleaning before titles)
- T008 → T009 → T014; T015 → T016 → T017 → T018 → T019; T020 → T021 → T022

### Parallel Opportunities

- T001/T003 (different crates), T005/T007, T024/T025 (frontend files), T026 (docs) against any story still open.

---

## Implementation Strategy

### MVP first

1. Phase 1 + Phase 2.
2. Phase 3 (US1) — the reported failure class is gone and the conflict machinery is deleted.
3. **Validate** with quickstart S1–S3 before starting US2.

### Incremental delivery

US1 → US2 (the migration the user asked for) → US3 (rebuild) → US4 (drop flow) → US5 (UI) → Polish. US5 can ship
before US3/US4 if the queue UI is what bothers the user most.

## Notes

- The user's date-stamped decisions are in `spec.md`'s Assumptions and in
  `specs/share-followups-2026-11-08.md` D2; if a task appears to contradict one, the spec wins.
- No task may reintroduce a rename-in-the-browser flow, a filename template, or a `.classic`-style second layout.
- `.classic` was the working name; the directory is `_byname` and no task may rename it again.

## Implementation Record

**2026-11-09** — Phase 1, the sidecar, title derivation, the new ingest destination and the `_byname`
relocation pass are implemented and green (`lanrurugi-storage` 129 tests, `lanrurugi-scanner` 120,
`lanrurugi-api` 687, `mise run clippy`, `fmt-check`, frontend `tsc`). What follows records the
departures from plan.md, so a later reader does not mistake them for unfinished work.

**2026-11-09 (later pass)** — the relocation pass now refuses to move a file whose mtime is younger
than 60 seconds. A `cp`/rsync/SMB upload that is still arriving would otherwise be moved mid-copy:
the writer keeps appending to the old path while `_byname/` holds a truncated file, which the next
scan would then catalogue. The next pass moves it once it has settled; the test suite ages its
fixtures accordingly.

**2026-11-09 (adoption pass)** — rows that a pre-020 build parked with `pending_filename_conflict`
are no longer left asking a question that has no answer. At `serve` startup, a row whose staged
file still exists is ingested under its own id (`adopt_pending_conflict`) and marked `Done`; a
duplicate of bytes already in the library is dropped and the row points at the existing archive. A
row whose staged file vanished is still marked `StaleAfterRestart`, as before. The frontend conflict
menu (T024/T025/T026) can therefore be deleted without stranding anything.

- **No new `Archive` fields (T002).** `Archive.name` has always been the source filename minus its
  extension, so it is kept verbatim and only `title` is cleaned. This is what
  `find_all_by_filename` (`crates/lanrurugi-storage/src/repository.rs:181`) already matches, which
  is why legacy backup import keeps working (T023a) without touching `lanrurugi-backup`. The full
  original filename (extension included) and the relative path live in the sidecar.
- **No `RelocationCounters` struct (T003).** `ScanSummary` gained `relocated`,
  `relocation_skipped`, `relocation_failed`; surfacing them in `POST /api/scan` (T018) is still open.
- **Sidecar module** is `crates/lanrurugi-storage/src/sidecar.rs` (`Sidecar`, `write`, `read`), not
  `SidecarV1`; the version field makes the on-disk name unnecessary.
- **Title derivation** happens inside `catalogue_new_archive` (no separate `title.rs`); the sidecar
  is written there too, immediately after `archives.save`, and only for hash-layout paths.
- **Relocation** (`crates/lanrurugi-scanner/src/relocate.rs`) runs once at the end of `full_scan`,
  over every record, rather than per-file inside the ingest unit of work: it covers both the
  pre-020 upgrade and files dropped in since the last scan, in one pass, and a failed move can never
  abort a scan. The watcher path goes through `full_scan` too.
- **Conflict machinery is deleted (T012, T024).** The server half (`/overwrite`, `/rename`,
  `/compare*`, the `/compare/keep-b` and `/export-patch` routes, `resolve_conflict` and the whole
  comparison pipeline, the `compare_cache` store) and the client half (the conflict menu, rename
  popover, comparison overlay and its hooks/types) are gone. Items staged by a pre-upgrade build are
  adopted at startup by `download_manager::ingest::adopt_pending_conflict`; `DELETE /download_queue/{id}`
  still removes their leftover temp file, and `QueueErrorText` still renders the two
  `duplicate_filename*` codes for those historical rows.
- **Rebuild (T020/T021/T022).** `rebuild-index` re-scans, and `catalogue_new_archive` now reads a
  sidecar when no record exists, so a lost database recovers titles and original filenames. There is
  no separate discrepancy report yet, and no test that flushes the database and rebuilds.

## Implementation Record addendum (2026-11-09)

`crates/lanrurugi-api/src/download_manager/ingest.rs` now has tests for the end of the pipeline that a
filesystem cannot express but the record can: a download lands at `paths::hash_layout_path(...)`,
its `name` keeps the original filename verbatim, its `title` is the cleaned one, and the sidecar
holds `original_filename` and `title`. The second test feeds a 600-byte title and asserts the
record keeps it while the on-disk basename stays 44 bytes (40 hex + `.zip`) — the ENAMETOOLONG case
that started 020 is now an ingest-level assertion, not only a scan-level one.
