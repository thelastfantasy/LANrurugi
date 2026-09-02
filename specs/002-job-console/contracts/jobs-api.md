# Contract: Job Console API

Additive endpoints, no legacy contract equivalent (see research.md §1 for why `/api/minion/*` is
deliberately left untouched). All endpoints require the same authentication as the rest of the
admin-facing API (FR-008) — standard API-key/session middleware, no special-cased bypass.

## `GET /api/jobs`

Lists every currently tracked job, most-recently-created first.

**Response** `200`:

```json
{
  "jobs": [
    {
      "id": "b3f1...-uuid",
      "name": "regen_thumbnails",
      "state": "active",
      "progress": 0.42,
      "result": null,
      "error": null
    },
    {
      "id": "a1c2...-uuid",
      "name": "backup",
      "state": "finished",
      "progress": 1.0,
      "result": { "notes": "..." },
      "error": null
    }
  ]
}
```

`state` ∈ `"queued" | "active" | "finished" | "failed"` (matches `JobState`'s serde
`rename_all = "snake_case"`, i.e. `Queued → "queued"`).

## `DELETE /api/jobs/{id}`

Clears one job by ID. Only permitted when the job is in a terminal state.

- **Response** `200` (cleared): `{ "operation": "clear_job", "success": 1 }`
- **Response** `404` (unknown ID): standard error envelope (`common::not_found`)
- **Response** `409` (job still queued/active): standard error envelope (`common::error`),
  message explaining the job must reach a terminal state before it can be cleared

## `DELETE /api/jobs?state=finished`

Clears every job currently in a terminal state (`Finished` or `Failed`). Active/queued jobs are
left untouched and continue to be tracked.

- **Response** `200`: `{ "operation": "clear_finished_jobs", "success": 1, "cleared": <count> }`
- **Response** `400` if `state` is missing or has any value other than `finished` (only supported
  filter value in this version — see data-model.md)

## Per-state counts, filtering, search, pagination (US4)

No API surface at all — `GET /api/jobs` above is the only endpoint the frontend calls, and US4's
counts/state-filter/name-search/pagination are all derived client-side from that one response
array (research.md §5). There is no `?state=`/`?q=`/`?page=` query parameter on `GET /api/jobs`
itself, and none is planned.

## Frontend consumption

`apps/frontend/src/api/hooks.ts` adds:

- `useJobs()` — `useQuery` over `GET /api/jobs`, `refetchInterval` matching the existing
  `useShinobuStatus`/`useLogLines` polling convention (research.md §3).
- `useClearJob(id)` / `useClearFinishedJobs()` — `useMutation`s over the two `DELETE` endpoints
  above, invalidating the `['jobs']` query key on success.

`apps/frontend/src/api/types.ts` adds a `JobRecord` interface matching the native shape above
(`id`/`name`/`state`/`progress`/`result`/`error`) — distinct from the existing `JobStatus`
interface (which matches the legacy-mimicking `/api/minion/{jobid}` shape: `task`/`state`/
`notes`/`error` — left as-is, still used by `waitForJob`/`pollJob` for existing job-triggering
call sites like Backup.tsx).
