# Quickstart: Validating the Background Job Console

Prerequisites: Phase 1 (`001-lanrurugi-full-rewrite`) built and running — this feature only adds
to the existing `lanrurugi-server` binary and `apps/frontend` app, no new deployable.

## 1. See a job appear and progress (US1)

```
lanrurugi serve --redis-url <redis> --library-path <library>
# In the UI: Settings → Tags and Thumbnails → "Regenerate all Thumbnails"
```

**Expected**: opening `/jobs` (or Settings → Background Workers → "Open Minion Console") shows the
just-triggered job with state `active` and increasing `progress`, updating without a manual page
reload (SC-001). Once it completes, its state flips to `finished` and it stops being counted as
active.

## 2. Empty state (US1, edge case)

```
lanrurugi serve --redis-url <fresh redis> --library-path <library>
```

**Expected**: opening `/jobs` immediately after a fresh server start (before triggering anything)
shows an explicit empty state, not an error or blank page (FR-007).

## 3. Inspect a failed job (US2)

Trigger a restore with a malformed/corrupt JSON file via Settings → Database Backup/Restore.

**Expected**: the job appears in `/jobs` as `failed`; opening its detail shows the specific error
captured at failure time (SC-004), not a generic "job failed" message. Confirm via
`GET /api/jobs` directly too (`contracts/jobs-api.md`) — the `error` field is populated.

## 4. Inspect a finished job's result (US2)

Trigger "Scan for duplicates" on the Duplicates page (`POST /database/duplicates/scan`, a
job-tracked operation — unlike "Clean Database" in Settings, which is a synchronous endpoint that
never creates a tracked job at all, see spec.md FR-005's note and data-model.md).

**Expected**: the finished job's detail view shows its result summary (e.g. how many duplicate
groups were found), matching what the scan already reports elsewhere in the UI.

## 5. Clear finished jobs without disturbing active ones (US3)

With at least one finished job and one still-active job (e.g. trigger a database backup or
restore — `POST /database/backup`/`POST /database/restore`, both job-tracked — and immediately
open `/jobs` while it's running, alongside an already-finished job from a prior step; a plain
archive rescan does **not** work for this since `POST /shinobu/rescan` is synchronous and never
creates a tracked job — see spec.md FR-005):

```
curl -X DELETE 'http://localhost:3000/api/jobs?state=finished' -H '<auth>'
```

**Expected**: the finished job disappears from a subsequent `GET /api/jobs`; the still-active job
remains listed with its current state untouched. Note: the bulk clear button always targets every
finished/failed job server-side, regardless of any US4 state/name filter currently narrowing the
displayed list (FR-004) — if you have a search/filter applied while testing this, confirm a
finished job *not* matching the current filter also gets cleared, not just the visible ones.

## 6. Bounded retention (FR-006)

Not practical to validate by hand end-to-end (would require triggering 500+ jobs); instead,
confirm at the unit-test level (`crates/lanrurugi-core/src/jobs.rs` tests) that `create()` evicts
the oldest terminal-state job once `MAX_TRACKED_JOBS` is exceeded, and does not evict/drop a
still-active job to make room.

## 7. Find a job in a long list (US4)

With a handful of jobs of different names and states already listed (e.g. a mix from steps 1-5
above):

**Expected**: the console shows a count per state (queued/active/finished/failed) that matches
the actual list (FR-009); selecting a state filters the list down to only that state (FR-010);
typing part of a job name (e.g. `"backup"`) narrows the list to matching jobs only (FR-011); with
enough jobs to exceed one page, the list is paginated rather than one long scroll (FR-012, SC-005)
— all of this without any extra network request beyond the same `GET /api/jobs` poll from step 1.
