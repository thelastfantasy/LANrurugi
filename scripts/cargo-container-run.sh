#!/usr/bin/env bash
# Runs a cargo subcommand inside the `lanrurugi-dev` container with the shared guardrails every
# `mise run {test,clippy,fmt-check,build}` task uses — CPU quota, capped parallelism, persistent
# target/registry cache, and an optional foreground-app-aware resource step-down. Added after a
# real host crash (2026-07-21) from an unbounded `cargo build` container.
#
# No `--memory`/`--cpu-shares` here: memory/priority capping is expected to come from a personal
# `~/.config/containers/containers.conf` `cgroup_conf` override instead (not repo-tracked — every
# contributor's machine differs), which podman applies to `memory.max`/`cpu.weight` unconditionally,
# overriding any value this script would pass anyway. `--cpus` (`cpu.max`, a separate quota
# mechanism `cgroup_conf` doesn't touch) still works and is kept. `--blkio-weight` was tried and
# reverted — this class of rootless-podman host doesn't delegate the `io` controller, so it breaks
# every container outright.
#
# Usage: scripts/cargo-container-run.sh <cargo-subcommand-and-args...>
#   e.g. scripts/cargo-container-run.sh cargo check -p lanrurugi-api
# Prefer `-p <crate>` over `--workspace` whenever possible — `--workspace` compiles every crate
# including ones unrelated to whatever you actually changed (this workspace's heaviest
# dependencies, `rav1e`/`bindgen`/`aws-lc-sys`, only come from `lanrurugi-scanner`/
# `lanrurugi-imgcompare`'s AVIF support). `--workspace` is for a final pre-push sanity check, not
# routine iteration.
#
# `CARGO_BUILD_JOBS=1` below is deliberate, not just "low" — parallelism itself (not just total
# memory) is what spikes the page-reclaim activity `systemd-oomd` watches; capping *memory* alone
# doesn't reduce how many rustc processes contend for pages at once. Spreading compilation out to
# one unit at a time trades wall-clock time for never producing that spike in the first place
# (2026-08-14: `systemd-oomd` killed both a build container *and* the VSCode window three times in
# 20 minutes at the previous jobs=6 setting, immediately after `.cargo-target`'s 20GB auto-clean
# forced a from-scratch `--workspace` compile).
#
# Optional env var (unset by default — a personal-machine tuning knob, not a repo default):
#   CARGO_CONTAINER_YIELD_TO=<comma-separated exact process names> — step down CPU budget/priority
#   while any of these processes are running. See that env var's own use below for details.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CMD="$(command -v podman || command -v docker || true)"
[ -n "$CMD" ] || { echo "error: podman/docker not found on PATH" >&2; exit 1; }

mkdir -p .cargo-target .cargo-registry

# Reap any stuck container from a *previous* invocation of this exact script whose own parent
# process died without it (e.g. the parent shell got killed by `systemd-oomd` mid-build) — a
# `--rm` container only self-removes on its own exit, not when whatever launched it dies, so one
# can silently keep running (and consuming CPU/memory) indefinitely with nothing left to notice or
# stop it. Confirmed live twice (2026-08-13, 2026-08-14): each was still `Up` for 11 minutes to
# ~19 hours after its parent died, and both times was still actively contributing to the very
# memory pressure that then went on to trip `systemd-oomd` again. Filtered by this script's own
# image (`lanrurugi-dev`, distinct from `lrr-dev`'s own `lanrurugi-dev-full` — the persistent dev
# server compose brings up, which must never be touched here) and by age (30 minutes — comfortably
# longer than any real `-p <crate>` build this script is meant for should ever take at jobs=1;
# older than that is almost certainly orphaned, not just slow).
REAPER_MAX_AGE_SECS=1800
now_epoch="$(date +%s)"
# `--format json`'s own `.Created` is a raw Unix-epoch int — unlike the Go-template `{{.Created}}`/
# `{{.CreatedAt}}` fields, which render a human date string podman's own locale/timezone
# formatting makes unreliable to re-parse with `date -d` (a trailing zone abbreviation like "JST"
# isn't accepted back by GNU date without first stripping it).
if command -v jq >/dev/null 2>&1; then
  while IFS=$'\t' read -r cid created_epoch; do
    [ -n "$cid" ] || continue
    age=$((now_epoch - created_epoch))
    if [ "$age" -gt "$REAPER_MAX_AGE_SECS" ]; then
      echo "note: reaping orphaned lanrurugi-dev container $cid (running ${age}s, likely orphaned by a killed parent shell)" >&2
      "$CMD" stop -t 5 "$cid" >/dev/null 2>&1 || true
    fi
  done < <("$CMD" ps --filter ancestor=localhost/lanrurugi-dev:latest --format json 2>/dev/null \
    | jq -r '.[] | "\(.Id)\t\(.Created)"')
