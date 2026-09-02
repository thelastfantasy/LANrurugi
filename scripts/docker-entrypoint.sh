#!/bin/sh
# Starts the bundled `redis-server` and `lanrurugi-server` together in one container — matching
# legacy LANraragi's own real single-container deployment (Redis + app in one container; legacy
# supervises this via s6-overlay, this repo has no existing s6-overlay usage anywhere else so a
# plain POSIX trap-based wrapper is used instead).
#
# This script stays PID 1 for the container's whole lifetime (deliberately never `exec`s into
# either child) so its own `trap` keeps working — an `exec lanrurugi-server` here would replace
# this shell process entirely, silently dropping the trap below and leaving redis-server killed
# ungracefully (no AOF/RDB flush) on `docker stop` instead of shutting down cleanly first.
set -e

redis-server /etc/lanrurugi/redis.conf &
REDIS_PID=$!

for i in $(seq 1 50); do
  if redis-cli -p 16379 PING >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

lanrurugi-server "$@" &
APP_PID=$!

shutdown() {
  kill -TERM "$APP_PID" 2>/dev/null
  wait "$APP_PID" 2>/dev/null
  kill -TERM "$REDIS_PID" 2>/dev/null
  wait "$REDIS_PID" 2>/dev/null
}
trap shutdown TERM INT

# Whichever of the two exits first ends the container — a crashed Redis with the app still
# spinning (or vice versa) isn't a state worth staying up in. Polls both PIDs via `kill -0` rather
# than `wait -n` (a bashism unavailable in Debian's `/bin/sh`, which is dash) so either process
# dying is noticed promptly, not just whichever `wait` happened to be given.
while kill -0 "$APP_PID" 2>/dev/null && kill -0 "$REDIS_PID" 2>/dev/null; do
  sleep 1
done
wait "$APP_PID" 2>/dev/null
EXIT_CODE=$?
shutdown
exit "$EXIT_CODE"
