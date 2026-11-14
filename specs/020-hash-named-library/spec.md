# Feature Specification: Hash-Named Library

**Feature Branch**: `020-hash-named-library`

**Created**: 2026-11-08

**Status**: Draft

**Input**: User description — "放弃现在的档案命名方式，统一改成 hash.ext：直接上传的文件入库后自动改名 hash.ext，原文件名作为档案标题（现有的插件功能改为 Rust 原生，插件删除）；档案编辑页面的「显示/编辑档案名」功能去掉。所有新档案用新形式，老档案全都移动到 `_byname` 目录。"

## Context

Today the bytes of every archive are stored under a name derived from its human title, and that name is treated as user-visible state with its own lifecycle:

- Ingest picks a destination from the title (`catalogue_staged_file` in `crates/lanrurugi-api/src/download_manager/ingest.rs`), and when another file already occupies that name it fails to a `PendingFilenameConflict` (`crates/lanrurugi-storage/src/download_queue.rs`) that the queue holds until the user picks overwrite, rename, or compare. `unique_dest_path` appends ` (n)`/a uuid suffix; `stage_pending_rename` parks the bytes in the temp directory as `temp_{crc32}_{filename}`.
- Renaming is a first-class feature: `PUT /api/archives/{id}/rename` (`crates/lanrurugi-api/src/archives.rs:1837`) — an endpoint the legacy system never had — plus a rename popover and a conflict menu in the upload queue (`apps/frontend/src/pages/Upload/FilenameTemplateEditor.tsx`, `QueueItemRow.tsx`), and a filename template whose default is `{filename}_{crc}.{ext}`.
- Share import carries its own filename policy, and the scanner has to tolerate all of the above.

This was never necessary. An archive's identity is already content-derived (`size_aware_id` in `crates/lanrurugi-storage/src/id.rs`: SHA-1 of the first 512,000 bytes plus the file length), and the title has been decoupled from the disk name since `title_filename` (`crates/lanrurugi-scanner/src/pipeline.rs:498`) — ingest passes the real filename there explicitly. The only thing the human name buys is a browsable library directory, and it costs a state machine, a UI, and a class of bugs.

The class of bugs arrived on 2026-11-08: a download whose title exceeded the filesystem's 255-byte name limit failed with `File name too long (os error 36)` (`lanrurugi_api::download_manager::ingest`, Sentry `LANRURUGI-PROD-19`), surfacing as a queue item that could not be resolved and, in one earlier case, a zombie archive record with an empty `file`. Patching that one path with a byte check and a "rename in the browser" dialog leaves the state machine in place and only moves the failure boundary; the user's decision is to remove the reason the machinery exists.

This feature makes the on-disk name a function of content and nothing else:

1. **Every ingested archive is stored as `{id}.{ext}`**, bucketed by the first two characters of the id under the library root.
2. **The human name becomes metadata.** The original filename is recorded verbatim at ingest, the title is derived from it (release markers stripped) by the server itself rather than by a plugin, and it can be as long or as unusual as the source allows.
3. **Everything outside the hash layout moves into `_byname` unchanged.** Archives already in the library — and anything the user adds later — are relocated by the scanner itself, keeping their names and relative paths exactly as they are; `_byname` is catalogued the old way, by filename. Because nothing is renamed and the id is unchanged, categories, tankoubons, reading progress, bookmarks and stamps are untouched — this is a move, not a re-import.
4. **A sidecar per hash-named archive keeps identity recoverable.** `{id}.json` next to a new-layout file records the original filename, original relative path, title and extension, so a lost database does not lose that archive's name. `_byname` needs no sidecar: its filename is its name.

On the user's decision, no interim UI fix ships: the naming change alone removes the class of failure, and the abandoned
255-byte work is not a prerequisite.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A title of any length can be ingested (Priority: P1)

A user pastes a gallery whose title is a 300-character CJK string, or a title containing characters that are legal in a database but not in a filename. The download completes, the archive appears in the queue as finished, and it can be opened in the reader. The title is shown exactly as the source gave it (minus release markers), and the file on disk is named after its content, not after the title.

