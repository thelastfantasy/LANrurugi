#!/usr/bin/env bash
# Runs a cargo subcommand inside the `lanrurugi-dev` container with the shared guardrails every
# `mise run {test,clippy,fmt-check,build}` task uses — memory cap, capped parallelism, persistent
# target/registry cache, and an optional foreground-app-aware resource step-down. Added after a
# real host crash (2026-07-21): an unbounded `--rm` container running `cargo build` on a
# 32-core/28GB-RAM machine, hitting `rav1e`'s heavy codegen via the `image` crate's AVIF support,
# plus a second concurrent build container, drove the system into OOM and a hard reboot (confirmed
# via `journalctl`'s last pre-crash line being "Under memory pressure, flushing caches").
#
# Usage: scripts/cargo-container-run.sh <cargo-subcommand-and-args...>
#   e.g. scripts/cargo-container-run.sh cargo test --workspace
# Optional env var (unset by default — a personal-machine tuning knob, not a repo default):
#   CARGO_CONTAINER_YIELD_TO=<comma-separated exact process names> — step down CPU budget/priority
#   while any of these processes are running. See that env var's own use below for details.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CMD="$(command -v podman || command -v docker || true)"
[ -n "$CMD" ] || { echo "error: podman/docker not found on PATH" >&2; exit 1; }

mkdir -p .cargo-target .cargo-registry

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
# separated exact process names — no default; this is a personal-machine tuning knob, not
# something an open-source repo should hardcode a contributor's own game/app choices into) is
# running, halve the CPU budget and drop the container's scheduling weight (`--cpu-shares`,
# relative to the default 1024) so that process keeps priority under contention, rather than just
# hoping `nproc`-wide parallelism doesn't starve it. Matched by exact process-name (`pgrep -x`,
# comparing against `/proc/<pid>/comm`), deliberately NOT `pgrep -f` — that flag matches the full
# command line of *every* process, including this very script's own invocation (whose shell
# wrapper command text can itself legitimately contain the same text being searched for, e.g.
# while being edited/tested), which was observed self-matching and producing false positives
# during development. Example: `export CARGO_CONTAINER_YIELD_TO=dwproton` in your own shell
# profile to step down while Wuthering Waves (run via the `dwproton` Proton build) is open.
cpus=16
cpu_shares=1024
cargo_jobs=8
IFS=',' read -ra YIELD_TO_PATTERNS <<< "${CARGO_CONTAINER_YIELD_TO:-}"
for pattern in "${YIELD_TO_PATTERNS[@]}"; do
  [ -n "$pattern" ] || continue
  if pgrep -x "$pattern" >/dev/null 2>&1; then
    echo "note: detected '$pattern' running (via \$CARGO_CONTAINER_YIELD_TO) — stepping down build resources (4 CPUs, low priority) so it keeps priority" >&2
    cpus=4
    cpu_shares=256
    cargo_jobs=4
    break
  fi
done

# Forward .env.local (gitignored, per-machine — e.g. TEST_REAL_DOWNLOAD_URL/
# TEST_REAL_DOWNLOAD_EXPECTED_FILENAME for stream.rs's opt-in real-server test) into the
# container's environment, since the container otherwise starts with none of the host shell's
# env vars. `--env-file` accepts the same KEY=VALUE format .env.local already uses.
ENV_FILE_ARGS=()
[ -f "$REPO_ROOT/.env.local" ] && ENV_FILE_ARGS=(--env-file "$REPO_ROOT/.env.local")

"$CMD" run --rm --network host --memory=8g --memory-swap=8g --cpus="$cpus" --cpu-shares="$cpu_shares" \
  -v "$REPO_ROOT":/workspace \
  -v "$REPO_ROOT/.cargo-target":/workspace/target \
  -v "$REPO_ROOT/.cargo-registry":/usr/local/cargo/registry \
  -w /workspace -e CARGO_BUILD_JOBS="$cargo_jobs" "${ENV_FILE_ARGS[@]}" \
  lanrurugi-dev "$@"
