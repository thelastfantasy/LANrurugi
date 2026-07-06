# Contract: REST API (Phase 1)

Per constitution Principle II, all endpoints listed under "Existing contract (must not change)"
were verified present in the legacy `tools/openapi.yaml` (path list obtained by reading that file
directly, not assumed). LANrurugi's `lanrurugi-api` crate MUST reproduce the same paths, request
parameters, response shapes, and API-key auth semantics for every path below. The full field-level
schema for each path is the legacy `tools/openapi.yaml` itself — that file is the authoritative
contract; this document records which parts of it are in scope for Phase 1 and what is being
added, not a re-derivation of every field.

## Existing contract (must not change) — grouped by legacy `tags`

- **search**: `/search`, `/search/ids`, `/search/random`, `/search/cache` (US3)
- **archives**: `/archives`, `/archives/untagged`, `/archives/upload`, `/archives/{id}`,
  `/archives/{id}/metadata`, `/archives/{id}/thumbnail`, `/archives/{id}/categories`,
  `/archives/{id}/tankoubons`, `/archives/{id}/toc`, `/archives/{id}/download`,
  `/archives/{id}/files`, `/archives/{id}/files/thumbnails`, `/archives/{id}/page`,
  `/archives/{id}/progress/{page}`, `/archives/{id}/isnew`, `/archives/{id}/stamps`,
  `/archives/{id}/stamps/{index}` (US1, US2, US3)
- **categories**: `/categories`, `/categories/bookmark_link`,
  `/categories/bookmark_link/{id}`, `/categories/{id}`, `/categories/{id}/{archive}` (US1, US3)
- **tankoubons**: `/tankoubons`, `/tankoubons/{id}`, `/tankoubons/{id}/full`,
  `/tankoubons/{id}/thumbnail`, `/tankoubons/{id}/progress/{page}`,
  `/tankoubons/{id}/{archive}` (US1, US3)
- **plugins**: `/plugins/{type}`, `/plugins/use`, `/plugins/queue` (US4)
- **shinobu**: `/shinobu`, `/shinobu/stop`, `/shinobu/restart`, `/shinobu/rescan` (US2's
  scanner, exposed as-is for operational parity)
- **minion**: `/minion/{jobid}`, `/minion/{jobid}/detail`, `/minion/{jobname}/queue` (job-status
  polling; LANrurugi's in-process task queue MUST answer these paths equivalently even though
  there is no separate Minion process per Principle III)
- **database**: `/database/stats`, `/database/backup`, `/database/backup/{jobid}`,
  `/database/restore`, `/database/isnew`, `/database/drop`, `/database/clean` (US5's
  backup/restore, US1's continuity)
- **opds**: `/opds`, `/opds/{id}`, `/opds/{id}/pse` (US3)
- **stamps**: `/stamps/{id}` (data model's Stamp entity)
- **misc**: `/info`, `/tempfolder`, `/download_url`, `/regen_thumbs`

Any request/response shape change to a path above is a contract break and is disallowed for
Phase 1 (FR-013/FR-014); if one is later found to be genuinely necessary, it MUST land under a new
versioned surface (e.g. `/api/v2/...`) per constitution Principle II, not by editing the path
above in place.

## New, additive endpoints (Phase 1)

These are new capabilities (FR-011 duplicate-repair, FR-020 benchmark) with no legacy equivalent,
added under `/api/` alongside the existing paths — they do not alter any path listed above.

- `POST /database/rebuild-index` — triggers the US6 duplicate-repair/reindex operation
  (data-model.md's "Rebuild/Reindex operation"). Returns a job identifier pollable the same way
  existing Minion-style jobs are (`/minion/{jobid}`-shaped), for UI/tooling consistency.
- `POST /bench/run` — triggers the US8 benchmark suite (`bench/compare`) against a synthetic
  library; returns a report identifier. `GET /bench/{reportid}` — retrieves the comparison report
  (see `contracts/benchmark-report.md`).

## Auth semantics (must not change)

API-key header-based auth, matching the legacy scheme, applies identically to all paths above
(FR-013). No new auth mechanism is introduced for Phase 1; Phase 2's per-provider secrets
(constitution Principle V) are out of scope here.
