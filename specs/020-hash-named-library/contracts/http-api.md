# Contract: HTTP API — Hash-Named Library

## Removed

| Endpoint | Effect after removal |
|---|---|
| `PUT /api/archives/{id}/rename` | Not registered; the route returns 404. No replacement: the stored name is no longer user-visible state. |

The frontend call site and the docgen sidecar entry are deleted in the same change; `mise run docs-api -- --check`
fails if a route is served but undocumented, which is the gate that proves the removal is complete.

## Added — migration

All endpoints require an administrator session (the same role that may run backups and rebuilds). They are additive
and versionless like the rest of `/api`.

### `GET /api/migration/preview`

Computes a plan from the current library and returns counts and a bounded sample. Does not modify anything.

```json
{
  "library_root": "/home/koyomi/lanrarugi/content",
  "classic_root": "/home/koyomi/lanrarugi/content/.classic",
  "total_archives": 100000,
  "move": 98750,
  "merge": 12,
  "skip": { "already_hashed": 1200, "not_catalogued": 30, "read_only": 2, "unreadable": 6 },
  "sample": [ { "id": "…", "from": "series/vol1.zip", "to": ".classic/5f/5fff….zip" } ]
}
```

Errors: `409` when a migration job is already running, `500` when the library root is unreadable.

### `POST /api/migration`

Starts the job. Body: `{ "confirm": true, "dry_run": false }`. `dry_run: true` behaves like preview plus a per-entry
log and is the way to exercise the job on a large library.

- `202` with `{ "job_id": "…", "state": "running" }`.
- `409` when another migration is running.
- `400` when `confirm` is absent/false.

The job appears in the existing job console (`GET /api/jobs`) with `kind = "hash_migration"` and a progress counter
`moved / total`.

### `GET /api/migration`

Returns the current or last run:

```json
{ "job_id": "…", "state": "running", "moved": 4210, "total": 98750, "merge": 12, "skip": 38,
  "started_at": "…", "finished_at": null, "journal": "…/.classic/.migration/<job>.jsonl" }
```

### `POST /api/migration/rollback`

Reverses the journal of the last completed run in reverse order.

- `202` with the new job id.
- `409` when a migration is running.
- `404` when no journal exists for a completed run.

## Unchanged but load-bearing

| Endpoint | Guarantee |
|---|---|
| `GET /api/archives/{id}/download` and `…/download-link` | The downloaded filename is the archive's title, never the id (`Content-Disposition`), and the link path is unchanged. |
| `GET /api/archives/{id}/thumbnail`, `…/page/{n}` | Unchanged; they resolve `file` from the record, which now points into a bucket. |
| `PUT /api/archives/{id}/metadata` | Title editing remains; it writes the record and rewrites the sidecar. |
| Scan/rebuild endpoints | Behaviour unchanged except that sidecars and journals are never catalogued. |

## Error shapes

New errors reuse the existing `{ "error": "…", "code": … }` envelope. No new success shape is introduced for an
existing endpoint.
