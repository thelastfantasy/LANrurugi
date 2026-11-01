# Research: Recycle Bin and Safety Password

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Date**: 2026-10-31

Each section records the decision, why it is the right one for this codebase, and what was rejected. Every claim
about existing behaviour below was checked against the source named in the text, not from memory.

## 1. Where the bin's bytes live

**Decision**: `<library>/.lanrurugi-recycle/<archive_id>/payload/<original filename>` plus
`<archive_id>/meta.json`, with `<library>/.lanrurugi-recycle` reserved and excluded from every library walk.

**Rationale**: The library and the bin are then on the same filesystem (the bin is inside the library), so
delete and restore are a `rename` — O(1), no byte copy, works for a 10 GiB archive, and the payload can never be
half-copied by a crash. A directory name starts with a dot, keeping it out of casual browsing and out of the
"first-level subfolder" grouping features conceptually, and `<archive_id>` as the directory name makes an
otherwise orphaned directory self-identifying during reconciliation (§3).

**Exclusion must happen in one place.** `watcher::is_watched_archive_path`
(`crates/lanrurugi-scanner/src/watcher.rs`) is already the single filter used by (a) the `notify` watcher, (b)
`full_scan::walk`, (c) `scripts::walk_subfolders` and (d) `scripts::collect_archives_recursive`, and the `thumb`
exclusion there is the precedent for exactly this kind of reserved directory. Adding a `.lanrurugi-recycle`
component check to that function excludes the bin from all four at once. It also prevents the real race that
would otherwise exist: moving a file **into** the bin emits a create event inside the watched `archive_dir`, and
without the filter the watcher would immediately try to catalogue the archive that was just deleted.

