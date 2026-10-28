# Data Model: Error Telemetry and Feedback Bundles

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Research**: [research.md](./research.md)

All identifiers below that name a stored record are newtypes in code (constitution Technology Stack);
the plain names here are for readability. Every key is additive: nothing that exists today changes
shape, so no migration tooling ships with this feature.

## Entities

### DiagnosticEvent

One captured occurrence. Stored verbatim enough to replay to a tracker, plus the projection the UI
needs.

| Field | Type | Notes |
|---|---|---|
| `id` | `DiagnosticEventId` | stable id; taken from the envelope's `event_id` when present, else generated |
| `fingerprint` | `String` | grouping key, see below |
| `timestamp` | `i64` (unix ms, server clock) | the ordering key |
| `client_timestamp` | `Option<i64>` | the browser's own clock, when it differs |
| `level` | `error \| warning \| info` | from the event, defaulting to `error` |
| `source` | `web \| server \| plugin` | where it came from |
| `release` | `Option<String>` | git sha of the build that produced it |
| `platform` | `String` | `rust` / `javascript` / plugin namespace |
| `message` | `String` | scrubbed, truncated to the event cap |
| `stack` | `Option<String>` | scrubbed raw stack, or resolved frames when symbolication succeeded |
| `breadcrumbs` | `Vec<Breadcrumb>` | capped (default 50) |
| `tags` | `BTreeMap<String, String>` | route, engine version, theme, locale, user agent class |
| `request_id` | `Option<String>` | the `req=` correlation id |
| `envelope` | `Vec<u8>` (gzip) | the original envelope, for replay (FR-017) |
| `bytes` | `u64` | stored size, used by the quota counter |

Validation: `message` ≤ 8 KB after scrubbing; whole event ≤ 64 KB by default; `breadcrumbs` trimmed
oldest-first; unknown envelope items are kept in `envelope` but not projected.

### DiagnosticIssue

A deterministic group of events.

| Field | Type | Notes |
|---|---|---|
| `fingerprint` | `String` | primary key |
| `title` | `String` | representative message, first line |
| `level` | `Level` | highest level seen |
| `count` | `u64` | monotonically increasing |
| `first_seen` / `last_seen` | `i64` (unix ms) | |
| `releases` | `BTreeSet<String>` | capped at 20 |
| `sources` | `BTreeSet<Source>` | |
| `state` | `Open \| Resolved \| Ignored` | administrator action |
| `state_at` | `Option<i64>` | |
| `sample_event_id` | `Option<DiagnosticEventId>` | for the detail view |

Issue rows outlive their events: eviction removes events, not the issue, so counts and history remain.

### Breadcrumb

`{ at: i64, category: navigation | interaction | request | state | log, message: String (≤ 200 chars), level }`.
Built from a fixed field set; never carries form values, file names or archive titles unless the
administrator opted in.

### SelectionSpec

Frozen at bundle-generation time and recorded in the manifest.

```json
{
  "from": 1761600000000, "to": 1761603600000,
  "levels": ["error", "warning"],
  "sources": ["web", "server"],
  "issues": ["<fingerprint>"],
  "include_ids": ["<event id>"],
  "exclude_ids": ["<event id>"],
  "logs": { "general": [{ "from": 1761600000000, "to": 1761603600000 }] },
  "categories": { "server_logs": true, "frontend_events": true, "environment": true, "config": false },
  "resolve_stacks": true
}
```

### BundleManifest

Written into the zip as `manifest.json`: the `SelectionSpec`, the counters actually included (events,
log lines, truncations), the release(s), the scrubbing policy id, the generator version and the
generation timestamp.

### DiagnosticsSettings

Fields added to the existing settings hash (`LRR_CONFIG`), all administrator-only:

| Field | Default | Notes |
|---|---|---|
| `diagnostics_enabled` | on | master switch |
| `diagnostics_web` / `_server` / `_plugins` | on | per-source capture |
| `diagnostics_breadcrumbs` | on | off drops breadcrumbs only |
| `diagnostics_request_bodies` | on | error response bodies, scrubbed |
| `diagnostics_include_archive_names` | off | opt-in; the only path that ever stores titles |
| `diagnostics_retention_days` | 7 | |
| `diagnostics_max_events` | 50000 | |
| `diagnostics_max_bytes` | 536870912 | 512 MB |
| `diagnostics_tracker_dsn` | empty | empty means "never forward" |
| `diagnostics_window_deadline` | 0 | unix ms; 0 means closed |

### ForwardWindow

Not a stored entity: derived from `diagnostics_window_deadline` plus the in-memory send counters.

States: `closed` (deadline 0 or in the past) → `open` (deadline in the future) → `closed` on expiry or
when the administrator stops it. A restart never extends a deadline; an already-expired deadline reads
as `closed`.

### ReleaseMaps

`release → { chunk → map file }`, read from the image's non-served artifact directory. Not stored in
Redis; only referenced by path. Missing maps are not an error — frames stay raw and the manifest
carries the release.

## Redis keys

| Key | Type | Purpose |
|---|---|---|
| `LANRURUGI_DIAGNOSTIC_EVENT:<id>` | hash | event fields + gzipped envelope |
| `LANRURUGI_DIAGNOSTICS_EVENTS` | zset | event ids scored by `timestamp` (age + range queries) |
| `LANRURUGI_DIAGNOSTIC_ISSUE:<fingerprint>` | hash | issue counters and state |
| `LANRURUGI_DIAGNOSTICS_ISSUES` | zset | issues scored by `last_seen` |
| `LANRURUGI_DIAGNOSTIC_EVENT_ISSUE:<id>` | string | id → fingerprint, for eviction bookkeeping |
| `LANRURUGI_DIAGNOSTICS_BYTES` | string counter | stored bytes; lazily recomputed if absent |
| `LANRURUGI_DIAGNOSTICS_DROPS` | hash | `retention`, `rate_limit`, `too_large`, `store_error` |

## Retention and eviction

Order of work after each ingest batch, all inside one pipeline:

1. Trim by age: `ZREMRANGEBYSCORE LANRURUGI_DIAGNOSTICS_EVENTS -inf (now - retention_days)`.
2. Trim by count: while `ZCARD > max_events`, take the oldest ids and delete them.
3. Trim by bytes: while the counter exceeds `max_bytes`, delete the oldest ids.
4. Keep the cheap invariants: delete each removed id's hash and `EVENT_ISSUE` link, decrement the
   counter, and bump the corresponding drop counter only for the *rate-limit* and *too-large* paths —
   retention evictions are the normal steady state and are reported as "evicted", not "dropped".

## State transitions

**Issue**: `Open → Resolved` and `Open → Ignored` (administrator, recorded with a timestamp); `Resolved
/ Ignored → Open` on a new event with the same fingerprint (a regression reopens it).

**Window**: `closed → open (deadline = now + duration)` on start; `open → closed` on expiry (lazy,
checked on read and before every send) or on stop. No transition extends a deadline.

**Bundle**: `previewed → generating → completed | failed`. A failure leaves nothing behind (the zip is
streamed to the response, never persisted).

## Relationships

- one `DiagnosticIssue` — many `DiagnosticEvent` (via `fingerprint`)
- one `DiagnosticEvent` — many `Breadcrumb`
- one `BundleManifest` — one `SelectionSpec` — many selected events and log ranges
- one `ForwardWindow` — at most one configured tracker DSN
