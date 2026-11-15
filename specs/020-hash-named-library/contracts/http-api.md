# Contract: HTTP API — Hash-Named Library

## Removed

| Endpoint | Effect after removal |
|---|---|
| `PUT /api/archives/{id}/rename` | Not registered; the route returns 404. No replacement: the stored name is no longer user-visible state. |

The frontend call site and the docgen sidecar entry are deleted in the same change; `mise run docs-api -- --check`
fails if a route is served but undocumented, which is the gate that proves the removal is complete.

## Added — relocation observability

Relocation has no endpoint of its own: it happens during every scan. The scan result (and the `scan` job in
`GET /api/jobs`) gains counters — `moved_to_byname`, `skipped_occupied`, `failed` — so an administrator can see what
moved without reading the log. The exact response shape follows whatever `POST /api/scan` returns today; `job_console`
already renders per-job progress, and these are additive fields on it. There is no preview, confirm, cancel or rollback
endpoint, because relocation is a rule rather than a job: a file moved by mistake is moved back on disk, and the next
scan updates its record.

## Unchanged but load-bearing

| Endpoint | Guarantee |
|---|---|
| `GET /api/archives/{id}/download` and `…/download-link` | The downloaded filename is the archive's title, never the id (`Content-Disposition`), and the link path is unchanged. |
| `GET /api/archives/{id}/thumbnail`, `…/page/{n}` | Unchanged; they resolve `file` from the record, which now points into a bucket. |
| `PUT /api/archives/{id}/metadata` | Title editing remains; it writes the record and, for hash-layout archives, rewrites the sidecar. |
| Scan/rebuild endpoints | Behaviour unchanged except that sidecars are never catalogued and files outside the hash layout are relocated into `_byname/`. |

## Error shapes

New errors reuse the existing `{ "error": "…", "code": … }` envelope. No new success shape is introduced for an
existing endpoint.
