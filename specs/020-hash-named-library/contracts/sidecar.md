# Contract: On-Disk Sidecar — Hash-Named Library

## Name and location

```
<library>/<ab>/<id>.json          # live archive
<library>/.classic/<ab>/<id>.json # migrated archive
```

`ab` is the first two characters of the id. There is exactly one sidecar per archive; no sidecar exists without a
matching archive, and a sidecar left without one is reported and removed by the rebuild/sweep (never silently kept).

## Format

JSON object, UTF-8, no trailing newline required. `version` is required; everything else may be missing (a partial
sidecar is better than none).

| Field | Type | Required | Meaning |
|---|---|---|---|
| `version` | integer | yes | `1` today |
| `id` | string | yes | content-derived id, lowercase hex |
| `original_filename` | string | no | verbatim source filename including extension |
| `original_path` | string | no | path relative to the library root at first cataloguing |
| `title` | string | no | title at write time |
| `extension` | string | no | lowercase, no dot |
| `size` | integer | no | byte size at write time |
| `mtime` | string (RFC 3339) | no | file mtime at write time |
| `written` | string (RFC 3339) | no | when this document was written |

## Writing rules

1. Serialize to `<id>.json.tmp` in the same directory, `fsync` it, then `rename` over `<id>.json`.
2. Never write a sidecar for a file that is not yet in its final place.
3. Write after the archive record exists (research R8). A missing sidecar is repaired by the rebuild or the next scan.
4. Any component that changes title or original filename rewrites the sidecar in the same operation.

## Reading rules

1. A missing sidecar is not an error.
2. An unparsable sidecar is logged with its path, ignored, and regenerated on the next write or rebuild.
3. `version` greater than the supported one: read the known fields, log once, rewrite at the supported version on the
   next write.
4. `id` that disagrees with the filename stem: the filename (and the content) win; the disagreement is reported.
5. `original_path` is advisory for humans and rollback; it is never used to build a live path.

## Enumeration rules

The following must ignore `*.json` files inside the library:

- the full scan's `walk()`;
- the watcher (`is_watched_archive_path`);
- the stale/orphan sweep;
- the migration's plan (it enumerates records, but its file discovery must not pick up sidecars);
- the duplicate scanner.

A `.json` file that is *not* named like a sidecar (`<id>.json` with a 40-hex stem) is still ignored — the rule is
extension-based, because the library is the user's directory and they may drop JSON in it.

## Compatibility

- Adding a field is a minor change: readers ignore unknown fields, writers keep them only if they know them.
- Removing a field requires `version` 2 and a documented migration; the rebuild must still read version 1.
- The sidecar is a derived snapshot, not a source of truth: Redis wins whenever both exist.