**Why this priority**: this is the production failure being fixed. Nothing else in the feature matters if a long title still cannot be stored.

**Independent Test**: ingest one archive whose title exceeds 255 bytes in UTF-8 and one whose title contains `/`, `:` and a newline; both must finish without a queue error and open in the reader.

**Acceptance Scenarios**:

1. **Given** a download whose title is 300 bytes of CJK, **When** ingestion finishes, **Then** the queue shows it as done, the archive opens, and no filename-related error appears anywhere.
2. **Given** the same download, **When** the user inspects the library directory, **Then** the bytes live at `{id}.{ext}` inside a two-character bucket directory, and the archive's title is the original title.
3. **Given** a title that cleans down to an empty string after release markers are stripped, **When** ingestion finishes, **Then** the archive falls back to the original filename as its title rather than to an empty title.

---

### User Story 2 - Everything that is not hash-named ends up under `_byname` (Priority: P2)

The first scan after upgrading finds a library whose archives are still named and organised by the user. The scanner
moves each one into `_byname/`, keeping its filename and its path relative to the library root, and updates the record;
nothing asks for confirmation, because keeping the root for machine-named files is the point of the feature. Files the
user drops in afterwards follow the same rule on the next scan. When it finishes, the library looks the same in the UI:
same titles, same categories, same tankoubons, same reading progress, and the user's folder structure is still visible
under `_byname/`. A file moved by mistake can be moved back, because its original path is recorded.

**Why this priority**: without this the library would keep two layouts forever and the root would stay mixed; it is
second only because it must not run before the new layout is proven on new ingests.

**Independent Test**: on a copy of a real library, scan once, compare archive ids, category membership, tankoubon
membership and progress counts before and after, then verify every path is `_byname/<original relative path>`, then
move one file back and scan again.

**Acceptance Scenarios**:

1. **Given** a library of 100,000 archives in arbitrary subdirectories, **When** the scan completes, **Then** every
   archive id is unchanged, every category/tankoubon/progress reference still resolves, and every file is under
   `_byname/` with its original filename and relative path.
2. **Given** an archive with a `.patch.zip` beside it, **When** the archive moves, **Then** the patch moves with it and
   remains attached.
3. **Given** a scan interrupted halfway, **When** it runs again, **Then** archives already under `_byname/` are
   untouched, the rest move, and no record is duplicated or lost.
4. **Given** a file dropped into the library root after the upgrade, **When** the next scan runs, **Then** it is
   catalogued and moved under `_byname/`.

### User Story 3 - A lost database does not lose the library's names (Priority: P3)

The Redis instance is lost or deliberately flushed. Rebuilding the index walks the library, reads each archive's sidecar, and restores every archive's id, title, original filename and path. The user reattaches categories and tankoubons from the last export, but does not have to re-name or re-identify a single archive.

**Why this priority**: hash naming removes the last human-readable trace from the disk, so the sidecar is what keeps a database loss from being a library loss. It is lower than the relocation because it only matters after the layout exists.

**Independent Test**: flush the database on a test instance, run rebuild-index, and compare the recovered archive count, ids and titles against a snapshot taken before the flush.

**Acceptance Scenarios**:

1. **Given** a library with sidecars and an empty database, **When** the index is rebuilt, **Then** every archive is recovered with its id and its title.
2. **Given** a sidecar whose recorded id disagrees with the file's content-derived id, **When** the index is rebuilt, **Then** the file's id wins, the discrepancy is reported, and no archive is duplicated.

---

### User Story 4 - Dropping files into the library still works (Priority: P3)

A user copies a batch of archives into the library directory over SMB. The scanner picks them up on its next pass exactly as it does today, the original filename becomes the title, and the files then move under `_byname/` with their relative paths and names intact — so the folder structure the user copied in survives.

**Why this priority**: this is the workflow the browsable directory exists for; breaking it would make the feature user-hostile even though it is not the reported bug.

**Independent Test**: copy a file into the library directory, wait for a scan, and confirm it appears with a title derived from its filename.

**Acceptance Scenarios**:

