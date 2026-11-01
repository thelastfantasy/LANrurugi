# HTTP API Contract: Recycle Bin and Safety Password

All endpoints are new (Principle II: existing paths and fields are unchanged; `DELETE /api/archives/{id}` and
`DELETE /api/archives` gain one additive field). They are registered in `crates/lanrurugi-api/src/recycle.rs` and
`safety_password.rs`, documented through the 011 docgen sidecar (`docs/api/sidecar/recycle-bin.yaml`), and gated
by `route_policy.csv`. Errors use the existing `{ "operation", "error", "success": 0 }` shape.

## Authorization

| Role | `/api/recycle-bin*` | `/api/safety-password*` |
|---|---|---|
| `session` | allow | allow |
| `token_admin` | **deny** | **deny** |
| `token_guest` | **deny** | **deny** |
| `guest_visitor` | deny (not in the explicit allow list) | deny |

Deny lines are explicit in `route_policy.csv`, matching the `/api/sessions*` and `/api/settings/password`
precedent.

## Bin endpoints

| Method & path | Body / query | Response |
|---|---|---|
| `GET /api/recycle-bin` | `?q=<text>` (optional filter over title/name/path) | `RecycleBinView` |
| `GET /api/recycle-bin/{id}/thumbnail` | | image bytes (`image/webp`, `image/jpeg` or `image/png`); `404` if none |
| `POST /api/recycle-bin/{id}/restore` | — | `RestoreResult` |
| `DELETE /api/recycle-bin/{id}` | `{ "safety_password": "1234" }` | `PurgeResult` |
| `DELETE /api/recycle-bin` | `{ "safety_password": "1234" }` (empty the bin) | `EmptyResult` |

`DELETE` with a JSON body is already this codebase's established shape (`DELETE /api/archives`,
`DELETE /api/activity`), and the frontend uses `fetch` directly, so no client constraint applies.

### Types

```text
RecycleBinView {
  entries: [RecycleEntryView],        // newest first
  usage:   { count, bytes },
  limits:  { retention_days, max_bytes },   // 0 means the corresponding bound is off
  leftovers: { count, bytes },        // orphaned payloads past the grace period, if any
  safety_password: { configured: bool }
}

RecycleEntryView {
  id, title, name, original_path, file_name, size, page_count,
  deleted_at, deleted_by: { kind, label },
  expires_at: number|null, remaining_seconds: number|null,
  restorable: bool,
  conflict: "identical_archive_exists"|"payload_missing"|"payload_size_mismatch"|"unsupported_schema"|null,
  has_thumbnail: bool
}

RestoreResult  { operation: "restore_archive", success: 1, id, path, renamed: bool }
PurgeResult    { operation: "purge_archive",   success: 1, id }
EmptyResult    { operation: "empty_recycle_bin", success: 1, deleted, failed: [ { id, error } ] }
```

### Check order (FR-022)

Every destructive bin endpoint resolves its inputs in this order, and the order is part of the contract:

1. authenticate the session (route policy);
2. verify the safety password, including the lock check — a failure here returns the same generic
   `safety_password_incorrect` / `safety_password_locked` regardless of which id was asked for;
3. resolve the entry (`404` only now);
4. take the per-entry lock (`409 in progress` on a lost race);
5. execute.

A caller without the safety password therefore cannot use the response to learn whether a given id exists.

### Behaviour and errors

- `POST …/restore`
  - `404 not_found` — no such entry.
  - `409 conflict` — `identical_archive_exists` (FR-008) or `payload_missing` / `payload_size_mismatch` /
    `unsupported_schema` (FR-012).
  - `200` with `renamed: true` and the actual `path` when the original path was occupied (FR-007) or the
    original directory vanished (FR-009).
  - Restoring an entry already being restored/emptied returns `409` with `operation in progress` (FR-014).
- `DELETE /api/recycle-bin/{id}`
  - `401`/`403` when no safety password is configured (`safety_password_not_set`) — the body carries that code so
    the UI can open the setup dialog.
  - `429` while locked, body `{ error, success: 0, locked_until }` (FR-023).
  - `403` with `safety_password_incorrect` on a wrong PIN — generic, no entry-state information (FR-022).
  - `404` — no such entry; `200` only after the file, thumbnails and entry are gone.
- `DELETE /api/recycle-bin`
  - same PIN errors as above; a partial failure still returns `200` with `deleted` count and a per-id `failed`
    list, and the surviving entries stay listed (FR-011).
  - `409` if another destructive operation holds the bin mutation lock for a conflicting entry.

### Existing delete endpoints (additive only)

- `DELETE /api/archives/{id}` → unchanged fields plus `"recycled": true`. No filesystem path is added: the
  response is reachable by API tokens, and the bin's internal path is not something a client needs.
- `DELETE /api/archives` (batch) → unchanged fields plus `"recycled": true` per successful result and
  `"recycled_count"` at the top level.
- Both recycle the archive (FR-001). Deletions the system performs on its own — missing-file cleanup, superseded
  revision retirement, split original removal — do not call these endpoints and never create entries (FR-017).

Clients that ignore unknown fields are unaffected; the contract tests keep asserting every existing field.

## Safety-password endpoints

| Method & path | Body | Response |
|---|---|---|
| `GET /api/safety-password` | — | `SafetyPasswordStatus` |
| `POST /api/safety-password` | `{ "current_password"?: "1234", "new_password": "5678" }` | `SafetyPasswordStatus` |
| `POST /api/safety-password/reset` | `{ "login_password": "…" }` | `SafetyPasswordStatus` |

```text
SafetyPasswordStatus {
  configured: bool,
  locked: bool,
  locked_until: number|null,       // unix seconds, when locked
  attempts_remaining: number|null  // 5 - failures, null when not configured
}
```

- `POST /api/safety-password` sets the password when none is configured; otherwise `current_password` is required
  and must verify through the same helper as a sensitive action (FR-018/FR-019). A wrong current password returns
  `403 safety_password_incorrect`, shares the same failure counter and lock, and is refused while locked. A
  successful set/change/reset writes the `safety_password.set` / `safety_password.reset` activity entry (FR-036).
- `POST /api/safety-password/reset` verifies the **login** password against `LRR_CONFIG.password`; on success it
  clears the hash and the lockout (FR-025). A wrong login password returns `403 wrong_password` and is not
  counted as a safety-password attempt.
- Any value that is not exactly four ASCII digits returns `400 invalid_safety_password_format` without touching
  stored state.
- No response ever contains the hash, the submitted digits, or any prefix of either (FR-018).

## CLI

`lanrurugi reset-safety-password --redis-url redis://127.0.0.1:6379` (also honours `LANRURUGI_REDIS_URL`):

- deletes `safety_password_hash` and `safety_password_set_at` from `LRR_CONFIG`;
- deletes every `LANRURUGI_SAFETY_FAIL:*` / `LANRURUGI_SAFETY_LOCK:*` key;
- prints one confirmation line and the new state (`safety password cleared; set a new one in Settings`);
- never accepts a password argument, never prints one, exits non-zero on a Redis error.
