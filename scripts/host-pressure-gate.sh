#!/usr/bin/env bash
# Refuses to start a heavy build while the host is already under real memory pressure.
#
# Shared by *both* paths that compile this workspace: `cargo-container-run.sh` (the guardrailed
# cargo runs) and the `dev-rebuild`/`dev-rebuild-auto` tasks (the dev *image* build). The image
# build was the hole this extraction closes — `podman build` runs `cargo build --release` with
# cargo's own default parallelism (every core) and bypassed every guardrail the cargo path has, so
# on 2026-10-10 it drove the user slice to 72% pressure and `systemd-oomd` killed a Firefox scope
# while an agent had launched it unattended.
#
# Reads the same PSI (Pressure Stall Information) metric `systemd-oomd` itself watches
# (`/proc/pressure/memory`'s `avg60`, the percentage of the last 60s spent with at least one task
# stalled on memory) rather than a raw free-memory number, since free memory alone doesn't capture
# reclaim *activity* — the actual thing that trips `systemd-oomd`'s own threshold (its default
# config kills at 50% sustained pressure; see `journalctl -u systemd-oomd` for recent kills). The
# default threshold sits well below that.
#
# Deliberately fails outright with a message, not a sleep-and-retry loop — silently blocking makes a
# command's wall-clock time unpredictable and hides the real signal (the host needs something to
# actually finish or be closed) behind a script that just looks "slow" instead.
#
# Override the threshold with `PSI_AVG60_THRESHOLD=<percent>` (a per-machine tuning knob).
set -euo pipefail

PSI_AVG60_THRESHOLD="${PSI_AVG60_THRESHOLD:-20}"

if [ -r /proc/pressure/memory ]; then
  avg60="$(awk -F'avg60=' '/^some/ {split($2,a," "); print a[1]}' /proc/pressure/memory)"
  if [ -n "$avg60" ] && awk -v v="$avg60" -v t="$PSI_AVG60_THRESHOLD" 'BEGIN{exit !(v>t)}'; then
    echo "error: host memory pressure too high to start a new build (avg60=${avg60}%, threshold=${PSI_AVG60_THRESHOLD}%)." >&2
    echo "       Close some memory-heavy apps or wait for current load to settle, then retry." >&2
    echo "       (see /proc/pressure/memory, or 'journalctl -u systemd-oomd' for recent kills)" >&2
    exit 1
  fi
fi
