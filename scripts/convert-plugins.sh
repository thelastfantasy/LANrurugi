#!/usr/bin/env bash
# Batch-converts legacy LANraragi Perl plugins (Metadata/Login/Download) into TypeScript under
# plugins/<category>/, via `lanrurugi-plugin-converter`. Wraps the same per-file steps that were
# done by hand for the first 21 Metadata plugins: run the converter inside the `lanrurugi-dev`
# container (the host has no Rust/Perl/PPI toolchain — see `Dockerfile.build`), prepend a
# provenance/warnings header, then verify with `deno check` on the host (deno *is* available on
# the host via `mise`'s own toolchain — see `.mise.toml`).
#
# Usage: scripts/convert-plugins.sh <path-to-legacy-LANraragi-checkout> [--force] [category ...]
#   categories: metadata login download (default: all three)
#   --force:    overwrite a destination file even if it already has manually-reviewed
#               `declared_permissions` (see the safety check below) — off by default so a second
#               run (e.g. after a converter bugfix) can't silently clobber hand-verified net-host
#               lists the way the first 21 Metadata plugins needed (7 of them required manually
#               checking real URL literals in the Perl source — a converter re-run has no way to
#               redo that verification itself).
#
# `Plugin/Scripts/*.pm` is deliberately not a category here: legacy's own Scripts plugins
# (FolderToCat, SourceFinder, nHentaiSourceConverter) are reimplemented as native Rust REST
# endpoints in `lanrurugi-api::scripts` instead of going through this Deno-plugin pipeline (see
# that module's own doc comment) — there is nothing under `Plugin/Scripts/` for this script to
# convert.
set -euo pipefail

usage() {
  echo "Usage: $0 <path-to-legacy-LANraragi-checkout> [--force] [category ...]" >&2
  echo "  categories: metadata login download (default: all three)" >&2
  exit 1
}

[ $# -ge 1 ] || usage
LEGACY_SRC="$1"
shift

FORCE=0
CATEGORIES=()
for arg in "$@"; do
  case "$arg" in
    --force) FORCE=1 ;;
    *) CATEGORIES+=("$arg") ;;
  esac