1. **Given** a new file placed anywhere in the library directory, **When** a scan runs, **Then** it is catalogued and appears in the library.
2. **Given** such a file, **When** it is catalogued, **Then** it is relocated under `_byname/` without being renamed, and its path relative to the library root is preserved.

---

### User Story 5 - The conflict and rename UI is gone (Priority: P4)

A user opens the upload queue and the archive edit page. There is no "overwrite / rename / compare" conflict menu, no filename template, no rename popover, and no filename field on the edit page. The only name they see is the title, which they can edit as before.

**Why this priority**: it is the visible half of the simplification, but it is only honest once the storage layout no longer produces conflicts.

**Independent Test**: grep the frontend and the API surface for the removed affordances and exercise the upload queue with two archives whose titles collide.

**Acceptance Scenarios**:

1. **Given** two downloads whose titles are identical, **When** both finish, **Then** both are catalogued with distinct ids and neither raises a conflict.
2. **Given** the archive edit page, **When** it opens, **Then** it offers title editing but no filename display or rename action.

---

### Edge Cases

- Two different files that share the first 512,000 bytes and the same length (the id is a sample hash, not a full-content hash) must be rejected as a duplicate and reported, never stored under one path.
- A source file with no extension, or with an extension the scanner does not know, must still get a deterministic stored name and a format decision.
- A file whose name is not valid UTF-8 must not abort a scan; it is skipped with a log entry.
- A user or SMB client renames or moves an archive behind the application's back: the next scan must recognise the content-derived id and update the record rather than create a duplicate.
- A `.patch.zip` whose archive was moved manually must not be silently attached to a different archive.
- `_byname` on a different filesystem from the library root: the move must fall back to copy-then-delete and must not lose the file if it is interrupted mid-copy.
- Two archives in different subdirectories that share a filename: both keep their relative path, so `_byname` never has to resolve a name collision.
- A drop whose relative path already exists under `_byname/` with different content: the move is skipped and reported, never overwritten.
- A sidecar that is corrupt or unparsable must not prevent the archive from being catalogued; it is reported and regenerated.
- Case-insensitive filesystems: ids are lowercase hex, so no two ids collide by case; the bucket names must not assume case sensitivity.
- An archive already named `{id}.ext` whose bytes no longer match the id (edited in place) is a rekey case and must be handled explicitly, not silently.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: Every archive the application ingests MUST be stored under a name consisting of its content-derived id and its extension, and of nothing else.
- **FR-002**: Stored archives MUST be placed in a two-character bucket directory derived from the first two characters of the id, created on demand; the application MUST NOT place new archives directly in the library root.
- **FR-003**: Ingest MUST NOT fail, and MUST NOT defer to a user decision, because of the title's length, characters, or collision with another archive's title.
- **FR-004**: The original filename MUST be recorded verbatim as archive metadata at ingest time, regardless of its length or characters, and MUST survive restarts.
- **FR-005**: The archive title MUST default to the original filename with release markers removed, computed by the server; title derivation MUST NOT depend on a plugin being installed or enabled.
- **FR-006**: Re-ingesting content that is already in the library MUST be rejected as a duplicate that names the existing archive; it MUST NOT overwrite, merge, or create a second record.
- **FR-007**: The filename-conflict workflow MUST be removed end to end: no pending-conflict queue state, no overwrite/rename/compare actions, no rename popover, no filename template.
- **FR-008**: The archive filename rename capability MUST be removed from the API and from the edit page; title editing MUST remain.
- **FR-009**: Each archive stored in the hash layout MUST have a sidecar record next to it, written atomically, holding at least the id, original filename, original relative path, title and extension. Archives under `_byname` MUST NOT need one: their filename stays their name.
- **FR-010**: Sidecars MUST NOT be catalogued as archives by scanning, watching, or sweeping.
- **FR-011**: Index rebuild MUST be able to reconstruct a hash-layout archive's id, title and path from the file and its sidecar alone, and a `_byname` archive's title from its filename, with the database as the authority when both exist and every disagreement reported.
- **FR-012**: Any catalogued archive whose stored path is neither the hash layout nor under `_byname/` MUST be relocated into `_byname` during scanning, keeping its filename and its path relative to the library root, without asking the user; the archive id MUST NOT change.
- **FR-013**: Relocation MUST be idempotent and safe to interrupt: a file already in the hash layout or under `_byname/` MUST NOT be moved, and a target path already occupied by different content MUST be reported and skipped rather than overwritten.
- **FR-014**: Relocation MUST be observable and reversible by hand: each move is logged with its source and destination and counted in the scan report, and `original_path` records where the file came from.
- **FR-015**: The `_byname` directory MUST be a normal part of the library for scanning and serving; it MUST NOT be a hidden area the application refuses to index.
- **FR-016**: Files a user places in the library directory themselves MUST be catalogued and then relocated into `_byname` by the same rule as FR-012, keeping their names; the library root MUST end up holding only hash-named archives. (Resolved 2026-11-08: automatic relocation was chosen — the root is for machine-named files, and `_byname` preserves the user's own organisation.)
- **FR-017**: Legacy backup import MUST match archives by the recorded original filename, not by the on-disk name.
- **FR-018**: The path-to-id mapping used to detect files still being written MUST stay consistent across relocation and rekey operations.
- **FR-019**: Downloads MUST continue to offer the archive's title as the downloaded filename, never the id.
- **FR-020**: The application MUST NOT require a filesystem-specific name limit anywhere in the ingest path; the only name the application constructs is bounded by construction.

### Key Entities

- **Archive**: the catalogued unit; its id is derived from its content, its stored path is a function of the id, and its title and original filename are metadata.
- **Sidecar record**: a per-archive document next to a hash-layout archive's bytes, recording what the database knows so the library can be rebuilt without it. `_byname` archives have none.
- **Relocation rule**: the scanner's continuous rule that moves any archive outside the hash layout and outside `_byname/` into `_byname/<relative path>`, keeping its name and id.
- **Bucket directory**: one of 256 two-character directories under the library root; holds archives and sidecars, never other structure.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An archive whose title is 300 UTF-8 bytes is ingested, listed, opened and downloaded with no error, on the first attempt.
- **SC-002**: After release, "file name too long" errors attributable to archive ingest are zero over a 30-day window.
- **SC-003**: A 100,000-archive relocation changes zero archive ids, leaves category, tankoubon and reading-progress counts identical before and after, and leaves no archive outside `_byname/` and the buckets.
- **SC-004**: With the database flushed, an index rebuild recovers 100% of archives' ids and titles from the on-disk layout — hash-layout archives from their sidecar, `_byname` archives from their filenames.
- **SC-005**: At 100,000 archives, no bucket directory holds more than 2,000 entries (archives plus sidecars); `_byname` mirrors the user's own structure and is not subject to the fanout.
- **SC-006**: The upload queue can finish two downloads with identical titles without asking the user anything, and no UI path offers overwrite/rename/compare.

## Assumptions

- The archive id algorithm stays as it is (a sample hash over the first 512,000 bytes plus length). This keeps existing ids and therefore avoids a rekey during relocation; the consequence — two files can share an id while differing later in the file — is handled by rejecting the duplicate rather than by changing the algorithm.
- At 100,000 archives the two-character fanout keeps buckets around 400 entries; four-character fanout was considered and rejected because it makes the library root unlistable for humans, backup tools and SMB clients.
- The `_byname` directory lives inside the library directory (the user's decision) and is therefore scanned like any other subdirectory; the name is a convention for humans, not a reserved mount.
- Existing archives in `_byname` keep their filenames and layout exactly as before (user's decision, 2026-11-08): the directory stays browsable and nothing needs a sidecar to pair with.
- Sidecars are enabled by default; they add roughly one small file per archive and are ignored by every enumerator.
- Files a user drops into the library are catalogued and then moved under `_byname/` on the next scan, keeping their names; nothing is renamed.
- External tools or scripts that reference archives outside `_byname` by their old paths will need the `_byname/` prefix; this is documented, and `original_path` is the recovery path.
- The production error stops as soon as this feature lands: the stored name no longer depends on the title, so no interim UI fix is shipped first (the abandoned 255-byte work is not a prerequisite).
- Share import keeps its own formats and permissions; only its filename policy changes, because there is no filename policy left to apply.
