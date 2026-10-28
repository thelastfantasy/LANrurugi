# Contract: Diagnostics API

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Data model**: [data-model.md](./data-model.md)

All routes are mounted under `/api` (the existing router assembly in
`crates/lanrurugi-server/src/app.rs`), so they go through the same cookie/API-key authentication and the
same Casbin `route_policy.csv` as everything else. Every route below MUST be added to that policy for
the `admin` subject only; guests and restricted visitors MUST be refused by the policy, not by a check
inside the handler.

Error responses keep the project shape:

```json
{ "operation": "diagnostics_issue", "error": "No such issue.", "success": 0 }
```

## 1. Envelope ingest (the SDK's endpoint)

### `POST /api/diagnostics/envelope`

The browser SDK's DSN points at this instance, so its default transport posts here. Authentication is
the session cookie (same origin); the `X-Sentry-Auth` header and the `?sentry_key=` parameter are
accepted and ignored.

- Request: `Content-Type: application/x-sentry-envelope`, optionally `Content-Encoding: gzip`. The body
  is a standard envelope (newline-delimited JSON header, then item headers and payloads).
- `200 {"id": "<event_id>"}` when accepted and stored (possibly after background batching).
- `204` when the feature is disabled, the source is switched off, the event was rate limited, or the
  body was not a parseable envelope. A 2xx keeps the SDK from retrying; the reason is counted, not
  returned.
