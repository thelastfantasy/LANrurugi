#!/usr/bin/env bash
# Fails when the release image would ship without a binary the server spawns at runtime.
#
# Added after beta.1/beta.2: `Dockerfile` built and copied only `lanrurugi-server` +
# `lanrurugi-book-worker`, while `Dockerfile.dev` also built the embed and GPU workers. The dev
# container therefore behaved correctly and every release image silently lost recommendations, AI
# grouping and local translation (`spawn` fails with `BinaryNotFound`, each call site degrades to
# "no result", so nothing ever logged an error).
#
# Two checks, both cheap enough for the PR path:
#   1. every `-p` package `Dockerfile.dev` builds is also built by `Dockerfile` (the two lists must
#      not drift apart again);
#   2. every worker the server spawns from its own directory is in that set *and* copied to
#      /usr/local/bin by the release `Dockerfile`.
#
# `--image <ref>` additionally verifies a built image (used by release.yml before anything is
# tagged): every expected binary is present and executable inside it.
set -euo pipefail

cd "$(dirname "$0")/.."

# The binaries `embed_worker_client` / `gpu_worker_client` / the e-book client look for next to the
# running server. Adding another worker crate means adding it here too — that is the point.
WORKERS=(lanrurugi-book-worker lanrurugi-embed-worker lanrurugi-gpu-worker)

fail=0
note() { printf '  %s\n' "$*" >&2; }

packages() {
  # `-p <crate>` inside a `cargo build` invocation; the release Dockerfile also builds the docgen
  # tool, which is fine — the comparison below is one-directional.
  grep -oE '(^|[[:space:]])-p[[:space:]]+lanrurugi-[a-z0-9-]+' "$1" | awk '{print $NF}' | sort -u
}

release=$(packages Dockerfile)
dev=$(packages Dockerfile.dev)

echo "release image builds: $(echo "$release" | tr '\n' ' ')"
echo "dev image builds:     $(echo "$dev" | tr '\n' ' ')"

missing_build=$(comm -13 <(echo "$release") <(echo "$dev"))
if [ -n "$missing_build" ]; then
  echo "FAIL: Dockerfile.dev builds packages the release Dockerfile does not:" >&2
  note "$missing_build"
  fail=1
fi

for worker in "${WORKERS[@]}"; do
  if ! grep -qx "$worker" <<<"$release"; then
    echo "FAIL: $worker is spawned by the server but the release image does not build it" >&2
    fail=1
  fi
  if ! grep -qE "COPY .*${worker} +/usr/local/bin/${worker}" Dockerfile; then
    echo "FAIL: $worker is built but not copied into the release image" >&2
    fail=1
  fi
done

if [ "${1:-}" = "--image" ]; then
  image="${2:?--image needs an image reference}"
  echo "checking image: $image"
  runtime=$(command -v docker || command -v podman)
  if [ -z "$runtime" ]; then
    echo "FAIL: --image needs docker or podman" >&2
    exit 1
  fi
  # `--entrypoint sh`: the image's own entrypoint starts Redis and the server, which is not what a
  # binary-presence check wants (and would leave a container running).
  out=$("$runtime" run --rm --entrypoint sh "$image" -c '
    rc=0
    for b in lanrurugi-server '"${WORKERS[*]}"'; do
      if [ -x "/usr/local/bin/$b" ]; then echo "ok $b"; else echo "MISSING $b"; rc=1; fi
    done
    lanrurugi-server --version >/dev/null 2>&1 || { echo "MISSING lanrurugi-server --version"; rc=1; }
    exit $rc
  ') || fail=1
  echo "$out" | sed 's/^/  /'
fi

if [ "$fail" -ne 0 ]; then
  echo >&2
  echo "The release image would not behave like the dev container. Fix Dockerfile (or Dockerfile.dev)" >&2
  echo "before merging — see the beta.1/beta.2 incident this check exists for." >&2
  exit 1
fi

echo "OK: release image builds and copies every worker the server spawns."