done
[ ${#CATEGORIES[@]} -eq 0 ] && CATEGORIES=(metadata login download)

[ -d "$LEGACY_SRC/lib/LANraragi/Plugin" ] || {
  echo "error: '$LEGACY_SRC/lib/LANraragi/Plugin' not found — is this a real LANraragi checkout?" >&2
  exit 1
}
LEGACY_SRC="$(cd "$LEGACY_SRC" && pwd)"

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PODMAN="$(command -v podman || command -v docker || true)"
[ -n "$PODMAN" ] || { echo "error: podman/docker not found on PATH" >&2; exit 1; }
command -v deno >/dev/null 2>&1 || {
  echo "error: deno not found on PATH — run this via 'mise run convert-plugins ...' so .mise.toml's pinned deno is active" >&2
  exit 1
}

declare -A LEGACY_SUBDIR=( [metadata]=Metadata [login]=Login [download]=Download )
for category in "${CATEGORIES[@]}"; do
  if [ -z "${LEGACY_SUBDIR[$category]:-}" ]; then
    if [ "$category" = "script" ] || [ "$category" = "scripts" ]; then
      echo "note: skipping '$category' — legacy's Scripts plugins are reimplemented natively in" \
           "lanrurugi-api::scripts, not converted (see this script's own header comment)" >&2
    else
      echo "error: unknown category '$category' (known: metadata, login, download)" >&2
    fi
    exit 1
  fi
done

STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT

# One container invocation for the whole batch (rather than one per file) — each `cargo run`
# inside it is still a fresh process per plugin (the converter has no batch mode of its own), but
# this way there's only one container-startup/dependency-check cost instead of dozens.
inner_script='
set -euo pipefail
for subdir in "$@"; do
  dir="/legacy/lib/LANraragi/Plugin/$subdir"
  [ -d "$dir" ] || { echo "no such directory: $dir" >&2; continue; }
  mkdir -p "/stage/$subdir"
  shopt -s nullglob
  for pm in "$dir"/*.pm; do
    name="$(basename "$pm" .pm)"
    echo "converting $subdir/$name.pm ..." >&2
    if cargo run -q -p lanrurugi-plugin-converter --bin lanrurugi-plugin-converter -- \
        convert "$pm" -o "/stage/$subdir/$name.ts" 2> "/stage/$subdir/$name.warn"; then
      :
    else
      echo "FAILED: $subdir/$name.pm (see /stage/$subdir/$name.warn — not written to plugins/)" >&2
    fi
  done
done
'
legacy_subdirs=()
for category in "${CATEGORIES[@]}"; do
  legacy_subdirs+=("${LEGACY_SUBDIR[$category]}")
done

"$PODMAN" run --rm --memory=4g --memory-swap=4g \
  -v "$REPO_ROOT":/workspace \
  -v "$LEGACY_SRC":/legacy:ro \
  -v "$STAGE_DIR":/stage \
  -w /workspace lanrurugi-dev \
  bash -c "$inner_script" bash "${legacy_subdirs[@]}"

# Header + `deno check` verification + final placement — all on the host (has deno, no container
# needed for this half).
clean=()
dirty=()
skipped=()
failed=()

for category in "${CATEGORIES[@]}"; do
  legacy_subdir="${LEGACY_SUBDIR[$category]}"
  stage_subdir="$STAGE_DIR/$legacy_subdir"
  [ -d "$stage_subdir" ] || continue
  dest_dir="$REPO_ROOT/plugins/$category"
  mkdir -p "$dest_dir"

  shopt -s nullglob
  for tmp_ts in "$stage_subdir"/*.ts; do
    name="$(basename "$tmp_ts" .ts)"
    tmp_warn="$stage_subdir/$name.warn"
    dest_name="$(echo "$name" | tr '[:upper:]' '[:lower:]').ts"
    dest_path="$dest_dir/$dest_name"
    rel_pm="LANraragi/lib/LANraragi/Plugin/$legacy_subdir/$name.pm"

    if [ -f "$dest_path" ] && grep -q 'declared_permissions' "$dest_path" \
        && ! grep -q 'TODO: host(s)' "$dest_path" && [ "$FORCE" != "1" ]; then
      dest_path="${dest_path%.ts}.new.ts"
      echo "note: $dest_dir/$dest_name already has reviewed permissions — writing fresh output to" \
           "$dest_path instead (diff it in by hand, or pass --force to overwrite directly)" >&2
    fi

    warnings="$(grep '^  - ' "$tmp_warn" 2>/dev/null | sed 's/^  - //' | sort -u || true)"

    {
      echo "// Converted from $rel_pm via"
      echo "// \`lanrurugi-plugin-converter\` (\`cargo run -p lanrurugi-plugin-converter -- convert ...\`),"
      echo "// regenerated by \`scripts/convert-plugins.sh\` on $(date +%F)."
      if [ -n "$warnings" ]; then
        echo "//"
        echo "// Known limitations (from the converter's own warnings — review, don't blindly trust):"
        while IFS= read -r warning; do
          [ -z "$warning" ] && continue
          printf '%s\n' "$warning" | fold -s -w 94 \
            | awk 'NR==1{print "//   - " $0} NR>1{print "//     " $0}'
        done <<< "$warnings"
      fi
      if grep -q 'TODO: host(s)' "$tmp_ts"; then
        echo "//"
        echo "// NOTE: this plugin makes HTTP calls but declared_permissions.net is still the"
        echo "// converter's placeholder — fill in the real host(s), verified against the URL"
        echo "// literals in the original Perl source, before this plugin can actually reach them"
        echo "// (Deno's --allow-net grant is scoped to exactly this list)."
      fi
      echo
      cat "$tmp_ts"
    } > "$dest_path"

    if deno check "$dest_path" >/dev/null 2>&1; then
      clean+=("$category/$dest_name")
    else
      dirty+=("$category/$dest_name")
    fi
  done

  for warn_only in "$stage_subdir"/*.warn; do
    name="$(basename "$warn_only" .warn)"
    [ -f "$stage_subdir/$name.ts" ] || failed+=("$legacy_subdir/$name.pm")
  done
done

echo
echo "== convert-plugins summary =="
echo "clean (deno check passed): ${#clean[@]}"
for f in "${clean[@]:-}"; do [ -n "$f" ] && echo "  - $f"; done
echo "dirty (deno check failed — inspect before use): ${#dirty[@]}"
for f in "${dirty[@]:-}"; do [ -n "$f" ] && echo "  - $f"; done
if [ "${#failed[@]}" -gt 0 ]; then
  echo "failed to convert at all: ${#failed[@]}"
  for f in "${failed[@]}"; do echo "  - $f"; done
fi
echo
echo "Remember: any file with a 'TODO: host(s)' declared_permissions placeholder needs its real"
echo "net host(s) filled in by hand (verified against URL literals in the original .pm) before"
echo "that plugin can make its HTTP calls."
