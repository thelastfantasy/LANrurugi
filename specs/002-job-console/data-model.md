# Data Model: Background Job Console

No new persisted (Redis) entity — this feature is a read/manage surface over the existing
in-process job registry (`lanrurugi_core::jobs`, introduced in Phase 1). Everything below is
in-memory, scoped to a single server process's uptime (see spec Assumptions).

## Job Record (existing type, reused as-is)

Defined in `crates/lanrurugi-core/src/jobs.rs::JobStatus` — no field changes required:

| Field | Type | Notes |
|---|---|---|
| `id` | `String` (UUID) | Unique per job, assigned at creation |
| `name` | `String` | Job type, e.g. `"regen_thumbnails"`, `"backup"`, `"restore"`, `"rebuild_index"`, `"find_duplicates"`, `"plugin_exec"`, `"download_url"` — the exact literal strings each real call site passes to `create()`, verified against `crates/lanrurugi-api/src/{archives,database,duplicates,plugins,bench}.rs` (note: `clean_database` is *not* one of these — like archive rescans, it's a synchronous endpoint that never calls `jobs.create()`, so it will never appear here) |
| `state` | `JobState` (`Queued`/`Active`/`Finished`/`Failed`) | Terminal states: `Finished`, `Failed` |
| `progress` | `f32`, 0.0–1.0 | Best-effort; jobs without granular reporting jump 0→1 |
| `result` | `Option<serde_json::Value>` | Populated on `Finished` |
| `error` | `Option<String>` | Populated on `Failed` |

## JobRegistry (extended)

`crates/lanrurugi-core/src/jobs.rs::JobRegistry` gains:

- An internal creation-order index (`Vec<String>` of job IDs, oldest first) alongside the
  existing `HashMap<String, JobStatus>`, maintained by `create()`.
- `MAX_TRACKED_JOBS: usize = 500` — enforced by `create()`: once the registry holds this many
  entries, evict the oldest job(s) currently in a terminal state (`Finished`/`Failed`) before
  inserting the new one. See research.md §2 for the eviction-order rationale.
- `pub async fn list_all(&self) -> Vec<JobStatus>` — every tracked job, most-recently-created
  first (using the order index, not `HashMap` iteration order).
- `pub async fn clear(&self, id: &str) -> ClearOutcome` — removes one job by ID; returns
  `ClearOutcome::Cleared` on success, `ClearOutcome::NotFound` if no job with this ID is tracked,
  or `ClearOutcome::NotTerminal` if the job is still `Queued`/`Active` (FR-004 — in-flight jobs
  cannot be cleared). The three-valued outcome (rather than a plain `bool`) is what lets
  `DELETE /jobs/{id}` distinguish its 404 ("unknown ID") from its 409 ("still running") response
  from a single atomic operation.
- `pub async fn clear_finished(&self) -> usize` — removes every job currently in a terminal state
  (`Finished` or `Failed`); returns the count removed. Active/queued jobs are untouched.

No changes to `create`, `mark_active`, `set_progress`, `finish`, `fail`, `get`, or `by_name` —
existing callers (backup/restore, thumbnail regen, duplicate scans, index rebuilds, plugin
execution, URL-download ingestion) are unaffected.

## API Response Shapes (native, not legacy-mimicking — see research.md §1)

`GET /api/jobs` → `{ "jobs": JobStatus[] }`, each `JobStatus` serialized with its real field names
(`id`, `name`, `state`, `progress`, `result`, `error`) — a straight `#[derive(Serialize)]` of the
existing Rust type, no shape translation.

`DELETE /api/jobs/{id}` → `{ "operation": "clear_job", "success": 1 }` on success, `404` if the
job doesn't exist, `409` if the job is still queued/active (cannot clear a non-terminal job).

`DELETE /api/jobs?state=finished` → `{ "operation": "clear_finished_jobs", "success": 1, "cleared": <count> }`.
`state=finished` means "any terminal-state job" (both `Finished` and `Failed`), matching FR-004's
combined "finished and/or failed" bulk-clear action as a single operation — there is no separate
`failed`-only filter in this first version; one could be added later as an additive query-param
value without a breaking change if a real need for it shows up.
