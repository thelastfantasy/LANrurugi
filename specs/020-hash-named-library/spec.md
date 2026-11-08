# Feature Specification: Hash-Named Library

**Feature Branch**: `020-hash-named-library`

**Created**: 2026-11-08

**Status**: Draft

**Input**: User description — "放弃现在的档案命名方式，统一改成 hash.ext：直接上传的文件入库后自动改名 hash.ext，原文件名作为档案标题（现有的插件功能改为 Rust 原生，插件删除）；档案编辑页面的「显示/编辑档案名」功能去掉。所有新档案用新形式，老档案全都移动到 `.classic` 目录。"

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
3. **The existing library moves into `.classic`.** Old archives are relocated (and renamed to `{id}.ext`) by an explicit, previewable, reversible migration. Because the id is unchanged by a rename, categories, tankoubons, reading progress, bookmarks and stamps are untouched — this is a move, not a re-import.
4. **A sidecar per archive keeps identity recoverable.** `{id}.json` next to the file records the original filename, original relative path, title and extension, so a lost database does not lose the library's names.

On the user's decision, the interim fix (store an over-long title under a short placeholder name and keep the real title as metadata) is a separate, small change; this specification describes the end state that makes the placeholder unnecessary.

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

### User Story 2 - The existing library is moved to `.classic` without losing anything (Priority: P2)

An administrator with an existing library starts the migration from the settings page. The preview reports how many archives will move, how many would collide by content and be skipped, and how many are already in the target shape. They confirm, and the job moves every pre-existing archive (and its `.patch.zip`) into `.classic/<bucket>/{id}.ext`. When it finishes, the library looks the same: same titles, same categories, same tankoubons, same reading progress. If something goes wrong, a rollback puts every file back where it was.

**Why this priority**: without this, the feature would only apply to future archives and the user would be left with a permanently split library. It is second only because it must not run before the new layout is proven on new ingests.

**Independent Test**: on a copy of a real library, run the preview, run the migration, compare archive ids, category membership, tankoubon membership and progress counts before and after, then roll back and compare paths.

**Acceptance Scenarios**:

1. **Given** a library of 100,000 archives in arbitrary subdirectories, **When** the migration completes, **Then** every archive id is unchanged and every category/tankoubon/progress reference still resolves.
2. **Given** an archive with a `.patch.zip` beside it, **When** the archive moves, **Then** the patch moves with it and remains attached.
3. **Given** a migration interrupted halfway, **When** it is restarted, **Then** it resumes without moving an already-moved archive twice and without losing a partially moved one.
4. **Given** a completed migration, **When** the administrator rolls it back, **Then** every file returns to its recorded original path and the library still resolves.
5. **Given** two files with identical content under different names, **When** the migration runs, **Then** it reports the collision and keeps one, rather than silently overwriting the other.

---

### User Story 3 - A lost database does not lose the library's names (Priority: P3)

The Redis instance is lost or deliberately flushed. Rebuilding the index walks the library, reads each archive's sidecar, and restores every archive's id, title, original filename and path. The user reattaches categories and tankoubons from the last export, but does not have to re-name or re-identify a single archive.

**Why this priority**: hash naming removes the last human-readable trace from the disk, so the sidecar is what keeps a database loss from being a library loss. It is lower than the migration because it only matters after the layout exists.

**Independent Test**: flush the database on a test instance, run rebuild-index, and compare the recovered archive count, ids and titles against a snapshot taken before the flush.

**Acceptance Scenarios**:

1. **Given** a library with sidecars and an empty database, **When** the index is rebuilt, **Then** every archive is recovered with its id and its title.
2. **Given** a sidecar whose recorded id disagrees with the file's content-derived id, **When** the index is rebuilt, **Then** the file's id wins, the discrepancy is reported, and no archive is duplicated.

---

### User Story 4 - Dropping files into the library still works (Priority: P3)

A user copies a batch of archives into the library directory over SMB. The scanner picks them up on its next pass exactly as it does today, and the original filename becomes the title.

**Why this priority**: this is the workflow the browsable directory exists for; breaking it would make the feature user-hostile even though it is not the reported bug.

**Independent Test**: copy a file into the library directory, wait for a scan, and confirm it appears with a title derived from its filename.

**Acceptance Scenarios**:

1. **Given** a new file placed anywhere in the library directory, **When** a scan runs, **Then** it is catalogued and appears in the library.
2. **Given** such a file, **When** it is catalogued, **Then** the application does not rename or move it on its own.

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
- `.classic` on a different filesystem from the library root: the migration must fall back to copy-then-delete and must not lose the file if it is interrupted mid-copy.
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
- **FR-009**: Each archive MUST have a sidecar record next to it, written atomically, holding at least the id, original filename, original relative path, title and extension.
- **FR-010**: Sidecars MUST NOT be catalogued as archives by scanning, watching, or sweeping.
- **FR-011**: Index rebuild MUST be able to reconstruct an archive's id, title and path from the file and its sidecar alone, with the database as the authority when both exist and every disagreement reported.
- **FR-012**: An explicit migration MUST move every pre-existing catalogued archive, and its `.patch.zip`, from its current path into the `.classic` directory under the new layout, preserving the id.
- **FR-013**: The migration MUST be previewable before it changes anything, reporting at least how many archives move, how many are merged as duplicate content, how many are skipped and why.
- **FR-014**: The migration MUST be resumable after interruption and MUST record enough information to be reversible.
- **FR-015**: The `.classic` directory MUST be a normal part of the library for scanning and serving; it MUST NOT be a hidden area the application refuses to index.
- **FR-016**: Files a user places in the library directory themselves MUST be catalogued in place without the application renaming them; adoption into the hash layout happens only when the user explicitly selects those files for it. (Resolved 2026-11-08: automatic adoption on scan was rejected — the library directory is the user's, and renaming files a user just copied in is the behaviour that made the old naming scheme surprising.)
- **FR-017**: Legacy backup import MUST match archives by the recorded original filename, not by the on-disk name.
- **FR-018**: The path-to-id mapping used to detect files still being written MUST stay consistent across migration and rekey operations.
- **FR-019**: Downloads MUST continue to offer the archive's title as the downloaded filename, never the id.
- **FR-020**: The application MUST NOT require a filesystem-specific name limit anywhere in the ingest path; the only name the application constructs is bounded by construction.

### Key Entities

- **Archive**: the catalogued unit; its id is derived from its content, its stored path is a function of the id, and its title and original filename are metadata.
- **Sidecar record**: a per-archive document next to the stored bytes, recording what the database knows so the library can be rebuilt without it.
- **Migration job**: a resumable, previewable operation that relocates pre-existing archives into `.classic` and records the original paths for rollback.
- **Bucket directory**: one of 256 two-character directories under the library root; holds archives and sidecars, never other structure.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An archive whose title is 300 UTF-8 bytes is ingested, listed, opened and downloaded with no error, on the first attempt.
- **SC-002**: After release, "file name too long" errors attributable to archive ingest are zero over a 30-day window.
- **SC-003**: A 100,000-archive migration changes zero archive ids and leaves category, tankoubon and reading-progress counts identical before and after.
- **SC-004**: With the database flushed, an index rebuild recovers 100% of archives' ids and titles from the on-disk layout.
- **SC-005**: At 100,000 archives, no single directory holds more than 2,000 entries (archives plus sidecars).
- **SC-006**: The upload queue can finish two downloads with identical titles without asking the user anything, and no UI path offers overwrite/rename/compare.

## Assumptions

- The archive id algorithm stays as it is (a sample hash over the first 512,000 bytes plus length). This keeps existing ids and therefore avoids a rekey during migration; the consequence — two files can share an id while differing later in the file — is handled by rejecting the duplicate rather than by changing the algorithm.
- At 100,000 archives the two-character fanout keeps buckets around 400 entries; four-character fanout was considered and rejected because it makes the library root unlistable for humans, backup tools and SMB clients.
- The `.classic` directory lives inside the library directory (the user's decision) and is therefore scanned like any other subdirectory; the name is a convention for humans, not a reserved mount.
- Existing archives moved into `.classic` are also renamed to `{id}.ext`, so the whole library has one shape and sidecars can be paired by filename.
- Sidecars are enabled by default; they add roughly one small file per archive and are ignored by every enumerator.
- Files a user drops into the library are catalogued in place and never silently renamed.
- External tools or scripts that reference archives by their old filenames will stop working; this is documented, and the sidecar's original path is the recovery path.
- An interim fix that stores over-long titles under a short placeholder name ships before this feature, so the production error stops immediately; this specification describes the state in which the placeholder is no longer needed.
- Share import keeps its own formats and permissions; only its filename policy changes, because there is no filename policy left to apply.
