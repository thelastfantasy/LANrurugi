# Feature Specification: Recycle Bin and Safety Password

**Feature Branch**: `019-recycle-bin-safety-password`

**Created**: 2026-10-31

**Status**: Draft

**Input**: User description: "我想加入回收站和安全密码功能，具体的做法类似于115网盘" — confirmed in conversation:
the bin covers archives and whole Tankoubons (deleting a Tankoubon offers dissolve vs recycle); deleting moves the
file (or every member plus the grouping) into a library-internal recycle area and restore puts it back; the safety password is exactly four half-width digits and guards permanent deletion
and emptying the bin (not viewing it, not deleting into it); a forgotten safety password can be reset both from
the settings page (after entering the login password) and from the server CLI; the bin keeps entries for a
configurable number of days (default 30) and under a configurable size cap, evicting the oldest first.

## Context

Today deleting an archive is irreversible and instantaneous. `delete_one_archive`
(`crates/lanrurugi-api/src/archives.rs`) deletes the Redis record, unlinks the archive file, its sidecar
`.patch.zip`, its cover and per-page thumbnails, and drops the search-index entries, recommendation cache,
category memberships, bookmarks, stamps and any download-queue rows that referenced it. A mis-click on the
Library grid, a wrong selection on the Batch page, or a mistaken sweep on the Edit page loses the file and
every piece of user data attached to it with no undo — recovery means re-obtaining the file from wherever it
originally came from, and re-entering title, tags, categories and bookmarks by hand.

This feature gives that action a safety net and a second lock:

1. **Deleting archives moves them to a recycle bin.** The archive disappears from the library exactly as it
   does today, but its file, metadata and user data are retained for a configurable window, and it can be
   restored to its original location in one action — the 115-style "回收站".
2. **Irreversible actions need a safety password.** Permanently deleting one entry or emptying the bin asks for
   a four-digit safety password that is separate from the login password — the 115-style "安全密码". It does
   not stand between the user and normal deletion, only between the user and actions that cannot be undone.
3. **The bin governs its own lifetime.** A configurable retention window and size cap bound how much disk the
   bin can hold, with the oldest entries evicted first.

Three existing behaviours must not be broken by making deletion recoverable:

- **The library scan walks the whole library directory.** `full_scan::walk`
  (`crates/lanrurugi-scanner/src/full_scan.rs`) recurses into every subdirectory with no hidden-directory
  exclusion, and `scripts.rs::walk_subfolders` / `walk_first_level_subfolders` turn subdirectories into
  categories and tankoubons. Anything placed inside the library for the bin is therefore visible to all three
  unless each explicitly excludes it.
- **Archive identity is content-derived.** An id is `SHA-1(first 512000 bytes)` of the file
  (`lanrurugi-storage/src/id.rs`), so re-importing the same file after deleting it produces the same id — the
  restore path has to treat "an archive with this identity already exists again" as a real, expected conflict.
- **Anything that enumerates archive ids today assumes the id resolves.** Share links, OPDS, tankoubon
  membership, recommendations and statistics all hold ids; a recycled archive's id must behave exactly like a
  nonexistent one everywhere except the bin.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A deleted archive can be brought back (Priority: P1)

An administrator deletes an archive by mistake from the Library grid. The confirmation now says the archive is
moved to the recycle bin. They open the new recycle bin page, find it with its thumbnail and title, and click
restore; the archive is back on the shelf with its title, tags, categories, bookmarks and reading progress
intact, exactly where it was before.

**Why this priority**: this is the entire value of the feature. Without a working, complete restore there is no
recycle bin, only a second place where data disappears.

**Independent Test**: delete an archive that has a title, tags, two category memberships, three bookmarks and a
reading position, then restore it from the bin page and verify every one of those attributes plus the original
file path are back, and the archive is searchable again.

**Acceptance Scenarios**:

1. **Given** an archive in the library, **When** the administrator deletes it, **Then** it leaves the library
   list, search, tag facets, category contents and statistics immediately, and appears in the recycle bin with
   its title, size, deletion time and original path.
