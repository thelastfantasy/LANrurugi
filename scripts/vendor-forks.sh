#!/usr/bin/env bash
# Rebuilds each patched crate under vendor/ from its crates.io release plus our patches, so a fork is
# never edited by hand and moving it to a new upstream version is one command.
#
#   scripts/vendor-forks.sh [--check] [<crate> ...]
#
# For every vendor/patches/<crate>/ (or just the crates named): download the release named in its
# VERSION file, verify it against the checksum crates.io publishes, apply its *.patch files in order,
# append its Cargo.append.toml (if any) to Cargo.toml, add its FORK.md, and replace vendor/<crate>
# with the result. `--check` builds the same tree in a
# temporary directory and fails if vendor/<crate> differs from it (CI runs this).
#
# Moving a fork forward: write the new version into VERSION and run this. When a patch no longer
# applies, git leaves <file>.rej next to the source in the kept work directory; fix the source there,
# regenerate the patch with `git diff --no-index --no-prefix a b` against a clean copy, and rerun.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
mode=sync
if [ "${1:-}" = "--check" ]; then
  mode=check
  shift
fi

crates=("$@")
if [ ${#crates[@]} -eq 0 ]; then
  for dir in "$repo"/vendor/patches/*/; do
    crates+=("$(basename "$dir")")
  done
fi

work="$(mktemp -d)"
keep_work=no
cleanup() { [ "$keep_work" = yes ] || rm -rf "$work"; }
trap cleanup EXIT

status=0
for crate in "${crates[@]}"; do
  patches="$repo/vendor/patches/$crate"
  if [ ! -f "$patches/VERSION" ]; then
    echo "vendor-forks: $patches/VERSION is missing" >&2
    exit 1
  fi
  version="$(tr -d '[:space:]' < "$patches/VERSION")"
  archive="$work/$crate-$version.crate"
  curl -fsSL -A "lanrurugi vendor-forks (https://github.com/thelastfantasy/LANrurugi)" \
    -o "$archive" "https://static.crates.io/crates/$crate/$crate-$version.crate"
  expected="$(curl -fsSL -A "lanrurugi vendor-forks" "https://crates.io/api/v1/crates/$crate/$version" \
    | grep -o '"checksum":"[0-9a-f]*"' | head -1 | cut -d'"' -f4)"
  actual="$(sha256sum "$archive" | cut -d' ' -f1)"
  if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "vendor-forks: $crate $version checksum mismatch (crates.io: ${expected:-none}, downloaded: $actual)" >&2
    exit 1
  fi

  tree="$work/$crate"
  mkdir -p "$tree"
  tar -xzf "$archive" -C "$tree" --strip-components=1
  for patch in "$patches"/*.patch; do
    [ -e "$patch" ] || continue
    if ! (cd "$tree" && git apply -p1 --reject --whitespace=nowarn "$patch"); then
      keep_work=yes
      echo "vendor-forks: $(basename "$patch") does not apply to $crate $version; see the .rej files in $tree" >&2
      exit 1
    fi
  done
  # Dependencies the fork adds go in Cargo.append.toml rather than a patch hunk: a hunk against the
  # dependency list breaks whenever upstream touches a neighbouring entry, appending a table never does.
  if [ -f "$patches/Cargo.append.toml" ]; then
    { echo; cat "$patches/Cargo.append.toml"; } >> "$tree/Cargo.toml"
  fi
  cp "$patches/FORK.md" "$tree/FORK.md"

  target="$repo/vendor/$crate"
  if [ "$mode" = check ]; then
    if ! diff -r "$tree" "$target" > "$work/$crate.diff"; then
      echo "vendor-forks: vendor/$crate is not $crate $version plus vendor/patches/$crate:" >&2
      head -40 "$work/$crate.diff" >&2
      status=1
    fi
  else
    rm -rf "$target"
    mv "$tree" "$target"
    echo "vendor-forks: vendor/$crate <- $crate $version + $(ls "$patches"/*.patch | wc -l) patch(es)"
  fi
done
exit "$status"