fi

# 20GB soft cap on the persistent target cache — comfortably above one full workspace build's
# incremental footprint, well below the 86GB an earlier unbounded version reached. `cargo clean`,
# not deletion, so `.cargo-target`'s own directory structure stays intact.
size_kb="$(du -sk .cargo-target 2>/dev/null | cut -f1 || echo 0)"
if [ -n "$size_kb" ] && [ "$size_kb" -gt 20971520 ]; then
  echo "note: .cargo-target exceeded 20GB ($(du -sh .cargo-target | cut -f1)) — running cargo clean first" >&2
  "$CMD" run --rm -v "$REPO_ROOT":/workspace -v "$REPO_ROOT/.cargo-target":/workspace/target \
    -w /workspace lanrurugi-dev cargo clean
fi

# Foreground-app-aware step-down: if a process named in `$CARGO_CONTAINER_YIELD_TO` (comma-
# separated exact process names — a personal tuning knob, not a repo default) is running, cut the
# CPU quota and `CARGO_BUILD_JOBS`. `pgrep -x`, not `-f` — `-f` matches this script's own command
# line too and was observed self-matching. Example:
# `export CARGO_CONTAINER_YIELD_TO=dwproton` to step down while that Proton game is open.
cpus=4
cargo_jobs=1
IFS=',' read -ra YIELD_TO_PATTERNS <<< "${CARGO_CONTAINER_YIELD_TO:-}"
for pattern in "${YIELD_TO_PATTERNS[@]}"; do
  [ -n "$pattern" ] || continue
  if pgrep -x "$pattern" >/dev/null 2>&1; then
    echo "note: detected '$pattern' running (via \$CARGO_CONTAINER_YIELD_TO) — stepping down build resources (2 CPUs) so it keeps priority" >&2
    cpus=2
    cargo_jobs=1
    break
  fi
done

# Forward .env.local (gitignored, per-machine test config) into the container — it otherwise
# starts with none of the host shell's env vars. .env.test.local is a second, optional file kept
# separate for values containing spaces: mise's dotenv parser needs those quoted, but podman's
# `--env-file` parser doesn't strip quotes, so no single file/quoting style satisfies both readers.
ENV_FILE_ARGS=()
[ -f "$REPO_ROOT/.env.local" ] && ENV_FILE_ARGS+=(--env-file "$REPO_ROOT/.env.local")
[ -f "$REPO_ROOT/.env.test.local" ] && ENV_FILE_ARGS+=(--env-file "$REPO_ROOT/.env.test.local")

"$CMD" run --rm --network host --cpus="$cpus" \
  -v "$REPO_ROOT":/workspace \
  -v "$REPO_ROOT/.cargo-target":/workspace/target \
  -v "$REPO_ROOT/.cargo-registry":/usr/local/cargo/registry \
  -w /workspace -e CARGO_BUILD_JOBS="$cargo_jobs" "${ENV_FILE_ARGS[@]}" \
  lanrurugi-dev "$@"
