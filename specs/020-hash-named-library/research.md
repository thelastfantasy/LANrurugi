# Phase 0 — Research: Hash-Named Library

All decisions below were settled while writing the spec (2026-11-08) and are recorded with the alternatives they
beat. The spec's Assumptions section carries the user-facing consequences.

## R1. Fanout: two hexadecimal characters, one level

**Decision**: `<library>/<ab>/<id>.<ext>`, where `ab` is the first two characters of the id.

**Rationale**: 256 buckets put ~390 archives in each at 100,000 archives and ~3,900 at 1,000,000 — both fine for
`readdir`, backups and SMB browsing. It keeps the library root listable (256 entries), which matters because the
library directory is the user's own directory and is browsed by humans, `ls`, `rsync` and SMB clients.

**Alternatives considered**: no fanout (100,000 entries in one directory — the reason this feature exists is not to
trade a name-length limit for a directory-size limit); four characters (65,536 directories in the root — the root
becomes the new unusable directory); two levels of two (65,536 leaves but the same 256 root entries — a reasonable
future move, not needed now, and it would make the migration path shape more complex).

## R2. Sidecar per archive, not one map

**Decision**: `<ab>/<id>.json` next to `<ab>/<id>.<ext>`.

**Rationale**: each sidecar is written with `write tmp → rename`, so a crash leaves either the old or the new document;
there is no lock, no torn global file and no write amplification. Moving or deleting an archive moves or deletes its
sidecar. Restoring a lost database reads one small file per archive.

**Alternatives considered**: a single library-level JSON (40–80 MB at 100k, rewritten on every title edit, and a torn
write loses everything); one map per bucket (~150 KB each, needs a bucket lock and loses ~390 entries on a torn write);
appending JSONL (needs compaction and half-line tolerance).

**Rule**: Redis is authoritative; the sidecar is a derived snapshot. Rebuild uses it only where a record is missing,
and any disagreement is reported and the sidecar rewritten.

## R3. Existing archives are renamed to `{id}.{ext}` as they move

**Decision**: `.classic/<ab>/<id>.<ext>`, with `{id}.patch.zip` beside it.

**Rationale**: one shape for the whole library means sidecar pairing, patch pairing and scanning need no special case;
the rename is free inside the same filesystem, and the id does not change, so nothing keyed by id is affected.

**Alternative considered**: keep old names inside `.classic` (the library keeps two shapes forever, and sidecars could
not be paired by filename).

## R4. The id algorithm is not changed; duplicate content is rejected

**Decision**: keep `size_aware_id` (SHA-1 of the first 512,000 bytes plus the file length).

**Rationale**: changing it would rekey the entire existing library and every share reference. The cost is that two
files sharing a prefix and a length collide; they are rejected as duplicates with the existing archive's id named.

**Alternative considered**: full-file SHA-256 (a true content address, but a full extra read on every ingest and
rescan, and a migration that must rehash everything).

## R5. Titles are derived in Rust, not by a plugin

**Decision**: `title_from_filename()` in `lanrurugi-scanner`, using the existing `release_markers` token list; an
empty result falls back to the filename with its extension removed.

**Rationale**: the title must exist even when no metadata plugin is installed or enabled, and the marker list already
exists for grouping. Plugins may still override the title through the metadata path afterwards.

## R6. Hand-dropped files stay where they are

**Decision**: a file the user places in the library is catalogued in place, with its filename as `original_filename`;
the application never renames it on its own. Adoption into the hash layout is an explicit action (not in this
feature's scope).

**Rationale**: the library directory is the user's; silently renaming files someone just copied in over SMB is exactly
the surprise the old scheme produced. It also keeps the "drop files in" workflow working unchanged.

## R7. Migration is a journaled rename job in the existing job registry

**Decision**: a job in `lanrurugi_core::jobs` with three phases (plan → preview → execute) and a journal file under
`.classic/.migration/<job-id>.jsonl`; each line records `from`, `to`, `id`, `sha?` before the rename completes.

**Rationale**: rename-only is fast and needs no extra space; the journal makes an interrupted run resumable (skip
entries already at `to`) and reversible (reverse the lines). The job registry already gives the console progress and
cancellation.

**Alternatives considered**: copy-then-delete (needs double space, slow for 100k); a separate CLI-only migration (the
user asked for a settings-page action, and the console already exists).

**Cross-filesystem fallback**: if `.classic` is on a different filesystem from the source, `rename` fails with
`EXDEV`; the job falls back to copy → fsync → rename into place → unlink source, and never unlinks before the copy is
durable.

## R8. Ordering at ingest: bytes, record, sidecar

**Decision**: rename the staged file into `{id}.{ext}`, create the archive record, then write the sidecar.

**Rationale**: the bytes are the truth. A crash after the rename but before the record leaves a file the next scan
finds and catalogues (writing the sidecar then); a sidecar written before the record could describe an archive that
never existed.

## R9. Patch sidecars keep the same-directory convention

**Decision**: `patch_path_for` stays "same directory, same stem, `.patch.zip`"; with hash naming that means
`<ab>/<id>.patch.zip`.

**Rationale**: no code change, and the migration moves patches with their archive because the stem moves.

## R10. Removing the rename endpoint is compatibility-safe

**Decision**: delete `PUT /api/archives/{id}/rename` and its frontend caller.

**Rationale**: it is additive API this project introduced; legacy never served it (`crates/lanrurugi-api/src/archives.rs:1837`
says so). No upstream client uses it. The spec records the removal; the documentation sidecar entry is deleted with it.

## R11. `original_filename` is backfilled from the on-disk name during migration

**Decision**: for records created before this feature, set `original_filename` from the basename of `file` (minus the
extension for display where needed, kept verbatim otherwise).

**Rationale**: it is the only name that exists for those records; the migration is the one moment the old name is still
available, so it must be captured then. Backup import then matches on `original_filename` instead of the path.

## R12. Extension handling is single-suffix and lowercased

**Decision**: extension = the text after the last `.` in the basename, lowercased, with no dot when absent.

**Rationale**: the scanner dispatches on a known-extension table; multi-suffix names (`.tar.gz`) are not an archive
format it supports. A file without a known extension still gets a stored name and is catalogued as `Unknown`.
