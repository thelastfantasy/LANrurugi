#!/bin/sh
# Start-up smoke test of a native release package: unpack it, start a throwaway Redis, run the
# server with nothing but data locations set, and check it serves the API and the web interface.
#
# Usage: smoke-native.sh <package.tar.gz>
#   SMOKE_EXPECT   space-separated components the start-up summary must report as found
#                  (pdfium onnxruntime djvulibre deno); default: none
#   SMOKE_LIBRARY  directory copied into the library before start (optional); the test then waits
#                  for the start-up scan and prints the files whose pages cannot be listed
#   SMOKE_STRICT   non-empty: any such file fails the test
# Needs redis-server and curl on PATH.
set -eu

package="${1:?usage: smoke-native.sh <package.tar.gz>}"
case "$package" in /*) ;; *) package="$(pwd)/$package" ;; esac
work="$(mktemp -d)"
redis_pid="" server_pid=""
cleanup() {
    [ -n "$server_pid" ] && kill "$server_pid" 2>/dev/null || true
    [ -n "$redis_pid" ] && kill "$redis_pid" 2>/dev/null || true
    wait 2>/dev/null || true
    rm -rf "$work"
}
trap cleanup EXIT

tar -xzf "$package" -C "$work"
prefix="$(find "$work" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
data="$work/data"
mkdir -p "$data/content" "$data/thumb" "$data/temp" "$data/log" "$data/redis"
if [ -n "${SMOKE_LIBRARY:-}" ]; then
    cp -R "$SMOKE_LIBRARY"/. "$data/content/"
fi

redis_port=26379
http_port=23000
redis-server --port "$redis_port" --bind 127.0.0.1 --dir "$data/redis" --save "" --appendonly no \
    >"$work/redis.log" 2>&1 &
redis_pid=$!

# Started from another directory on purpose: resources must be found from the install prefix.
(
    cd /
    exec "$prefix/bin/lanrurugi-server" serve \
        --redis-url "redis://127.0.0.1:$redis_port" \
        --library-path "$data/content" --thumb-dir "$data/thumb" \
        --temp-dir "$data/temp" --log-dir "$data/log" \
        --bind "127.0.0.1:$http_port" --disable-update-check
) >"$work/server.log" 2>&1 &
server_pid=$!

# The server log with terminal colour codes removed (portable: no `\x1b` in BSD sed).
plain() {
    tr -d '\033' <"$work/server.log" | sed 's/\[[0-9;]*m//g'
}

fail() {
    echo "smoke-native: FAIL — $1" >&2
    echo "--- server log ---" >&2
    plain | grep -v on_failure | tail -n 60 >&2 || true
    exit 1
}

i=0
until curl -fsS "http://127.0.0.1:$http_port/api/info" >"$work/info.json" 2>/dev/null; do
    i=$((i + 1))
    [ "$i" -ge 90 ] && fail "the API did not answer within 90 s"
    kill -0 "$server_pid" 2>/dev/null || fail "the server exited"
    sleep 1
done
curl -fsS "http://127.0.0.1:$http_port/" | grep -qi '<html' || fail "the web interface is not served"
curl -fsS -o /dev/null "http://127.0.0.1:$http_port/docs/" || fail "the documentation is not served"
plain | grep -q 'optional components' || fail "no start-up component summary"

summary="$(plain | grep 'optional components' | head -n 1)"
for component in ${SMOKE_EXPECT:-}; do
    case "$summary" in
        *" $component=not found"*) fail "$component was not found" ;;
    esac
done

if [ -n "${SMOKE_LIBRARY:-}" ]; then
    # Files present before start are catalogued by the start-up scan, which lists every book's
    # pages (the step that loads PDFium and the typesetter) before it reports.
    want="$(find "$data/content" -type f | wc -l | tr -d ' ')"
    i=0
    until plain | grep -q 'startup scan complete'; do
        i=$((i + 1))
        [ "$i" -ge 180 ] && fail "the start-up scan did not finish within 180 s"
        sleep 1
    done
    scanned="$(plain | grep 'startup scan complete' | sed -E 's/.* scanned=([0-9]+).*/\1/')"
    [ "$scanned" -ge "$want" ] || fail "the start-up scan found $scanned of $want library files"
    echo "smoke-native: $scanned library files catalogued"
    plain | grep 'cannot list pages' | sed -E 's/.*path=([^ ]+) error=/  \1: /' || true
    if [ -n "${SMOKE_STRICT:-}" ] && plain | grep -q 'cannot list pages'; then
        fail "some library files could not be read"
    fi
fi

echo "smoke-native: OK — $(basename "$package")"
echo "$summary"
plain | grep 'unavailable on this system' | head -n 1 || true
