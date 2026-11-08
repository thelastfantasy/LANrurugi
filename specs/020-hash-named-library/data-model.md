# Phase 1 — Data Model: Hash-Named Library

## Archive record (Redis)

The existing archive hash keeps every field it has today. One field is added:

| Field | Type | Notes |
|---|---|---|
| `id` | `ArchiveId` (newtype over the content-derived hex) | unchanged; also the stored file's stem |
| `file` | string | `<library>/<ab>/<id>.<ext>` for archives ingested after this feature, or the unchanged path under `_byname/`; still an openable absolute path |
| `title` | string | defaults to `title_from_filename(original_filename)`; editable; plugins may override |
| `original_filename` | string, **new** | verbatim filename as the source gave it, including extension; empty only for records created before this feature and not yet seen by a scan |
| `extension` | string | derived at ingest, lowercase, no dot |
| `pagecount`, `size`, timestamps, tags, etc. | unchanged | |

`original_filename` is additive: serialization must default it to `""` when absent so an older record still parses.
It is never used to build a path.

## Stored-path invariants

Given library root `L` and archive `A`:

```
basename(A)      = A.id + ("." + A.extension)          # extension may be empty; hash layout only
bucket(A.id)     = A.id[0..2]                          # two lowercase hex characters
live_path(A)     = L + "/" + bucket + "/" + basename
byname_path(A)  = L + "/_byname/" + A.original_path # filename and layout kept as they were
patch_path(A)    = same directory: stem + ".patch.zip"
sidecar_path(A)  = same directory as live_path, stem = A.id, suffix ".json"   # hash layout only
```

Invariants enforced by tests:

1. `basename` length ≤ 2 + 64 + 1 + extension; no filesystem limit can be reached.
2. No two archives share `live_path` — the id is the stem and duplicates are rejected at ingest.
3. A sidecar never shares a name with an archive because its extension is `.json`, which the scanner never treats as
   an archive. Sidecars exist only beside hash-layout archives; a `_byname` archive has none.
4. The bucket directory is exactly two characters and contains only archives, patches and sidecars.
5. `_byname` preserves each archive's filename and relative path, so a same-named pair in two subdirectories stays
   distinct and nothing there is renamed by the application.

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
- `original_path` is the path relative to the library root when the application first saw the file (before any
  relocation), kept so a file can be moved back by hand and so "where did this come from" survives the flat layout.
- Unknown fields are ignored on read; `version` > 1 is read best-effort and the file is rewritten at the current
  version on the next write.
- A missing or unparsable sidecar never blocks cataloguing; it is logged, regenerated from the record, and counted in
  the rebuild report.

## Relocation rule

The scanner applies one rule while cataloguing, for every archive it sees:

```text
if path is under <library>/_byname/            → leave it alone
else if path == <library>/<ab>/<id>.<ext>       → leave it alone
else                                            → move to <library>/_byname/<path relative to library root>,
                                                  move its .patch.zip by the same prefix, update `file`
```

Properties:

- **Idempotent**: the second run sees the file under `_byname/` and does nothing.
- **Interrupt-safe**: the id and the record do not change, so a file found at the old path is simply moved on the next
  run, and one found at the new path is consistent.
- **Never overwrites**: if the destination exists with different content, the move is skipped and reported.
- **Cross-filesystem**: `EXDEV` falls back to copy → fsync → rename → unlink, never unlinking before the copy is
  durable.
- **Reversible by hand**: `original_path` on the record and in the sidecar says where the file came from.

The rule is what replaces a one-shot migration job: it covers the pre-existing library on the first scan and every
file added later, with no separate state machine to resume or roll back.

## Rebuild-index behaviour

For each discovered file (sidecars excluded):

1. Compute the id from the bytes; if it disagrees with the filename stem, the content wins and the mismatch is
   reported.
2. If a record exists, it is authoritative; a disagreeing sidecar is rewritten (hash layout only).
3. If no record exists, build one from the filename and, for hash-layout files, the sidecar (if readable); then write
   the record and, for hash-layout files, the sidecar.

The report counts recovered, regenerated-sidecar and mismatched entries.