**Rejected**: a bin outside the library (needs a second configured path, breaks the "one library" mental model,
and risks `EXDEV` for deployments with separate mounts); keeping the file in place and hiding the record in Redis
(the file stays where users and file managers see it, and a rescan/full scan would re-catalogue it — violating
FR-004/FR-005); moving to `temp_dir` (its size is already bounded by `tempmaxsize`, whose sweep would delete
users' recoverable data).

## 2. Inventory: Redis hash plus an on-disk journal

**Decision**: one hash, `LANRURUGI_RECYCLE_BIN` on the **archive** logical DB (`redis.archive`, the DB every
archive-scoped repository already uses), field = archive id, value = JSON `RecycleEntry`. Each entry directory
also carries a `meta.json` with the same content, written before the Redis record and used only for recovery and
for self-description on disk.

**Rationale**: Redis is this project's only store (constitution), and the entry is archive-scoped data, so the
archive DB is the natural placement — unlike the `config` DB used for settings/caches. A hash gives O(1) get by
id, one `HGETALL` for the whole listing (the bin is small and bounded by design), and no pagination index to
maintain. `meta.json` is the safety net for the one window Redis cannot protect: a crash between "file renamed
into the bin" and "hash written". Because the directory name is the archive id, even a directory without a
readable `meta.json` can still be reconciled against a still-present archive record.

**Rejected**: an SQLite/embedded DB (constitution forbids a second store without an amendment); one Redis key per
entry (a `SCAN` to list, and no single place to compute usage); relying on disk alone (would make every listing a
directory walk plus a JSON parse per entry, and Redis is already required for the archive record).

## 3. Delete ordering and the two crash windows

**Decision**: the sequence for recycling one archive is:

1. take the per-entry bin lock and `state.lock_filename(<basename>)` (same lock the scanner/upload paths use);
2. read the archive record and snapshot everything restore needs (§5);
3. `create_dir_all(bin/<id>/payload)`, rename the file and its sidecar `.patch.zip` into it;
4. write `meta.json` atomically (temp file + rename);
5. `HSET` the entry into `LANRURUGI_RECYCLE_BIN`;
6. `HDEL LRR_FILEMAP[original_path]` — the file is gone, and a different file later arriving at that path must
   not be re-keyed against this recycled id (`pipeline.rs` treats a known path as "same archive, maybe rewritten"
   and calls `rename_id`);
7. run the existing purge tail: delete the record, search-index entries, recommend cache, category memberships,
   bookmarks, stamps, download-queue rows — **but not** the thumbnails (§10);
8. record the activity entry and return success.

Failures before step 3 change nothing. A failure at 3 fails the delete and leaves the archive fully intact. A
failure after a successful rename triggers a best-effort roll-back (rename back out, remove the hash field); if
the roll-back itself fails, the log records it and startup reconciliation finishes the job.

**Crash windows and reconciliation** (run at startup and exposed as part of the bin listing's `leftovers`):

| Disk state | Redis state | Archive record | Action |
|---|---|---|---|
| dir + payload | entry present | present | A delete crashed after step 6: finish step 7 (the user asked for the delete, the payload is safe). |
| dir + payload, no `meta.json` | no entry | present | Rebuild `meta.json` from the archive record, then finish the delete. |
| dir + `meta.json` | no entry | absent | The HSET was lost or Redis was restored from a backup: adopt the entry. |
| dir + payload | no entry | absent, no `meta.json` | Incomplete or foreign leftover: report it; the sweep removes it only after a 24 h grace period. |
| entry present | no payload | any | Mark the entry `un-restorable`, keep it listed, allow permanent removal (FR-012). |

**Rationale**: Writing the journal and the hash before removing the record means the recoverable copy always
exists before the authoritative record disappears; a crash can only ever leave *more* recoverable data than an
equivalent non-bin delete, never less. Finishing a half-done delete rather than resurrecting the archive is
consistent with the user's own request, and the payload remains restorable either way.

## 4. Restore path, conflicts and the watcher race

**Decision**: restore is the inverse sequence: lock entry + target filename, decide the destination, rename the
payload out, `archives.save` the re-created record, re-apply memberships, remove the entry and hash field.

Destination resolution, checked in order:

1. an archive with the same id already exists → refuse with a conflict error (FR-008) — ids are content-derived
   (`storage/id.rs`), so this is an expected case, not a bug; offer permanent delete of the bin copy.
2. original absolute path is free → use it exactly.
3. original path occupied by a different file → append ` (restored)`, then ` (restored 2)`, … before the
   extension until free (FR-007).
4. original directory missing or outside the current library → library root + original filename (FR-009).

The response returns the final path so the UI can tell the user where the archive actually landed.

**Watcher race**: renaming a file **out** of the bin into the library emits a create event that the watcher will
try to ingest. Restore therefore holds `state.lock_filename(<basename>)` around the rename + record save, the
same lock `download_manager::ingest` uses for the identical "renamed into the watched dir while we catalogue it
ourselves" race (see `AppState::filename_locks`'s own doc comment). While holding the lock it writes the
filemap entry for the destination path, but with the **scanner-computed** id (`size_aware_id` of the restored
payload), not blindly the stored id. That distinction matters:

- For an archive already catalogued under the size-aware algorithm (the normal case) the two ids are equal and
  the watcher resolves the rename to `Unchanged`, exactly as intended.
- For a migrated archive still carrying a **legacy** id (§: `storage/id.rs`), writing the legacy id into the
  filemap would make the watcher compute a different size-aware id and call `rename_id`, silently re-keying the
  record — and `rename_id` only renames the archive hash, so the bookmarks and stamps restore just re-created
  would be orphaned (only `rebuild::update_references` fixes those, and it is not on this path). Writing the
  computed id instead makes the restore event a no-op and keeps the record's identity (FR-006/FR-008) and the
  user data intact.

The cost of the legacy case is confined to a later *content change* of that file, which hits the same
"filemap points at an id with no record" fall-through any archive with a stale filemap entry already takes; no
restore-specific corruption. The rebuild-index path remains the supported way to migrate legacy ids
wholesale.

**Rejected**: letting the watcher do the restore (it would re-hash the file, lose the snapshot metadata, and
cannot restore memberships); restoring to a user-chosen path (out of scope per spec).

## 5. What a restore must put back

**Decision**: snapshot in `meta.json`/Redis — archive record fields (`name`, `title`, `tags`, `summary`,
`arcsize`, `pagecount`, `isnew`, `lastreadpage`, `lastreadtime`, `thumbhash`, `toc`, `stamp_ids`, `has_patch`,
`corrupted_pages`, `heal_failed_at`), the version-history chain, the list of category ids from
`categories.for_archive`, the bookmarks from `bookmarks.list_for_archive` (page, timestamp, name), and the full
`Stamp` records for `archive.stamp_ids` (`stamps.get`).

Restore writes the record with `archives.save`, re-adds categories through `categories::add_archive_to_category`,
re-adds bookmarks through `bookmarks.add` + `set_name`, re-inserts stamps with `stamps.restore_raw` (which
already exists for the rebuild-index path and is exactly the right primitive), and restores version history with
`archives.set_version_history`.

**Rationale**: today `delete_one_archive` destroys all of this, and a bin that restores only the file would
silently make the user re-enter years of metadata — the exact loss the feature exists to prevent. All the
primitives already exist, so the snapshot is a list of existing calls rather than new storage machinery.
Tankoubon/grouping membership needs no snapshot: `Grouping.archives` holds the identity, so the restored archive
reappears in its groups automatically.

## 6. Safety password: format, storage, verification

**Decision**: exactly four ASCII digits, hashed with `lanrurugi_core::password::hash_password` (bcrypt,
`DEFAULT_COST`, stored `{CRYPT}`-tagged like the login password), kept in `LRR_CONFIG` under
`safety_password_hash` + `safety_password_set_at`. It is excluded from the generic settings GET/PUT exactly like
`password`; dedicated endpoints are the only way in or out. Endpoints never return the hash and never echo the
submitted digits.

**Rationale**: reusing the existing password module keeps one hashing policy, one legacy-compatible format, and
one dependency; bcrypt at the default cost makes each guess ~0.3 s of server CPU, which combined with lockout
turns a 10,000-combination space into a multi-day attack. Validation is a single `len == 4 && all(is_ascii_digit)`
check applied on the server; the frontend additionally uses `inputmode="numeric"` but is never trusted.

**Rejected**: Argon2 (a second password-hashing dependency for no security gain at this entropy level);
plaintext/comparison in Redis (violates Principle V); a longer PIN (the user explicitly asked for four digits).

## 7. Lockout: keying, thresholds, reset

**Decision**: failures are counted per **login session family** — `AuthContext::session_family_id`, the `fid`
claim already carried for activity/device labelling. Keys on the `config` DB:

- `LANRURUGI_SAFETY_FAIL:<family>` — integer counter, TTL 300 s, incremented on each wrong PIN.
- `LANRURUGI_SAFETY_LOCK:<family>` — presence + TTL means locked; set when the counter reaches 5, TTL 300 s.

While the lock key exists, verification is refused *before* bcrypt runs, so a correct PIN cannot shortcut it;
the response reports `locked_until` so the UI can count down. A successful verification deletes the counter; the
lock expiring naturally clears the lock and the counter. A session whose access token lacks `fid` falls back to
the literal family `"session"` (documented; such cookies predate the current login flow).

**Rationale**: keying by family means a second browser does not inherit a first browser's failures, while a
script that logs in fresh still cannot brute force faster than five guesses per five minutes per family — the
spec's SC-004 maths (10,000 / 5 × 5 min ≈ 6.9 days). bcrypt's own cost is the backstop if an attacker somehow
obtains the hash. The check lives in the shared verification helper, so UI and API cannot diverge (FR-024).

**Rejected**: per-IP keying (shared NATs lock each other out; `AuthContext::client_ip` is explicitly documented
as diagnostic-only); a global instance lock (one attacker could deny the owner the bin indefinitely); a
time-window "unlock for 15 minutes" (115-style convenience, but it would let one verification authorise later
irreversible actions — FR-021 requires per-request verification).

## 8. Reset paths

**Decision**: two, as the user chose.

- **Settings page**: `POST /api/safety-password/reset` with the *login* password (verified against
  `LRR_CONFIG.password` with the existing `password::verify_password`), clearing both the hash and the lockout.
  Requires an authenticated session, so it is not reachable without already holding that password.
- **CLI**: `lanrurugi reset-safety-password --redis-url …` (same env/arg as `serve`,
  `LANRURUGI_REDIS_URL`), which deletes the hash fields and every `LANRURUGI_SAFETY_*` key. It never accepts or
  prints a password.

Both leave "no safety password configured", so the next permanent-delete/empty request is refused with the
setup prompt (FR-020) until a new one is set.

**Rationale**: a self-hosted appliance has no phone/email recovery, and the user explicitly asked for both. The
CLI is the only path that works when the login password is also lost, and it requires host access — the correct
trust boundary for an escape hatch.

## 9. Retention and capacity policy

**Decision**: two numeric settings, stored with the existing `NUMBER_FIELDS` machinery:
`recycle_retention_days` default 30 (0 = forever, otherwise 1–36500) and `recycle_max_size_mb` default 10240
(0 = unlimited). Enforcement:

- delete an entry when `now - deleted_at >= retention_days` (retention 0 disables this check);
- then, while `sum(size) > cap`, evict the oldest entry, **stopping before the newest entry** — the most recent
  mistake must always be recoverable, even when it alone exceeds the cap;
- run on an hourly interval task in `main.rs`, once at startup after reconciliation, and immediately (spawned
  sweep) after a `PUT /settings` that lowers either limit.

Usage (`count`, `bytes`) is computed from the hash values, not from a directory walk, so the listing stays one
round-trip; a reconcile pass also refreshes it when disk and inventory disagree.

**Rationale**: hourly is cheap (one `HGETALL` + stats) and keeps the "no entry survives more than 24 h past its
window" guarantee; immediate enforcement on tightening avoids the confusing interval where the user lowers a
limit and sees nothing happen. The newest-entry carve-out is the one policy that makes a "cap" and "safety net"
compatible at the same time.

**Rejected**: evicting strictly oldest-first including the newest (a user with a 20 GiB archive and a 10 GiB cap
would watch their just-deleted archive vanish instantly); a daily cron only (up to 24 h of slack on tightening);
capacity as a percentage of disk (needs library-size accounting that drifts).

## 10. Thumbnails and other id-keyed data

**Decision**: keep the cover and per-page thumbnails in `thumb_dir` under the recycled id; do not delete them on
recycle. The bin's thumbnail endpoint streams them directly. Permanent delete removes them exactly as today's
delete does. There is no orphan-thumbnail sweep in the codebase today (verified by search); any that is added
later must treat ids present in `LANRURUGI_RECYCLE_BIN` as live — FR-016's second sentence exists for that
future case, not for a mechanism that already runs.

**Rationale**: the bin page's whole value is recognising what was deleted; regenerating a cover requires opening
the archive, and for an encrypted or unreadable archive it may not be possible at all. Thumbnails are keyed by
archive id, and no live archive can share an id with a bin entry (the record was deleted), so there is no
collision risk. Search-index entries, recommendations, category membership, bookmarks and stamps **are** removed
on recycle — they would otherwise surface the archive in the library's own views.

## 11. Authorization

**Decision**: every new endpoint is session-cookie only. `route_policy.csv` gets explicit
`token_admin/token_guest → deny` lines for `/api/recycle-bin*` and `/api/safety-password*`, matching the
`/api/sessions*` and `/api/settings/password` precedent. Guest visitors are denied too (their whitelist is
explicit-allow only, so no rule is needed for them to stay out, but the deny lines keep the intent reviewable).

**Rationale**: the bin can permanently destroy data, and the safety password is an account-security secret.
Existing archive deletion by API token keeps working (and now recycles) — only the bin *management* surface is
session-only, exactly the split the spec asks for.

## 12. Every non-user deletion bypasses the bin

**Decision**: split `delete_one_archive` into `purge_archive_record(...)` (the current cleanup tail) and a
`recycle_or_purge` entry point. Three call sites call `purge_archive_record` directly, with no move and no bin entry:

1. `database.rs::clean_database` — records whose file is already gone;
2. `plugins.rs`'s post-download supersede sweep — a revision retired automatically once a newer one is
   catalogued (the user did not ask to delete it; the bin would silently fill with outdated revisions);
3. `archive_split.rs`'s removal of the original after a successful split (the operation already produced and
   catalogued the replacement archives).

**Rationale**: the bin is an undo for an explicit delete. Automatic replacement is not an undo candidate, and
creating an un-restorable entry for every missing file during cleanup would flood the bin and make routine
maintenance look like a mass deletion. FR-017 lists all three so a future refactor cannot route one through the
bin by accident.

## 13. UI integration

**Decision**: a new `/recycle-bin` page under the existing admin route block, a nav link next to "Bookmarks",
a `RecycleBinSection` on the Settings page (limits, usage, safety-password set/change/reset), a reusable
`SafetyPasswordDialog` for the two irreversible actions, and an "undo" action on the existing delete toast that
calls restore. Strings go through i18next; the 14 locale files get English source keys with translations where
the translator data exists.

**Rationale**: the undo path is what makes the bin useful in the moment of the mistake (FR-034); requiring a trip
to the bin page would be a worse experience than 115 itself. The Settings section is where the other bounded
caches (download-queue retention) already live, so it matches the existing mental model.

## 14. Testing strategy

- **Storage** (`lanrurugi-storage`): repository round-trip, usage aggregation, oldest-first eviction ordering,
  "newest is never evicted" rule — Redis-gated like the existing repository tests.
- **API** (`lanrurugi-server/tests/`): delete → list → restore round-trip with metadata/memberships; batch delete
  recycling every id; token cannot reach the bin; guest denied; PIN setup/verify/lockout with the real clock
  (5 slow bcrypt calls are acceptable) and a direct check that a locked session refuses the correct PIN; empty
  with a wrong PIN changes nothing; retention/capacity sweep using entries with backdated timestamps.
- **Scanner** (`lanrurugi-scanner`): a temp library containing a recycle dir with a valid archive file is fully
  scanned and catalogues 0 of them (SC-003).
- **Frontend**: Vitest for the PIN input validation and the expiry/remaining-time formatting; Playwright for
  delete → undo → restore, and permanent delete/empty through the dialog.
- **Docs gate**: the new routes appear in `docs/api/sidecar/recycle-bin.yaml`; `lanrurugi-api-docgen`'s scan
  fails if they are missing.

## 15. Alternatives considered at the feature level

| Alternative | Why not |
|---|---|
| Soft-delete flag on the archive record, file untouched | Re-catalogued by the watcher/full scan; recycled archives would still be openable via direct path; violates FR-004/FR-005. |
| Bin only in the UI (client-side undo buffer) | A page refresh or a second device loses it; no disk-space bound or audit trail. |
| Safety password as a second *login* factor | The user's intent is protecting irreversible actions inside an authenticated session, not changing login. |
| Unlock window after one PIN verification | Convenient but lets one verification authorise later irreversible actions; FR-021 forbids it. |
| Global bin lock per instance | Simpler than per-entry, but a 1 GiB restore would block every other bin operation; per-entry serialization (a small `Mutex<HashSet>` guard) is enough. |
