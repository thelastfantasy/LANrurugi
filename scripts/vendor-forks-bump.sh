#!/usr/bin/env bash
# Moves the iroh stack, and the forks under vendor/ that ride on it, to the newest iroh release:
# the workspace's iroh/iroh-relay requirements, the crates' netwatch/n0-dns-resolver requirements, each
# vendor/patches/<crate>/VERSION, and vendor/<crate> itself (via scripts/vendor-forks.sh).
#
#   scripts/vendor-forks-bump.sh [<iroh version>]
#
# Prints `unchanged` and exits 0 when the workspace already requires that iroh. Cargo.lock is left
# to the caller (`mise run cargo-update -- iroh iroh-relay netwatch n0-dns-resolver iroh-base iroh-dns`),
# as is building.
# Fails when a patch no longer applies; scripts/vendor-forks.sh says where the .rej files are.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"

# `name version` lines: iroh's newest stable release (or the one asked for), then the newest
# iroh-relay, netwatch and n0-dns-resolver releases its requirements accept.
targets="$(python3 - "${1:-}" <<'EOF'
import json, sys, urllib.request

def get(path):
    req = urllib.request.Request(f"https://crates.io/api/v1/{path}",
                                 headers={"User-Agent": "lanrurugi vendor-forks-bump"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)

def parse(v):
    return tuple(int(x) for x in v.split("+")[0].split("-")[0].split("."))

def accepts(req, v):
    # Cargo's default (caret) requirement, which is all iroh uses for these two.
    want = parse(req.lstrip("^= "))
    have = parse(v)
    if have < want:
        return False
    if want[0] > 0:
        return have[0] == want[0]
    return have[:2] == want[:2]

def newest(crate, req):
    versions = [v["num"] for v in get(f"crates/{crate}/versions?per_page=100")["versions"]
                if not v["yanked"] and "-" not in v["num"]]
    ok = [v for v in versions if accepts(req, v)]
    if not ok:
        sys.exit(f"no {crate} release satisfies {req}")
    return max(ok, key=parse)

def deps(crate, version):
    return {d["crate_id"]: d["req"] for d in get(f"crates/{crate}/{version}/dependencies")["dependencies"]
            if d["kind"] == "normal"}

iroh = sys.argv[1] or get("crates/iroh")["crate"]["max_stable_version"]
iroh_deps = deps("iroh", iroh)
print("iroh", iroh)
for crate in ("iroh-relay", "netwatch"):
    print(crate, newest(crate, iroh_deps[crate]))
# n0-dns-resolver comes in through iroh-dns.
iroh_dns = newest("iroh-dns", iroh_deps["iroh-dns"])
print("n0-dns-resolver", newest("n0-dns-resolver", deps("iroh-dns", iroh_dns)["n0-dns-resolver"]))
EOF
)"
iroh="$(awk '$1 == "iroh" { print $2 }' <<< "$targets")"
relay="$(awk '$1 == "iroh-relay" { print $2 }' <<< "$targets")"
netwatch="$(awk '$1 == "netwatch" { print $2 }' <<< "$targets")"
dns="$(awk '$1 == "n0-dns-resolver" { print $2 }' <<< "$targets")"

current="$(sed -n 's/^iroh = { version = "\([^"]*\)".*/\1/p' Cargo.toml)"
if [ "$current" = "$iroh" ]; then
  echo unchanged
  exit 0
fi

sed -i "s/^iroh = { version = \"[^\"]*\"/iroh = { version = \"$iroh\"/" Cargo.toml
sed -i "s/^iroh-relay = { version = \"[^\"]*\"/iroh-relay = { version = \"$relay\"/" Cargo.toml
for manifest in crates/*/Cargo.toml; do
  sed -i "s/^netwatch = \"[^\"]*\"/netwatch = \"$netwatch\"/" "$manifest"
  sed -i "s/^n0-dns-resolver = \"[^\"]*\"/n0-dns-resolver = \"$dns\"/" "$manifest"
done
echo "$relay" > vendor/patches/iroh-relay/VERSION
echo "$netwatch" > vendor/patches/netwatch/VERSION
echo "$dns" > vendor/patches/n0-dns-resolver/VERSION
scripts/vendor-forks.sh
echo "iroh $current -> $iroh (iroh-relay $relay, netwatch $netwatch, n0-dns-resolver $dns)"
