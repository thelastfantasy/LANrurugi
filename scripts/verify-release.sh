#!/usr/bin/env bash
# One-shot post-release verification: `scripts/verify-release.sh 1.0.0-beta.2`.
#
# Everything here is something that had to be checked by hand after a release — the beta.2 run
# needed its GHCR/Docker Hub tags, its four tarballs, the docs URL and the image contents poked at
# individually, and two of those had actually gone wrong (a half-uploaded release, and images
# missing the worker binaries that AI grouping/recommendations need).
#
# Needs: `gh` (authenticated), `curl`, `python3`. Uses `docker`/`podman` when available for the
# image-contents check and skips it otherwise.
#
# Checks:
#   1. GitHub release exists, is a prerelease for a `-...` version, and has all six assets with
#      non-zero size
#   2. GHCR and Docker Hub both serve `<version>`, `<version>-<sha>` and the channel tag, each as a
#      multi-arch (amd64 + arm64) index, with `<version>` and `<channel>` resolving identically
#   3. a prerelease never publishes `latest`
#   4. the docs site serves `/v<version>/`
#   5. the image actually contains every binary the server spawns (the beta.1/beta.2 defect)
set -uo pipefail

cd "$(dirname "$0")/.."

version="${1:?usage: scripts/verify-release.sh <version, e.g. 1.0.0-beta.2>}"
tag="v$version"
ghcr="thelastfantasy/lanrurugi"
hub="jade913/lanrurugi"
fail=0

ok()   { printf '  \033[32mok\033[0m   %s\n' "$*"; }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$*"; fail=1; }
skip() { printf '  skip %s\n' "$*"; }

# The published image tags come from Cargo.toml, not from the tag being verified, so a version
# that was never bumped verifies as "clean" while the images actually landed on the previous
# version (beta.5 shipped as 1.0.0-beta.4 that way). This is the cheapest possible place to notice.
declared=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
if [ "$declared" = "$version" ]; then ok "Cargo.toml version is $version"; else bad "Cargo.toml version is $declared, verifying $version"; fi

channel="latest"
[[ "$version" == *-* ]] && channel="beta"
prerelease="false"
[[ "$channel" == beta ]] && prerelease="true"

echo "verifying $tag (channel=$channel, prerelease=$prerelease)"

# --- 1. GitHub release + assets ------------------------------------------------------------
if gh release view "$tag" --json isPrerelease,assets > /tmp/lrr-release.json 2>/dev/null; then
  is_pre=$(gh release view "$tag" --json isPrerelease --jq '.isPrerelease')
  [ "$is_pre" = "$prerelease" ] && ok "release prerelease=$is_pre" || bad "release prerelease=$is_pre, expected $prerelease"
  assets=$(gh release view "$tag" --json assets --jq '.assets[].name' | sort)
  for want in SHA256SUMS "docs-$version.zip" \
              "lanrurugi-$version-linux-x86_64.tar.gz" \
              "lanrurugi-$version-linux-aarch64.tar.gz" \
              "lanrurugi-$version-macos-arm64.tar.gz" \
              "lanrurugi-$version-freebsd-x86_64.tar.gz"; do
    if grep -qx "$want" <<<"$assets"; then ok "asset $want"; else bad "missing asset $want"; fi
  done
  empty=$(gh release view "$tag" --json assets --jq '[.assets[] | select(.size == 0) | .name] | join(",")')
  [ -z "$empty" ] && ok "no empty assets" || bad "empty asset(s): $empty"
else
  bad "no GitHub release for $tag"
fi

# --- 2. registry manifests ----------------------------------------------------------------
check_registry() {
  local name=$1 registry=$2 repo=$3
  python3 - "$name" "$registry" "$repo" "$version" "$channel" "$prerelease" <<'PY'
import json, sys, urllib.error, urllib.request

name, registry, repo, version, channel, prerelease = sys.argv[1:7]

def token():
    if registry == "ghcr.io":
        url = f"https://ghcr.io/token?scope=repository:{repo}:pull"
    else:
        url = f"https://auth.docker.io/token?service=registry.docker.io&scope=repository:{repo}:pull"
    return json.loads(urllib.request.urlopen(url, timeout=30).read())["token"]

def manifest(ref):
    req = urllib.request.Request(f"https://{registry}/v2/{repo}/manifests/{ref}")
    req.add_header("Authorization", f"Bearer {tok}")
    req.add_header("Accept", "application/vnd.oci.image.index.v1+json, application/vnd.docker.distribution.manifest.list.v2+json")
    try:
        r = urllib.request.urlopen(req, timeout=30)
        return r.headers.get("Docker-Content-Digest"), json.loads(r.read())
    except urllib.error.HTTPError as e:
        return None, e.code

fail = 0
try:
    tok = token()
except Exception as e:  # noqa: BLE001
    print(f"  FAIL {name}: cannot get a pull token: {e}")
    sys.exit(1)

digests = {}
for ref in (version, channel):
    digest, doc = manifest(ref)
    if digest is None:
        print(f"  FAIL {name}:{ref} -> HTTP {doc}")
        fail = 1
        continue
    plats = sorted(f"{m['platform']['os']}/{m['platform']['architecture']}" for m in doc.get("manifests", []))
    missing = {"linux/amd64", "linux/arm64"} - set(plats)
    if missing:
        print(f"  FAIL {name}:{ref} missing {sorted(missing)} (has {plats})")
        fail = 1
    else:
        print(f"  ok   {name}:{ref} multi-arch {plats}")
    digests[ref] = digest

if len(digests) == 2 and digests[version] != digests[channel]:
    print(f"  FAIL {name}:{version} and :{channel} resolve to different manifests")
    fail = 1

if prerelease == "true":
    digest, code = manifest("latest")
    if digest is not None:
        print(f"  FAIL {name}:latest exists on a prerelease")
        fail = 1
    else:
        print(f"  ok   {name}:latest absent (prerelease)")

sys.exit(fail)
PY
}

echo "registry: ghcr"
check_registry "ghcr" "ghcr.io" "$ghcr" || fail=1
echo "registry: docker hub"
check_registry "dockerhub" "registry-1.docker.io" "$hub" || fail=1

# --- 3. docs site -------------------------------------------------------------------------
docs_url="https://docs.lrrg.dev/v$version/"
code=$(curl -s -o /dev/null -w '%{http_code}' "$docs_url" || echo 000)
[ "$code" = "200" ] && ok "docs $docs_url" || bad "docs $docs_url -> HTTP $code"

# --- 4. image contents --------------------------------------------------------------------
runtime=$(command -v docker || command -v podman || true)
if [ -z "$runtime" ]; then
  skip "image contents (no docker/podman on PATH)"
else
  image="ghcr.io/$ghcr:$version"
  if "$runtime" pull -q "$image" >/dev/null 2>&1; then
    # `--entrypoint sh`: the image's own entrypoint starts Redis and the server, which is not what
    # a binary-presence check wants (and would leave a container running for each probe).
    present=$("$runtime" run --rm --entrypoint sh "$image" -c '
      for b in lanrurugi-server lanrurugi-book-worker lanrurugi-embed-worker lanrurugi-gpu-worker; do
        test -x "/usr/local/bin/$b" && echo "$b"
      done' 2>/dev/null)
    for b in lanrurugi-server lanrurugi-book-worker lanrurugi-embed-worker lanrurugi-gpu-worker; do
      if grep -qx "$b" <<<"$present"; then ok "image has $b"; else bad "image is missing $b"; fi
    done
  else
    skip "image contents (could not pull $image)"
  fi
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — see the lines above."
  exit 1
fi
echo "all checks passed for $tag"
