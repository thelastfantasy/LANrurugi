# Phase 1 — Data Model: Hash-Named Library

## Archive record (Redis)

The existing archive hash keeps every field it has today. One field is added:

| Field | Type | Notes |
|---|---|---|
| `id` | `ArchiveId` (newtype over the content-derived hex) | unchanged; also the stored file's stem |
| `file` | string | now always `<library>/<ab>/<id>.<ext>` (or the path under `.classic/`); still an openable absolute path |
| `title` | string | defaults to `title_from_filename(original_filename)`; editable; plugins may override |
| `original_filename` | string, **new** | verbatim filename as the source gave it, including extension; empty only for records created before this feature and not yet migrated |
| `extension` | string | derived at ingest, lowercase, no dot |
| `pagecount`, `size`, timestamps, tags, etc. | unchanged | |

`original_filename` is additive: serialization must default it to `""` when absent so an older record still parses.
It is never used to build a path.

## Stored-path invariants

Given library root `L` and archive `A`:

```
basename(A)      = A.id + ("." + A.extension)          # extension may be empty
bucket(A.id)     = A.id[0..2]                          # two lowercase hex characters
live_path(A)     = L + "/" + bucket + "/" + basename
classic_path(A)  = L + "/.classic/" + bucket + "/" + basename
patch_path(A)    = same directory, stem = A.id, suffix ".patch.zip"
sidecar_path(A)  = same directory, stem = A.id, suffix ".json"
```

Invariants enforced by tests:

1. `basename` length ≤ 2 + 64 + 1 + extension; no filesystem limit can be reached.
2. No two archives share `live_path` — the id is the stem and duplicates are rejected at ingest.
3. A sidecar never shares a name with an archive because its extension is `.json`, which the scanner never treats as
   an archive.
4. The bucket directory is exactly two characters and contains only archives, patches and sidecars.

## Sidecar document (on disk)

```json
{
  "version": 1,
  "id": "5fff1a46220b45cf8db24c1721062b7e20fdb8dc",
  "original_filename": "[Circle] Some Title (2024) [DL版].zip",
  "original_path": "downloads/2024/[Circle] Some Title (2024) [DL版].zip",
  "title": "Some Title",
  "extension": "zip",
  "size": 162575115,
  "mtime": "2026-11-08T13:00:20Z",
  "written": "2026-11-08T13:02:11Z"
}
```

- Written with `tmp + fsync + rename` in the same directory.
- `original_path` is the path relative to the library root at ingest time (or the pre-migration path for migrated
  archives), kept for rollback and for the "where did this come from" question after the directory is flat.
- Unknown fields are ignored on read; `version` > 1 is read best-effort and the file is rewritten at the current
  version on the next write.
- A missing or unparsable sidecar never blocks cataloguing; it is logged, regenerated from the record, and counted in
  the rebuild report.

## Migration plan entry

```rust
enum PlannedAction {
    Move { id: ArchiveId, from: PathBuf, to: PathBuf, patch: Option<(PathBuf, PathBuf)> },
    Merge { keep: ArchiveId, drop_path: PathBuf, reason: CollisionReason },
    Skip { path: PathBuf, reason: SkipReason },
}

enum SkipReason { AlreadyHashed, NotCatalogued, Unsupported, ReadOnly, Unreadable }
```

Collisions (`Merge`) happen only when two files have the same id; the plan names the kept archive and the dropped path
and **never** overwrites silently.

## Migration journal

One JSON object per line, appended and flushed before the rename it describes:

```json
{"op":"move","id":"…","from":"…","to":"…","patch_from":"…","patch_to":"…","at":"…"}
{"op":"sidecar","id":"…","path":"…","at":"…"}
{"op":"backfill","id":"…","original_filename":"…","at":"…"}
```

Replay rules:

- **Resume**: an entry whose `to` already exists and whose `from` does not is considered done.
- **Rollback**: reverse order; `move` renames `to → from`; `backfill` restores the previous value recorded in the same
  line; a missing `from` (file was deleted by the user) is reported and skipped.
- **Journal location**: `<library>/.classic/.migration/<job-id>.jsonl`; the directory is excluded from scanning and
  watching, and a finished job's journal is kept so rollback remains possible until the user discards it.

## Migration job states

```
idle → planning → previewed → running → completed
                     │           ├→ cancelled (resumable)
                     │           └→ failed (resumable, journal intact)
                     └→ discarded
completed → rolling_back → rolled_back
```

`previewed` holds counts only (`move`, `merge`, `skip` per reason) plus a bounded sample for display; the full plan is
recomputed at start so a scan between preview and run cannot be acted on stale.

## Rebuild-index behaviour

For each discovered file (sidecars excluded):

1. Compute the id from the bytes; if it disagrees with the filename stem, the content wins and the mismatch is
   reported.
2. If a record exists, it is authoritative; a disagreeing sidecar is rewritten.
3. If no record exists, build one from the filename, the sidecar (if readable) and the file metadata; write it.

The report counts recovered, regenerated-sidecar and mismatched entries.
