# Quickstart: Error Telemetry and Feedback Bundles

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Contract**: [contracts/diagnostics-api.md](./contracts/diagnostics-api.md)

Runnable validation for the whole feature. Every scenario is written to be performed against a live
instance; the ones marked *(offline)* must pass with the machine's outbound network disconnected.

## Prerequisites

- A working dev or release instance (`mise run dev-rebuild && mise run dev-down && mise run dev-up`, or
  the Docker image) with Redis reachable.
- An administrator session. Log in and keep the cookie:

```sh
BASE=http://127.0.0.1:3000
curl -s -c /tmp/lrr.cookies -H 'Content-Type: application/json' \
  -d '{"password":"<password>"}' "$BASE/api/login" -o /dev/null -w '%{http_code}\n'
```

All `curl` calls below use `-b /tmp/lrr.cookies`.

- For scenario 3, a tracker reachable from the instance: a Rustrak container
  (`docker run -p 8080:8080 rustrak/rustrak-server:v0.21.1` with `SESSION_SECRET_KEY` and
  `CREATE_SUPERUSER`), a project, and its DSN. Set `RUSTRAK_TELEMETRY=off` on it so the tracker does not
  phone home.

## Scenario 1 — capture with nothing configured *(offline)*

1. `curl -b /tmp/lrr.cookies "$BASE/api/diagnostics/config"` → `enabled: true`, a `dsn` pointing at
   `$BASE`, no tracker anywhere.
2. Trigger a client error: open the library page with the browser devtools network throttled offline,
   then press "rescan"; and trigger a server error, e.g.
   `curl -b /tmp/lrr.cookies -X POST "$BASE/api/diagnostics/__boom"` (a debug-only route that panics in
   debug builds) — or use any real failure.
3. `curl -b /tmp/lrr.cookies "$BASE/api/diagnostics/issues"` → at least one issue, with `release` equal
   to the running build's sha.
4. `curl -b /tmp/lrr.cookies "$BASE/api/diagnostics/events?level=error"` → the event entries with
   timestamps, level, source and message.
5. Repeat the same failure 3 times → the issue's `count` grows, `issues` length does not.
6. `curl -b /tmp/lrr.cookies "$BASE/api/diagnostics/settings"` → `usage.events` grew, `drops` all zero.

**Expected**: all of the above with networking disabled; no request leaves the machine (`ss -tnp` during
the steps shows no connection to a tracker).

## Scenario 2 — bundle and feedback text *(offline)*

1. Pick a `from`/`to` covering the events from scenario 1.
2. `curl -b /tmp/lrr.cookies -H 'Content-Type: application/json' -d @spec.json \
   "$BASE/api/diagnostics/bundles/preview"` → a file list with sizes and `over_cap: false`.
3. `curl -b /tmp/lrr.cookies -H 'Content-Type: application/json' -d @spec.json \
   "$BASE/api/diagnostics/bundles" -o /tmp/bundle.zip && unzip -l /tmp/bundle.zip` → the previewed
   entries, including `manifest.json` and `README.txt`.
4. `unzip -p /tmp/bundle.zip manifest.json | jq '.selection'` → the frozen `SelectionSpec`.
5. `curl -b /tmp/lrr.cookies -H 'Content-Type: application/json' -d @spec.json \
   "$BASE/api/diagnostics/bundles/text" | jq` → `zh` and `en` titles and bodies, both naming the
   release, the range, the request id and the bundle name.
6. Grep the bundle for leaks: `unzip -p /tmp/bundle.zip | grep -E 'lrrg:|rt=|X-Share-Token|/home/|/temp/'`
   → no matches.
7. With `include_archive_names` off, confirm no archive title or filename appears in the bundle; turn it
   on and regenerate → titles appear only where the opt-in allows.

**Expected**: a zip that the administrator can attach to a GitHub issue as-is.

## Scenario 3 — time-boxed forwarding

1. `PUT /api/diagnostics/settings` with `{"tracker_dsn": "http://<key>@127.0.0.1:8080/<project>"}`.
2. `POST /api/diagnostics/window {"duration_secs": 300}` → `open: true`, `remaining_secs` ≈ 300.
3. Provoke two errors; within 10 s `GET /api/diagnostics/window` shows `sent` ≥ 2 and the tracker's UI
   shows the two issues (grouped, with the release).
4. `DELETE /api/diagnostics/window` → `open: false`.
5. Provoke a third error, wait 2 minutes: `sent` does not increase and the tracker receives nothing.
6. Reopen the window, reload the page: the countdown continues from the original deadline (it does not
   restart).
7. Reopen, then `docker restart` the instance: the deadline is unchanged; if it expired while down, the
   window reads as closed.
8. Point the DSN at an unreachable port and open a window: errors stay local, `failed` grows and
   `last_error` explains it, and the window does not extend itself.

## Scenario 4 — release identity and symbolication

1. Break the frontend build deliberately (`throwing` component), build a release image with
   `LANRURUGI_GIT_SHA=<sha>`, run it, provoke the error.
2. `GET /api/diagnostics/issues/{fingerprint}` → the stack frames show file/line/function for the
   original source (maps present in the image).
3. Generate a bundle → the resolved frames are inside, and the zip contains **no** `.map` file.
4. Build the same code without maps (or remove the artifact directory) and repeat → frames stay raw,
   the issue and the manifest still carry the release and the chunk file.
5. Confirm the static handler refuses `/sourcemaps/...` (404), and that `dist` does not contain maps.

## Scenario 5 — offline bundle replayed to a tracker (P3)

1. Produce a bundle offline (scenario 2).
2. On a connected machine, `POST /api/diagnostics/replay` with the bundle → `{"sent": n, "failed": 0}`.
3. The tracker shows the same events with their original `event_id`, timestamps and releases.
4. Replay the same bundle again → the tracker does not double-count (`event_id` dedup).

## Scenario 6 — limits and self-protection

1. Raise the store limits down to `{"max_events": 10}` and provoke 20 distinct errors →
   `GET /api/diagnostics/settings` shows `usage.events` ≤ 10 and `drops.retention` > 0; the issue list
   still lists the evicted issues with their counts.
2. Stop Redis, then provoke errors → the application keeps working, `drops.store_error` grows.
3. Flood with identical errors rapidly → one stored event per short window, `count` increases, and the
   store does not fill up.
4. A guest (no session) opening the settings page → no diagnostics section; direct
   `GET /api/diagnostics/issues` → 403 by policy.

## Scenario 7 — the old page still works

1. `GET /logs/general` (cookie session) still returns the raw tail.
2. The main navigation no longer links it; the diagnostics section links it as "raw log output".
