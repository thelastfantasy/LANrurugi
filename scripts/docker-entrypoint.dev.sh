#!/bin/sh
# Dev-mode entrypoint (Dockerfile.dev / compose.dev.yaml only — see docker-entrypoint.sh for the
# production entrypoint, which this deliberately does not touch or share). Starts three
# processes in one container: redis-server, a compiled `lanrurugi-server` bound to an *internal*
# port, and `vite dev` — Vite is the container's actual public port, serving the frontend from
# the bind-mounted host source (so edits show up live) and proxying `/api/*` through to the
# backend.
#
# Same trap-based supervisor shape as docker-entrypoint.sh (see its own doc comment for why: PID 1
# stays this shell so `trap` keeps working, no `exec`).
set -e

redis-server /etc/lanrurugi/redis.conf &
REDIS_PID=$!

for i in $(seq 1 50); do
  if redis-cli -p 16379 PING >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

# `--log-dir /log` matches `compose.dev.yaml`'s own named volume mount at that exact path — left
# unset, this falls back to `--log-dir`'s own default of `./log` (relative to `WORKDIR /workspace`,
# per `main.rs`), a plain unmounted container path the `/logs` API endpoint's own log_dir has no
# reason to know about and that vanishes on every container recreate — silently making every log
# category permanently empty from the `/logs` page's perspective, not just on this specific run.
#
# No `--static-dir` here — this backend only ever needs to serve `/api/*` in dev mode (`vite dev`
# is the container's real public frontend, per this script's own doc comment above). An earlier
# version of this added `--static-dir` + a one-time `vite build` step so `vite.config.ts`'s own
# theme-injection plugin could fetch a backend-rendered `index.html` to use as its dev-mode
# template, but that template turned out to reference the *production* bundle
# (`dist/assets/index-XXXX.js`), not the dev-mode source entry (`/src/main.tsx`) `vite dev` itself
# needs — a real, confirmed-live blank-page regression (a 404 on the bundled script vite dev never
# serves). The plugin now only fetches the theme *value* itself (`GET /api/theme`, no
# `--static-dir` needed for that route) and substitutes it into Vite's own already-correct
# dev-mode HTML, so this backend never needs to know about the frontend's static build at all.
lanrurugi-server serve --bind 0.0.0.0:3001 --log-dir /log &
APP_PID=$!

# Direct `node_modules/.bin/vite` invocation, not `pnpm exec`/`pnpm --filter` — `pnpm exec` runs
# its own "deps status check" first (re-verifying/reinstalling on every container start, and, in
# this image, failing outright on an `[ERR_PNPM_IGNORED_BUILDS]` for `sharp` despite
# `pnpm-workspace.yaml`'s own `allowBuilds` — verified the hard way, not worth chasing further
# when the direct binary works and matches how every other tool in this repo/session is already
# invoked, e.g. `./node_modules/.bin/tsc`/`eslint`). `LANRURUGI_DEV_BACKEND_PORT=3001` matches the
# backend's own `--bind` above (see vite.config.ts's own doc comment for this env var).
cd /workspace/apps/frontend
LANRURUGI_DEV_BACKEND_PORT=3001 ./node_modules/.bin/vite --host 0.0.0.0 --port 3000 &
VITE_PID=$!

shutdown() {
  kill -TERM "$VITE_PID" 2>/dev/null
  wait "$VITE_PID" 2>/dev/null
  kill -TERM "$APP_PID" 2>/dev/null
  wait "$APP_PID" 2>/dev/null
  kill -TERM "$REDIS_PID" 2>/dev/null
  wait "$REDIS_PID" 2>/dev/null
}
trap shutdown TERM INT

while kill -0 "$VITE_PID" 2>/dev/null && kill -0 "$APP_PID" 2>/dev/null && kill -0 "$REDIS_PID" 2>/dev/null; do
  sleep 1
done
wait "$VITE_PID" 2>/dev/null
EXIT_CODE=$?
shutdown
exit "$EXIT_CODE"
