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

redis_ready=false
for i in $(seq 1 50); do
  if [ "$(redis-cli -p 16379 PING 2>/dev/null)" = "PONG" ]; then
    redis_ready=true
    break
  fi
  sleep 0.1
done
if [ "$redis_ready" != "true" ]; then
  echo "Redis did not become ready in time" >&2
  exit 1
fi
# Give Redis a moment to finish any post-load readiness work before the app connects. Without
# this short pause the app's deadpool PING can race Redis's AOF/RDB load and exit with
# "Redis PING did not return PONG" (observed on a dev-container recreate).
sleep 0.2

# Best-effort MaxMind GeoLite2-City download/refresh — additive to the activity-log device-info
# feature (`lanrurugi_api::geoip`'s own docs), never required for the rest of the server to start.
# `geoipupdate` itself only reads its account/license key from /etc/GeoIP.conf, not environment
# variables directly, so this writes that file from LANRURUGI_GEOIP_ACCOUNT_ID/
# LANRURUGI_GEOIP_LICENSE_KEY (a free MaxMind account's own credentials — see
# https://www.maxmind.com/en/geolite2/signup) each container start, then runs it once. Silently
# skipped (not a fatal error) when either variable is unset — a deployment that never opted into
# geo data gets a server with no geo lookups, not a boot failure. `/var/lib/GeoIP` is a real
# directory in the image (created below), not required to be a mounted volume — an operator who
# wants the database to survive a container recreate without re-downloading it can mount one there,
# but it isn't required for this to work at all.
if [ -n "$LANRURUGI_GEOIP_ACCOUNT_ID" ] && [ -n "$LANRURUGI_GEOIP_LICENSE_KEY" ]; then
  cat > /etc/GeoIP.conf <<EOF
AccountID $LANRURUGI_GEOIP_ACCOUNT_ID
LicenseKey $LANRURUGI_GEOIP_LICENSE_KEY
EditionIDs GeoLite2-City
EOF
  if geoipupdate -d /var/lib/GeoIP; then
    echo "geoip: GeoLite2-City database ready"
  else
    echo "geoip: geoipupdate failed — continuing without geo data" >&2
  fi
else
  echo "geoip: LANRURUGI_GEOIP_ACCOUNT_ID/LANRURUGI_GEOIP_LICENSE_KEY not set — skipping, no geo data will be recorded"
fi

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
# Wait on whichever actually died so the container exit status reflects the real failure, instead
# of blocking on the still-alive half (a crashed Redis with the app alive would otherwise keep the
# container up indefinitely).
if ! kill -0 "$APP_PID" 2>/dev/null; then
  wait "$APP_PID" 2>/dev/null
else
  wait "$REDIS_PID" 2>/dev/null
fi
EXIT_CODE=$?
shutdown
exit "$EXIT_CODE"
