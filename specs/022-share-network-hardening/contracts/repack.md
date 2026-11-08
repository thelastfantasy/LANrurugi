# Contract: Repack Cache

## Key

```
sha256( archive_id || format_version
      || entry_names(sorted)
      || for each member: path, size, mtime_nanos
      || "store" | "deflate" )
```

Member order and entry names are part of the key (they were missing before, so two different zips could collide).

## Publication

1. Build into `<temp>/repack/<key>.zip.tmp`.
2. `fsync` the file, then `rename` to `<temp>/repack/<key>.zip`.
3. Only then does the path become servable; nothing writes the final path directly.

A crash leaves only `.tmp` files, which the temp sweep removes. The final path is never a partial file.

## Requests

| Situation | Response |
|---|---|
| Entry ready, `GET` | `200` with `Content-Length` and `ETag: "<key>"` |
| Entry ready, `Range` | `206`, `Content-Range`, `ETag`, `Accept-Ranges: bytes` |
| Entry ready, `If-Range` mismatched | `200` full body |
| Entry building, plain `GET` | `200` streaming follow (existing behaviour), no `Content-Length` |
| Entry building, `Range` | `503` + `Retry-After: 5` |
| Two requests for the same key | second waits on the first (singleflight), no duplicate build |

## Bounds

- TTL 24 h; total size capped (LRU eviction of ready entries).
- Entries live under `<temp>/repack/` and count against `tempmaxsize`.
- The cache is derived data: deleting it only costs a rebuild.