2. **Given** a recycled archive, **When** the administrator restores it, **Then** the file is back at its
   original path, the archive is visible and searchable again, and its title, tags, summary, page count,
   categories, bookmarks, stamps and reading progress are identical to before deletion.
3. **Given** an archive selected on the Batch page, **When** it is deleted in a batch operation, **Then** every
   deleted archive in that batch is recoverable individually, and the batch result still reports per-id success.
4. **Given** a delete request arriving through an API token, **When** it succeeds, **Then** the archive is
   recoverable in the same way (the token can delete, but cannot reach the bin's destructive actions).

---

### User Story 2 - Emptying the bin cannot be done by accident (Priority: P2)

Permanently deleting an entry or emptying the bin requires the four-digit safety password. An administrator who
has not set one yet is prompted to create it the first time; someone who forgot it can reset it from the
settings page with the login password, or from the server CLI if they cannot log in at all. Repeated wrong
guesses lock the prompt instead of allowing unlimited attempts.

**Why this priority**: the bin is only a safety net if emptying it is deliberate. It is second because nothing
in the feature depends on it existing first, but shipping the bin without it would let one mis-click inside the
bin destroy what the bin was protecting.

**Independent Test**: with no safety password configured, attempt to empty the bin; verify it is refused and the
UI offers setup. Set a password, verify wrong entries are rejected and lock the prompt after five attempts, then
verify the correct password empties the bin and the entries' files are gone from disk.

**Acceptance Scenarios**:

1. **Given** no safety password is configured, **When** the administrator tries to permanently delete an entry
   or empty the bin, **Then** the action is refused with an instruction to set a safety password first, and the
   UI offers to set it inline.
2. **Given** a configured safety password, **When** the administrator enters the correct four digits for the
   action, **Then** exactly that action executes and a second request cannot reuse the verification.
3. **Given** five consecutive wrong entries in one session, **When** the administrator tries again, **Then**
   verification is locked for five minutes — even the correct digits are refused until the lock expires, and
   the remaining wait time is shown.
4. **Given** a locked or unlocked session, **When** the administrator enters the login password on the settings
   page, **Then** they can set a new safety password without knowing the old one.
5. **Given** an instance where the administrator cannot log in, **When** they run the documented reset command on
   the server, **Then** the safety password is cleared and the next sensitive action requires setting a new one.
6. **Given** an API token of any role, or a guest visitor, **When** it calls any recycle-bin endpoint, **Then** the
   request is denied — the bin is a session-only surface, and the destructive actions additionally need the
   safety password.

---

### User Story 3 - The bin stays bounded (Priority: P3)

The recycle bin does not grow forever. The administrator can see how much space it currently occupies in the
settings page, choose how many days entries are kept (default 30, or forever) and how large the bin may become
(default unlimited, 0). Entries past the window or over the cap are removed oldest-first without any
user action, and the bin page shows how long each remaining entry has left.

**Why this priority**: without it the feature trades a one-click data loss for an unbounded disk leak on a
self-hosted appliance, but the bin is still useful on day one with sensible defaults.

**Independent Test**: set retention to one day and a small size cap, add entries, run the periodic sweep, and
verify expired entries are gone, the total stays under the cap (plus the newest entry), and the usage display
matches the bytes on disk.

**Acceptance Scenarios**:

1. **Given** a recycled archive older than the configured retention, **When** the automatic sweep runs, **Then**
   the entry and its file are permanently removed and the event is recorded in the activity log as system-initiated.
2. **Given** the bin's total size exceeds the configured cap, **When** the sweep runs, **Then** the oldest entries
   are removed until the total is within the cap; the most recently deleted entry is retained even when it alone
   exceeds the cap.
3. **Given** the administrator shortens the retention window or lowers the cap in settings, **When** the change
   is saved, **Then** the new limits are applied without waiting for the next sweep.
4. **Given** the recycle bin page, **When** it loads, **Then** it shows the current usage in entries and bytes,
   the configured limits, and for every entry its remaining retention time.

---

### Edge Cases

- **Original path is occupied.** While an archive sits in the bin, a different file with a different name can
  appear where the archive used to be. Restore must never overwrite: it places the restored file beside the
  occupant under a non-colliding name and reports the actual path.
- **Identical content was re-imported.** Because ids are content-derived, re-obtaining the same file after
  deleting it creates an archive with the same identity. Restoring the bin copy would create two records for one
  id — this must be refused with an explicit explanation and an offer to permanently delete the bin copy.
- **Original directory no longer exists.** The library may have been reorganised, or the delete itself pruned a
  subfolder that became empty. Restore recreates the recorded parent chain inside the library, so the archive
  lands back where it was; only a path outside the current library falls back to the root.
- **The file has been deleted or replaced in the bin.** An entry whose stored file is missing must be shown as
  un-restorable, must not break the page, and can only be permanently removed. An entry whose stored file no
  longer matches its recorded size must be treated the same way.
- **Crash between moving the file and recording the entry.** On startup the bin is reconciled against the disk:
  a stored file with no entry is an orphan that can be swept; an entry with no stored file is flagged, never
  silently offered as restorable.
- **Disk full or permission denied mid-delete.** The delete fails, the archive stays catalogued and visible, and
  no bin entry is created — a half-deleted archive is worse than no recycle bin.
- **Concurrent destructive actions.** Two tabs, or a Batch delete racing an empty-bin, must not interleave on the
  same entry; the loser receives a clear "already in progress / already gone" result instead of corrupt state.
- **Deletion triggered by the system, not by a person.** Missing-file cleanup, a superseded revision retired
  after a newer download, and the original archive removed by a split must not create bin entries and must not
  fail because of the bin. Only an explicit delete action populates the recycle bin.
- **Recycled ids across the app.** Share pages, OPDS, tankoubon listings, recommendations and the reader must
  behave as though a recycled archive does not exist (404 / omitted), while its tankoubon membership reference
  survives so the restored archive reappears in its groups.
- **The bin while guest mode is on.** Guest visitors never see a recycle bin entry point, and the endpoints
  answer as if the bin does not exist.
- **Safety password input.** Anything that is not exactly four half-width ASCII digits is rejected at set time;
  pasted full-width digits are rejected rather than silently converted.
- **Retention of zero.** A retention value of zero means "keep forever" and must not be read as "expire
  immediately".
- **A delete of an archive that is currently open in another reader tab.** The tab's subsequent requests behave
  as a 404, as they do for any deleted archive today.

## Requirements *(mandatory)*

### Soft delete and restore

- **FR-001**: Every user-initiated archive deletion — single delete, batch delete, and delete through an API
  token, from any page — MUST move the archive into the recycle bin instead of destroying it, while preserving
  the existing delete response shape (new information may only be added as new fields). Deletions that are not a
  user's explicit delete action MUST NOT enter the bin and fall under FR-017.
- **FR-002**: The archive's file and its sidecar patch MUST be moved into the bin's storage area as part of the
  delete operation, and the archive record, index entries and caches MUST only be discarded after that move
  succeeds. If the move fails, the delete MUST fail and the archive MUST remain fully catalogued and readable.
- **FR-003**: Each bin entry MUST retain the archive's identity, display title, tags, summary, page count, file
  size, original absolute path, deletion time, and deleting actor, together with the user data that deletion
  currently destroys: category memberships, page bookmarks, stamps and reading progress.
- **FR-004**: From the moment a delete returns success, a recycled archive MUST be absent from the library
  listing, search results, tag facets, category contents, statistics, recommendations, duplicate detection,
  shares, OPDS feeds and the reader — behaving exactly like a nonexistent archive.
- **FR-005**: The bin's storage area MUST be excluded from every consumer that walks the library directory: the
  live file watcher, a full rescan, and the two subfolder-driven scripts (categories and tankoubons), all of
  which already share one path filter. A full rescan of a library containing recycled archives MUST re-catalogue
  zero of them.
- **FR-006**: Restoring an entry MUST move its file back to the recorded original path, recreate the archive
  record with the retained metadata, and re-apply the retained category memberships, bookmarks, stamps and
  reading progress. The restored archive MUST be visible and searchable immediately, and the entry MUST leave
  the bin.
- **FR-007**: If the original path is occupied by a different file, restore MUST place the archive beside it
  under a non-colliding name, MUST NOT overwrite the occupant, and MUST report the path actually used.
- **FR-008**: If an archive with the same identity already exists in the library, restore MUST be refused with a
  message naming the conflict and an offer to permanently delete the bin copy; no duplicate record may be created.
- **FR-009**: If the recorded path is inside the configured library, restore MUST recreate any missing parent
  directories and use that exact path. Only a recorded path outside the configured library falls back to the
  library root under the original filename; either way the path actually used MUST be reported.
- **FR-010**: Permanently deleting one entry MUST remove its stored file, sidecar patch, thumbnails and entry
  metadata, and MUST be irreversible.
- **FR-011**: Emptying the bin MUST permanently delete every entry in one confirmed operation; if some entries
  fail, the response MUST report which ones failed and those entries MUST remain listed.
- **FR-012**: An entry whose stored file is missing or whose stored size disagrees with the recorded size MUST
  be reported as un-restorable, MUST not make the listing fail, and MUST be removable.
- **FR-013**: The bin MUST be reconciled against the disk on startup and on demand: orphaned stored files without
  an entry are reported as reclaimable leftovers, and entries without a stored file are flagged per FR-012.
- **FR-014**: Operations on a single entry (restore, permanent delete, empty-bin) MUST be serialized per entry;
  a request that loses the race MUST fail cleanly with an "in progress" or "already gone" result rather than
  corrupting the entry.
- **FR-015**: Moving an archive to the bin, restoring it, permanently deleting an entry and emptying the bin MUST
  each be recorded in the activity log with the acting identity, archive identity, title and file path.
- **FR-016**: A recycled archive's existing thumbnails MUST be retained for as long as its entry exists and
  served for its bin entry; they MUST be removed when the entry is permanently deleted. Any automatic thumbnail
  cleanup added later MUST treat recycled ids as live.
- **FR-037**: Moving an archive into the bin MUST remove directories under the library that the move left empty,
  walking upward only while each level is empty and stopping at the library root. A directory that still holds
  anything, and anything outside the library, MUST NOT be removed.
- **FR-038**: Deleting a Tankoubon MUST offer two explicit choices: dissolve it (delete only the grouping, keep the
  member archives in the library — the existing behaviour) or recycle it. Dissolve MUST remain available and MUST
  be the default choice.
- **FR-039**: Recycling a Tankoubon MUST move every member archive into the bin and delete the grouping record only
  after all of them moved, keeping the grouping's name, summary, tags, volume order, reading progress and cover
  choice. A failure while moving a member MUST roll the already-moved members back and leave the Tankoubon intact.
- **FR-040**: The bin MUST list a recycled Tankoubon as one row (title, member count, total size, deletion time,
  remaining retention) with an expand control that reveals its member archives read-only. Member archives MUST NOT
  appear as separate top-level rows.
- **FR-041**: Restoring a recycled Tankoubon MUST preflight every member (missing payload, changed size, or an
  identical archive already back in the library) and abort without restoring anything if one cannot be restored;
  otherwise it MUST restore all members and re-create the grouping with its saved metadata and volume order.
- **FR-042**: Permanently deleting a recycled Tankoubon MUST remove every member's payload and entry plus the
  grouping record in one confirmed operation, and MUST require the safety password. Retention and capacity
  enforcement MUST treat a recycled Tankoubon as one unit (its members are never evicted individually, and the
  newest row is never evicted by capacity).
- **FR-017**: Deletions that are not a user's explicit delete action MUST bypass the bin and purge directly.
  This covers at least: automated missing-file cleanup for a record whose file is already gone; a superseded
  revision retired automatically once a newer one is catalogued; and the original archive removed as part of a
  split operation. None of them may create a bin entry, and none may fail because the bin exists.

### Safety password

- **FR-018**: The safety password (中文：安全密码) MUST be exactly four half-width ASCII digits, independent of the login
  password, and stored only as a slow, deliberately expensive one-way hash. It MUST never be returned by any
  endpoint, written to logs, or echoed in an error body.
- **FR-019**: The safety password MUST be settable and changeable from the settings page while logged in; a
  change MUST require the current safety password.
- **FR-020**: When no safety password is configured, permanent deletion and emptying MUST be refused with an
  instruction to set one first, and the interface MUST offer that setup at the point of refusal.
- **FR-021**: Every permanent-delete and empty-bin request MUST carry the safety password and MUST be verified
  for that request only. Verification MUST NOT be cached, remembered for a time window, or reused by a later
  request; a single empty-bin request verifies once for all the entries it removes.
- **FR-022**: A failed verification MUST execute nothing, MUST return a generic failure that does not reveal
  the entry's state, and MUST count towards lockout. Verification MUST happen before the entry is resolved, so a
  caller without the safety password cannot distinguish an existing entry from a nonexistent one.
- **FR-023**: Five consecutive failed verifications within a session MUST lock safety-password verification for
  five minutes; while locked, even correct digits MUST be refused, and the remaining wait MUST be reported. A
  successful verification or the expiry of the lock resets the failure count. Every check of the safety password
  — including the "current password" check when changing it — shares this one counter and this one lock.
- **FR-024**: The lockout MUST be enforced server-side and apply to every client (failed attempts through the
  API count exactly like failed attempts through the dialog), so the interface cannot be used to bypass it.
- **FR-025**: A logged-in administrator MUST be able to reset a forgotten safety password by entering the login
  password on the settings page; the reset MUST clear the stored safety password and the lockout, and a new
  safety password MUST be required before the next sensitive action.
- **FR-026**: The server CLI MUST provide a documented command that clears the safety password and any lockout
  without requiring a login; it MUST neither accept the password as a command-line argument nor print it.
- **FR-027**: Reading the bin MUST be limited to an authenticated administrator session. API tokens of any role
  and guest visitors MUST be denied all recycle-bin endpoints; permanent deletion and emptying MUST additionally
  pass the safety-password check.

### Lifecycle, capacity and interface

- **FR-028**: Retention MUST be configurable in days with a default of 30; the value zero MUST mean "keep
  forever". A change MUST apply to entries already in the bin, without waiting for the next sweep.
- **FR-029**: The maximum bin size MUST be configurable in GiB with a default of 0 (unlimited); the value zero MUST
  mean "unlimited", so the smallest opt-in cap is 1 GiB. When the total exceeds the limit, entries MUST be evicted oldest-first until the total is within
  the limit, except that the most recently deleted entry MUST NOT be evicted by capacity even if it alone
  exceeds the limit.
- **FR-030**: Retention and capacity MUST be enforced automatically, at least once per day, without any user
  action, and immediately when a setting that tightens them is saved.
- **FR-031**: Each entry MUST display its remaining retention time; removals performed by the automatic sweep
  MUST be recorded in the activity log as system-initiated, with the reason (expired or over capacity).
- **FR-032**: The settings page MUST show the bin's current usage (entries and bytes), the configured retention
  and capacity, and what happens on the next sweep.
- **FR-033**: The web interface MUST provide a recycle bin page listing entries newest-deleted first, each with
  a preview image, title, deletion time, size, page count, remaining retention time and original path, with
  per-entry restore and permanent-delete actions, an empty-bin action, a usage summary and text filtering. No
  sort control is required; the order is fixed.
- **FR-034**: Every archive-deletion affordance in the interface MUST state that the archive is moved to the
  recycle bin and MUST offer a one-action undo immediately after a successful delete, without requiring the
  administrator to open the bin page first.
- **FR-035**: All new interface text MUST follow the application's existing localisation scheme, and every new
  dialog MUST trap focus, close on Escape, and return focus to the element that opened it. Every four-digit
  safety-password entry MUST use one segmented field with a slot per digit that accepts a pasted whole code and
  announces each slot's position to assistive technology, and every new colour MUST come from the active theme
  (all five themes), never from a value hardcoded for one of them.
- **FR-036**: Setting, changing or resetting the safety password MUST be recorded in the activity log with the
  acting identity and a timestamp, and MUST never record the password itself or any part of it.

### Key Entities *(include if feature involves data)*

- **Recycle Entry**: one recycled archive. Holds the archive identity, title, size, page count, original path,
  deletion time, deleting actor, the stored file's location inside the bin, and the metadata snapshot needed to
  restore the archive (tags, summary, categories, bookmarks, stamps, reading progress). Has a lifecycle of
  ready → restored or permanently deleted, plus an un-restorable state for a missing or mismatched file.
- **Safety Password**: the instance's four-digit secondary credential. Holds only a one-way hash and the
  lockout state (consecutive failures, locked-until time); never a plaintext value.
- **Recycle Policy**: the two bounds on the bin — retention in days (0 = forever) and maximum total size in bytes
  (0 = unlimited) — with defaults of 30 days and no size cap.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of user-initiated archive deletions — single, batch and token — appear in the recycle bin and
  can be restored within the retention window; no delete path bypasses the bin (verified across every deletion
  entry point in the interface and the API).
- **SC-002**: A restore returns title, tags, summary, page count, categories, bookmarks, stamps and reading
  progress with 100% fidelity in the end-to-end fixture, and lands at the original path when it is still free.
- **SC-003**: A full rescan of a library holding 20 recycled archives re-catalogues zero of them, and none of
  them appear in search, category contents or tankoubon listings while recycled.
- **SC-004**: With five-attempt lockouts of five minutes, an exhaustive 10,000-combination guessing attack takes
  at least six days against a single session, and a locked session refuses even the correct digits.
- **SC-005**: After any sweep, the bin's total size is at most the configured cap plus the size of the most
  recently deleted entry, and no entry older than the retention window remains within 24 hours.
- **SC-006**: Restoring a wrongly deleted 1 GiB archive takes under 30 seconds of wall-clock time from opening
  the bin page, and the delete operation itself completes without copying the file's bytes.
- **SC-007**: Existing delete, search and share behaviour is unchanged for every caller: the current contract
  tests for archive deletion, batch deletion and search still pass without modification.

## Assumptions

- One administrator per instance (the project's existing model); the bin is instance-wide rather than per user,
  and no per-user bin isolation is needed.
- The library and the bin live on the same filesystem — the bin is inside the library directory tree — so moving
  an archive is a rename, not a byte copy; a deployment whose library spans mounts is not supported for the bin.
- The recycle bin stores bytes, so it consumes the same disk as the library; the size cap is what bounds it, not
  a separate quota system.
- Four digits is accepted as sufficient entropy for this credential because it only guards actions available to
  an already-authenticated session, is verified per action, and is rate-limited by lockout; the login password
  remains the primary credential and nothing here weakens it.
- The bin's inventory is authoritative in Redis alongside the rest of an instance's data; the stored files are
  the payload. Backup/export of the library does not need to include the bin.
- Deleting into the bin does not require the safety password, and neither does reading or restoring — only
  irreversible actions do.
- The safety-password reset through the settings page requires only the login password, by explicit choice: it
  is accepted that this makes the safety password a second confirmation rather than an independent secret, and
  the CLI path exists for the case where even that is unavailable.
- The existing delete responses keep their current fields; the bin adds new fields and new endpoints rather than
  changing the shape existing clients depend on.
- Out of scope: a recycle bin for categories, stamps, plugins, or download-queue entries; per-user
  bins; restoring to a user-chosen location; encrypting the bin; browsing or sharing recycled archives; and any
  change to how archives are deleted from outside the application (a file removed from disk directly is still
  the watcher's and Clean Database's concern).