- `401` without a session (the SDK's own retry policy then backs off; this is the logout case).
- `413` only when the body exceeds the hard envelope limit (1 MB) — the SDK treats it as permanent.

Note: the endpoint MUST NOT require an API key (the SDK cannot send one) but MUST require the session,
which is the only reason it is safe to ignore the DSN key.

## 2. Configuration the client needs

### `GET /api/diagnostics/config`

```json
{
  "enabled": true,
  "dsn": "http://<public-key>@<this-origin>/api/diagnostics/",
  "release": "a94a8fe5",
  "environment": "production",
  "sample_rate": 1.0,
  "max_breadcrumbs": 50,
  "window": { "open": false, "deadline": 0, "remaining_secs": 0 }
}
```

`enabled` false means the client does not initialise the SDK at all. The tracker's own DSN is
never sent to the browser — `dsn` here is always this instance's own ingest address.

## 3. Issues

### `GET /api/diagnostics/issues`

Query: `level` (repeatable), `source` (repeatable), `from`, `to` (unix ms), `release`, `state`
(`open|resolved|ignored|all`, default `open`), `q` (free text), `sort` (`recent|frequent`, default
`recent`), `limit` (default 50, max 200), `cursor`.

```json
{
  "issues": [
    { "fingerprint": "…", "title": "…", "level": "error", "count": 12,
      "first_seen": 1761600000000, "last_seen": 1761603600000,
      "releases": ["a94a8fe5"], "sources": ["web"], "state": "open",
      "request_id": "7fa3", "sample_event_id": "…" }
  ],
  "total": 34,
  "next_cursor": null
}
```

### `GET /api/diagnostics/issues/{fingerprint}`

```json
{
  "issue": { "…": "as above" },
  "events": [ { "…": "DiagnosticEvent projection, newest first, max 20" } ],
  "log_excerpt": { "general": ["…up to 200 lines around the last event…"] }
}
```

`log_excerpt` is only included when `request_id` is known; it is the same matching the bundle uses.

### `POST /api/diagnostics/issues/{fingerprint}/state`

Body `{"state": "open" | "resolved" | "ignored"}`. Idempotent. `200 {"state": "resolved"}`.

## 4. Events and the merged timeline

### `GET /api/diagnostics/events`

The list the administrator multi-selects from. Same filters as issues plus `fingerprint`;
`level` defaults to all. Returns `{ "entries": [ … ], "total": n, "next_cursor": … }`, where an entry is
either a captured event projection or a server log line slice:

```json
{ "kind": "event", "id": "…", "at": 1761600000000, "level": "error",
  "source": "web", "message": "…", "fingerprint": "…", "request_id": "7fa3" }
{ "kind": "log", "category": "general", "at": 1761600000000, "level": "warn",
  "text": "…", "line": 1234 }
```

Log entries are parsed from the timestamped log files (`RFC3339` prefix); continuation lines without a
timestamp are attached to the entry above, so a stack trace is never split.

## 5. Bundles

### `POST /api/diagnostics/bundles/preview`

Body: the `SelectionSpec` (see data-model). Returns the file list and sizes, and any truncation that
would be needed:

```json
{
  "files": [ { "path": "server/general.log", "bytes": 81234, "entries": 412 },
             { "path": "frontend/events.jsonl", "bytes": 4021, "entries": 7 },
             { "path": "diagnostics.json", "bytes": 2210 },
             { "path": "manifest.json", "bytes": 900 },
             { "path": "README.txt", "bytes": 1400 } ],
  "total_bytes": 89765,
  "over_cap": false,
  "cap_bytes": 20971520,
  "truncations": []
}
```

### `POST /api/diagnostics/bundles`

Same body. Streams `application/zip` with
`Content-Disposition: attachment; filename="lrrg-diagnostics-<yyyyMMdd-HHmm>.zip"`. The zip contains
exactly the previewed files. `413` when `over_cap` and the client did not accept a truncation
(`"truncate": true` in the body asks for the fitting subset, reported in the manifest).

### `POST /api/diagnostics/bundles/text`

Same body. Returns both languages so the client can show, edit and copy either:

```json
{
  "zh": { "title": "…", "body": "…" },
  "en": { "title": "…", "body": "…" },
  "bundle_name": "lrrg-diagnostics-20261028-1412.zip"
}
```

The body names the version, release, engine version, platform, range and counts, the top fingerprint,
the request id when present, the included categories, and any truncation.

## 6. Settings and the forwarding window

### `GET /api/diagnostics/settings`

Everything from `DiagnosticsSettings`, plus usage and drops:

```json
{
  "settings": { "enabled": true, "retention_days": 7, "max_events": 50000,
                "max_bytes": 536870912, "sources": { "web": true, "server": true, "plugins": true },
                "breadcrumbs": true, "include_archive_names": false, "tracker_configured": false },
  "usage": { "events": 1203, "issues": 41, "bytes": 8123456 },
  "drops": { "retention": 10, "rate_limit": 0, "too_large": 0, "store_error": 0 }
}
```

`tracker_configured` is a boolean; the DSN (which contains a credential) is never returned. Updating it
is a separate write:

### `PUT /api/diagnostics/settings`

Body: any of the `DiagnosticsSettings` fields except the window. `tracker_dsn: ""` clears the endpoint.
`200 {"saved": true}`.

### `GET /api/diagnostics/window`

```json
{ "open": true, "deadline": 1761603900000, "remaining_secs": 298,
  "sent": 12, "failed": 1, "last_error": null }
```

### `POST /api/diagnostics/window`

Body `{"duration_secs": 300}` (allowed 300, 3600, 86400). `409` when no tracker is configured.
`200` with the same shape as `GET`.

### `DELETE /api/diagnostics/window`

Closes immediately. `200 {"open": false, "deadline": 0}`.

## 7. Offline replay (US5, P3)

### `POST /api/diagnostics/replay`

Body: a previously generated bundle's `envelopes.ndjson.gz` (or the bundle zip, entry located by name).
Sends each stored envelope to the configured tracker, honouring nothing else — this is an explicit
administrator action, not a window. Returns `{"sent": n, "failed": m, "errors": ["…"]}`. `409` when no
tracker is configured.

## 8. Source-map fetch used by symbolication (internal, not a route)

The server reads maps from its own artifact directory; there is no HTTP route for maps, and the static
handler MUST NOT serve the artifact directory. A missing map degrades a frame to raw, never a 5xx.

## Forwarding mapping (stored envelope → tracker)

| Local | Tracker request |
|---|---|
| The stored envelope bytes | POST to `<dsn-project-url>/envelope/?sentry_key=<key>` with `Content-Type: application/x-sentry-envelope` and `X-Sentry-Auth` reconstructed from the DSN |
| `release` tag | Already inside the envelope's event item; no rewriting |
| `event_id` | Already inside the envelope; the tracker's dedup uses it |
| Scrubbing | Applied before storage, so the forwarded bytes are already scrubbed |

A tracker rejecting an envelope is a forwarding failure: it increments `failed`, records `last_error`,
never deletes the local copy, and never closes the window's own state machine.
