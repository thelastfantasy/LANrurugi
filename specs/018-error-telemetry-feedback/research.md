# Research: Error Telemetry and Feedback Bundles

**Date**: 2026-10-28 | **Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md)

All decisions below are the ones the plan depends on. Anything marked *verify at implementation* is a
version or API detail that the constitution requires re-checking live rather than trusting this file.

## 1. How the client's events reach the local store

**Decision**: the instance is its own Sentry-compatible ingest. The browser SDK is initialised with a
DSN pointing at *this* instance (`http(s)://<session-scoped key>@<instance host>/api/diagnostics/`),
so its standard transport posts the standard envelope to `/api/diagnostics/envelope` on the same
origin, carrying the session cookie. The server accepts the envelope, stores it, and treats it as the
local record. A custom browser transport is **not** needed.

**Rationale**: the DSN's host is ours, so `@sentry/react`'s default fetch transport is busy doing
exactly the right thing (batching, retry, `sendBeacon` on unload) with zero custom code. Same-origin
also means no CORS, no key leakage beyond the instance, and the "offline" case is just "same origin is
up". It keeps FR-017 (bundle events replayable to another tracker) free: what we store *is* the
envelope.

**Alternatives considered**:
- *Custom client transport writing to `localStorage`/IndexedDB and uploading later.* Rejected: the
  store then exists twice (browser + server), the bundle builder needs both, and a cleared browser
  loses everything.
- *Client posts our own JSON shape instead of an envelope.* Rejected: loses SDK-side features
  (attachments, replay from the envelope) and makes FR-017 a conversion problem.
- *No SDK, hand-rolled capture.* Rejected in the design discussion: worse stacks, no source context,
  and we would reimplement grouping.

## 2. Server-side capture and the SDK integrations

**Decision**: `sentry` with `sentry-tower` (axum layer for request context and 5xx capture),
`sentry-panic` (Rust panics), `sentry-tracing` (error-level `tracing` events only; warnings become
breadcrumbs). The server's transport is an in-process one that writes into the same store; the HTTP
transport is only used by the forwarding sink.

**Rationale**: the integrations are the entire reason to use the SDK on the server; they capture
panic payloads and request context we would otherwise lose. Error-level only keeps the store honest
and avoids duplicating the existing log files.

**Alternatives considered**: feeding the store from `tracing` alone — loses panic backtraces and
request context; using `sentry-log` — the project does not use the `log` crate.

**Versions to verify live**: `sentry` 0.49.x (latest published 2026-09-21 when this was written),
`sentry-tower`/`sentry-tracing`/`sentry-panic` at the same version, `@sentry/react` 11.x (latest
11.9.0 when this was written). Any newer stable release wins per the constitution.

## 3. Envelope ingest contract

**Decision**: accept `POST /api/diagnostics/envelope` with `Content-Type: application/x-sentry-envelope`,
optionally `Content-Encoding: gzip`, and ignore the `X-Sentry-Auth` header (the DSN key is not a
secret here; the session cookie is the actual authentication). Decode the envelope's header and event
item, keep the raw bytes for replay, and derive: event id, timestamp, level, message, exception, stack,
breadcrumbs, tags, release, request id.

**Rationale**: the envelope is line-delimited JSON with a header line, item headers, and payloads;
storing it verbatim is what makes later replay to a tracker exact. Parsing only what grouping and the
claim need keeps the ingest path cheap.

**Risks**: the browser SDK may gzip or send `?sentry_key=…` query parameters; both are accepted. It
may also send a `sentry_client` item — unknown items are preserved, not rejected.

## 4. Grouping and fingerprints

**Decision**: use the event's own `fingerprint` when present (we set
`["{{ default }}", "release:<sha>"]` on the client to mirror Sentry's default grouping); otherwise
compute `sha256(type | top user frame (file:line, function) | normalized message)`. Message
normalization replaces hex ids, UUIDs, numbers longer than 3 digits, quoted strings and paths with
placeholders.

**Rationale**: matching Sentry's semantics means the same event groups the same way locally and on
the tracker, so a bundle and a forwarded issue agree.

**Alternatives considered**: exact-message grouping (too granular), stack-only (collides across
different bugs on one line).

## 5. Local store, retention, quota

**Decision**: Redis, additive keys. `LANRURUGI_DIAGNOSTIC_EVENT:<id>` is a hash (metadata + gzipped
envelope), `LANRURUGI_DIAGNOSTICS_EVENTS` is a zset scored by timestamp, `LANRURUGI_DIAGNOSTIC_ISSUE:<fp>`
is a hash with counters and a small representative-event cache, `LANRURUGI_DIAGNOSTICS_ISSUES` indexes
issues. Settings live on the existing `LRR_CONFIG` hash: retention days, max events, max bytes,
capture switches, endpoint, window deadline. A `LANRURUGI_DIAGNOSTICS_BYTES` counter tracks stored
bytes (incremented/decremented on write/evict, recomputed lazily if missing).

