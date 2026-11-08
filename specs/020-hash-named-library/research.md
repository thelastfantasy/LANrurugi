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
future move, not needed now, and it would make the relocation path shape more complex).

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

## R3. `_byname` keeps the original names and layout

**Decision**: `_byname/<original relative path>`, filename untouched, `.patch.zip` beside it. No sidecar. The name
says how it is addressed (by name, not by id), and the leading underscore makes it sort before the two-digit bucket
directories in Explorer and Finder — the two places the user actually browses the share — while staying visible,
unlike a dot-directory that Windows and macOS hide by default.

**Rationale**: the directory stays browsable, relocation is a pure move (also inside the same filesystem), and
nothing keyed by id is affected because neither the name nor the id changes. Two shapes in one library is the cost the
user accepted; `_byname` is catalogued by filename, and needs no sidecar to pair with.

**Alternative considered**: rename everything into `{id}.{ext}` buckets (one shape for the whole library, sidecars
pair by filename) — rejected on 2026-11-08 because the user wants `_byname` recognized exactly as before.

## R4. The id algorithm is not changed; duplicate content is rejected

**Decision**: keep `size_aware_id` (SHA-1 of the first 512,000 bytes plus the file length).

**Rationale**: changing it would rekey the entire existing library and every share reference. The cost is that two
files sharing a prefix and a length collide; they are rejected as duplicates with the existing archive's id named.

**Alternative considered**: full-file SHA-256 (a true content address, but a full extra read on every ingest and
rescan, and a relocation that must rehash everything).

## R5. Titles are derived in Rust, not by a plugin

**Decision**: `title_from_filename()` in `lanrurugi-scanner`, using the existing `release_markers` token list; an
empty result falls back to the filename with its extension removed.

**Rationale**: the title must exist even when no metadata plugin is installed or enabled, and the marker list already
exists for grouping. Plugins may still override the title through the metadata path afterwards.

## R6. Anything outside the hash layout is relocated into `_byname` automatically

**Decision**: on every scan, an archive whose stored path is neither `<ab>/<id>.<ext>` nor under `_byname/` is moved
to `_byname/<path relative to the library root>`, keeping its filename; its `.patch.zip` moves with it. A target path
already occupied by a different file is skipped and reported, never overwritten.

**Rationale**: one rule covers both the pre-existing library and anything the user drops in later, so the library root
holds only machine-named archives; the user's own folder structure survives unchanged inside `_byname`; and because
`original_path` is recorded, moving a file back is a plain `mv`.

**Alternatives considered**: leave hand-dropped files in place (rejected by the user on 2026-11-08: the root should
hold only hash-named archives); a one-time migration job (rejected — it would need to run again for every later drop,
which is two mechanisms for one rule).

## R7. Relocation runs inside the scanner, not as a job

**Decision**: the scanner performs the move while cataloguing: compute the id, keep or create the record, then, if the
path is outside the hash layout and outside `_byname`, move it and update `file` in the same step. Interruption is
safe because the id and the record do not change: the next scan either still sees the old path (record updated then) or
the new one (already consistent).

**Rationale**: the rule is continuous; there is no plan to preview or journal to replay, and correctness follows from
the content-derived id rather than from bookkeeping. The move is a rename in the common case, so no extra space is
needed.

**Alternatives considered**: a journaled job with preview/rollback (needed only for a one-shot migration, which R6
rejects); renaming into the hash layout instead of `_byname` (that would rekey the whole library — see R4).

**Cross-filesystem fallback**: if `_byname` is on a different filesystem from the source, the move fails with
`EXDEV`; the scanner falls back to copy → fsync → rename into place → unlink source, and never unlinks before the copy
is durable.

## R8. Ordering at ingest: bytes, record, sidecar

**Decision**: rename the staged file into `{id}.{ext}`, create the archive record, then write the sidecar.

**Rationale**: the bytes are the truth. A crash after the rename but before the record leaves a file the next scan
finds and catalogues (writing the sidecar then); a sidecar written before the record could describe an archive that
never existed.

## R9. Patch sidecars keep the same-directory convention

**Decision**: `patch_path_for` stays "same directory, same stem, `.patch.zip`"; with hash naming that means
`<ab>/<id>.patch.zip`.

**Rationale**: no code change, and relocation moves patches with their archive because the stem moves.

## R10. Removing the rename endpoint is compatibility-safe

**Decision**: delete `PUT /api/archives/{id}/rename` and its frontend caller.

**Rationale**: it is additive API this project introduced; legacy never served it (`crates/lanrurugi-api/src/archives.rs:1837`
says so). No upstream client uses it. The spec records the removal; the documentation sidecar entry is deleted with it.

## R11. `original_filename` is backfilled from the on-disk name during relocation

**Decision**: for records created before this feature, set `original_filename` and `original_path` from the path the
file has when the scanner first sees it under this version.

**Rationale**: it is the only name that exists for those records, and relocation is the last moment the old
name is still available. Backup import then matches on `original_filename` instead of the path.

## R12. Extension handling is single-suffix and lowercased

**Decision**: extension = the text after the last `.` in the basename, lowercased, with no dot when absent.

**Rationale**: the scanner dispatches on a known-extension table; multi-suffix names (`.tar.gz`) are not an archive
format it supports. A file without a known extension still gets a stored name and is catalogued as `Unknown`.