Eviction runs after each write, cheapest-first: drop by age (`ZREMRANGEBYSCORE`), then by count
(`ZREMRANGEBYRANK`), then by bytes until the quota fits. Issue rows are kept even when their events
were evicted, so the list still shows history with counts.

**Rationale**: three limits because each answers a different question ("how long", "how many", "how
much disk"); a bytes counter is the only way to enforce a quota without scanning every record.

**Alternatives considered**: files on disk (harder to rate-limit and to query by level/time), a single
list with `LTRIM` (no per-event metadata).

## 6. Scrubbing

**Decision**: two layers. (a) Structural: breadcrumbs are built from a fixed field set (category,
short description, no form values, no file names); the client never sends archive titles unless the
administrator opted in. (b) Textual: a regex pass over message, stack, breadcrumb text, tags and
request data removes `lrrg:`-prefixed codes, `?r=` sealed suffixes, `X-Share-Token`, `rt=`,
`Authorization`, `Cookie`, `api_key`/`token`/`secret` assignments, absolute paths (POSIX and Windows),
and long hex/base64-looking strings. Applied on ingest, before storage — so the store never holds a
secret, and the bundle cannot leak one the store never had.

**Rationale**: "scrub on export" is too late: the local store is readable in the settings page, and a
second export path could forget. Structural scrubbing first avoids regex chasing.

**Alternatives considered**: relying on the SDK's `send_default_pii=false` (necessary but not
sufficient — messages still contain codes).

## 7. Symbolication by release

**Decision**: the frontend build emits hidden maps (`build.sourcemap: 'hidden'`), the image build keeps
them **outside the served static tree** at `/usr/local/share/lanrurugi/sourcemaps/<release>/<chunk>.js.map.gz`,
and the release is the git sha (already passed to the Docker build as `LANRURUGI_GIT_SHA`; the
frontend build gains `__GIT_SHA__`). When a bundle is generated, frames are resolved on the server with
the `sourcemap` crate (MIT), loading only the maps the selected events reference, and `sourcesContent`
is dropped from any copy that would leave the process. If the map for that release is not present
(dev build, native package without maps), frames stay raw and the manifest carries the release and
chunk identity.

**Rationale**: shipping ~4 MB gzip of maps inside the image is cheaper than every instance fetching
them, keeps the intranet case working, and never publishes source maps. Resolving on the server means
the bundle text is readable even when the reporting browser is gone.

**Alternatives considered**: fetch maps per release from a public host (needs network and a host),
include maps in the bundle (large, leaks sources), client-side symbolication (needs the maps in the
browser anyway).

**Map size measured on this repository**: 45 chunks, 17.5 MB raw / 4.07 MB gzip total; the `main`
chunk alone is 4.4 MB raw / 907 KB gzip, or 613 KB / 149 KB gzip without `sourcesContent`.

## 8. Forwarding window

**Decision**: the window is server state (`LRR_CONFIG`: endpoint, key, window deadline, last error,
sent/failed counters), not a browser flag. The settings page reads the deadline and shows a countdown;
opening a window writes `deadline = now + duration`; closing writes 0. The forwarder checks the
deadline before every send, so expiry is enforced where data leaves. A browser reload reads the stored
deadline, and a server restart does not extend it (an expired deadline simply means "closed").

**Rationale**: a client-only timer is trivially defeated by a reload or a second tab and would let a
window outlive its stated duration.

**Alternatives considered**: storing the window in the browser (rejected above); a signed token in the
DSN (overkill).

## 9. Bundle generation and feedback text

**Decision**: a single blocking job (the `tankoubon_zip` pattern) streams a zip to the HTTP response:
`manifest.json`, `server/<category>.log` slices (continuation lines attached to their timestamped
entry), `frontend/events.jsonl`, `frontend/env.json`, `diagnostics.json`, `config.redacted.json`,
`README.txt`. Selection is resolved to explicit `{file, from, to, include, exclude}` ranges at request
time and frozen into the manifest. The feedback text is generated from the same selection, in both
Chinese and English, from templates in `text.rs` (no i18n framework on the server).

**Rationale**: streaming keeps memory flat and lets a 20 MB bundle start immediately; freezing the
selection makes the manifest an audit of what was actually included.

**Alternatives considered**: build the bundle client-side (the browser has no log files), generate it
into a file and serve later (garbage collection problem).

## 10. `/logs` demotion

**Decision**: keep `GET /logs/{category}` and the page reachable, remove it from the main navigation,
and link it from the diagnostics settings section as "raw log output". The diagnostics list is the
primary path.

**Rationale**: the raw view is still useful for deep debugging and costs nothing; keeping the route
preserves any existing bookmark and the documented legacy shape.

## Open items for implementation time

- Exact `sentry`/`@sentry/react` versions, and whether `sentry-tower` 0.49 supports the axum version in
  this workspace without a shim.
- Whether the browser SDK's default transport survives a 4xx from our ingest during logout (it must
  not retry forever); the ingest returns 200 for anything it accepts and 204 otherwise.
- The `sourcemap` crate's API for range lookups at the version pinned then.
