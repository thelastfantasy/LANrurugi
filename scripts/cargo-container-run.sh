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
# Usage: always through a `mise run` task — never invoked bare (enforced below via
# `MISE_TASK_NAME`). See `.mise.toml`'s `[tasks.test]`/`[tasks.clippy]`/`[tasks.fmt-check]`/
# `[tasks.fmt]`/`[tasks.build]`/`[tasks.check-crate]` for the fixed set of ways to call this;
# `mise run check-crate -- <crate> [<crate> ...]` is the one for ad-hoc single-crate iteration
# (`scripts/cargo-container-run.sh cargo check -p lanrurugi-api`'s old direct-call equivalent).
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

# Must be invoked through a `mise run <task>` (never called bare) — every guardrail below this
# point (PSI check, cooldown, reaper, cache cap) only helps if this script is actually the one
# path everyone (including an agent working in this repo) goes through; a bare
# `scripts/cargo-container-run.sh cargo ...` call in some ad-hoc shell session bypasses all of it
# just as easily as a raw host-side `cargo` would. `mise run` always sets `MISE_TASK_NAME` in the
# child process's environment (confirmed live via `env | grep -i mise` inside a real task run) —
# checking for it costs nothing and catches every real invocation shape:
#   - `mise run test` / `mise run clippy` / `mise run fmt-check` / `mise run fmt` / `mise run build`
#     (the fixed-subcommand tasks in `.mise.toml`)
#   - `mise run check-crate -- lanrurugi-api lanrurugi-storage` (the variadic `-p`-per-arg task)
#   - `mise run` invoked transitively from `mise run check` (→ `npx lefthook run pre-push`, which
#     itself shells out to the `rust-check`/`frontend-lint` lefthook jobs — but those jobs call
#     `mise run clippy`/`mise run fmt-check` internally, not this script directly, so the
#     environment variable is still set by the innermost `mise run`)
#   - any *new* task added to `.mise.toml` in the future that runs this script — nothing here is
#     tied to a specific task name, only to having gone through `mise run` at all
if [ -z "${MISE_TASK_NAME:-}" ]; then
  echo "error: scripts/cargo-container-run.sh must be run via 'mise run <task>', not called directly." >&2
  echo "       Use one of: mise run test | mise run clippy | mise run fmt-check | mise run fmt |" >&2
  echo "                   mise run build | mise run check-crate -- <crate> [<crate> ...]" >&2
  echo "       (all defined in .mise.toml — add a new task there instead of calling this script bare)" >&2
  exit 1
fi

CMD="$(command -v podman || command -v docker || true)"
[ -n "$CMD" ] || { echo "error: podman/docker not found on PATH" >&2; exit 1; }

# Serializes every invocation of this script — real concern, not theoretical: confirmed live
# (2026-08-14) that issuing several of these back-to-back (`check`, `clippy` in the background,
# `fmt-check`, `fmt`) let more than one actually overlap in the container runtime at once, each
# individually passing the PSI check below (a snapshot at its own start) while the *combined*
# concurrent memory usage still tripped `systemd-oomd` and killed the VSCode window. A cooldown
# after each run (further below) prevents back-to-back-but-sequential calls from piling on, but
# only `flock` actually prevents true overlap — two invocations racing to pass the PSI check within
# the same instant, before either one's own container has started consuming memory yet. Blocks
# (not fails) until free — a legitimate queue of calls (e.g. this script invoked from two different
# terminals) should still all eventually run, just never at the same time; `flock`'s own blocking
# wait is what the cooldown's explicit `sleep` message is modeled after. The lock file lives under
# `.cargo-target` (gitignored, already this repo's scratch dir for this script) rather than `/tmp`,
# so it doesn't collide with a *different* clone of this repo on the same machine.
mkdir -p "$REPO_ROOT/.cargo-target"
exec 9>"$REPO_ROOT/.cargo-target/.container-run.lock"
if ! flock -n 9; then
  echo "note: another cargo-container-run.sh invocation is in progress — waiting for it to finish (never running two at once)..." >&2
  flock 9
fi

# Refuses to start a new build while the host is already under real memory pressure — every
# guardrail above this point only caps what *this one invocation* can consume, which does nothing
# if the system was already close to the edge before this script even ran. Confirmed live
# (2026-08-14): a `cargo test -p lanrurugi-api -p lanrurugi-server` was launched right after
# `systemd-oomd` had just killed an orphaned container for memory pressure — the host never
# recovered, and six minutes later `systemd-oomd` killed the VSCode window itself. Reads the same
# PSI (Pressure Stall Information) metric `systemd-oomd` itself watches
# (`/proc/pressure/memory`'s `avg60`, a percentage of the last 60s spent with at least one task
# stalled on memory) rather than a raw free-memory number, since free memory alone doesn't capture
# reclaim *activity* — the actual thing that trips `systemd-oomd`'s own threshold (its default
# config kills at 50% sustained pressure, see `journalctl -u systemd-oomd`).
#
# Deliberately fails outright with a message, not a sleep-and-retry loop — silently blocking makes
# a single command's wall-clock time unpredictable and hides the real signal (the host needs
# something to actually finish or be closed) behind a script that just looks "slow" instead.
PSI_AVG60_THRESHOLD=20
if [ -r /proc/pressure/memory ]; then
  avg60="$(awk -F'avg60=' '/^some/ {split($2,a," "); print a[1]}' /proc/pressure/memory)"
  if [ -n "$avg60" ] && awk -v v="$avg60" -v t="$PSI_AVG60_THRESHOLD" 'BEGIN{exit !(v>t)}'; then
    echo "error: host memory pressure too high to start a new build (avg60=${avg60}%, threshold=${PSI_AVG60_THRESHOLD}%)." >&2
    echo "       Close some memory-heavy apps or wait for current load to settle, then retry." >&2
    echo "       (see /proc/pressure/memory, or 'journalctl -u systemd-oomd' for recent kills)" >&2
    exit 1
  fi
fi

# A single-snapshot PSI check (above) is not enough on its own — confirmed live (2026-08-14):
# several back-to-back invocations (`check`, `clippy`, `fmt-check`, `fmt`) each individually passed
# the `avg60` check, since `avg60` is a *trailing* 60s average that hadn't yet caught up to the
# pressure the previous invocation was still causing — but the cumulative effect still tripped
# `systemd-oomd` and killed the VSCode window six minutes after the first of that sequence started.
# This cooldown forces real wall-clock time between invocations so pressure actually has a chance
# to settle before the next one starts, rather than four checks in a row each seeing a stale,
# not-yet-updated `avg60`. Stamp file, not an in-shell variable — this script is invoked fresh by a
# new process every time, so nothing else could remember "when did the last invocation finish."
COOLDOWN_SECS=15
COOLDOWN_STAMP="$REPO_ROOT/.cargo-target/.last-container-run"
if [ -f "$COOLDOWN_STAMP" ]; then
  last_run="$(cat "$COOLDOWN_STAMP" 2>/dev/null || echo 0)"
  now_epoch_cooldown="$(date +%s)"
  elapsed=$((now_epoch_cooldown - last_run))
  if [ "$elapsed" -lt "$COOLDOWN_SECS" ]; then
    wait_secs=$((COOLDOWN_SECS - elapsed))
    echo "note: another container run finished ${elapsed}s ago — waiting ${wait_secs}s for memory pressure to settle before starting this one" >&2
    sleep "$wait_secs"
  fi
fi

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

# A large fraction of this workspace's own unit tests (`lanrurugi-storage`/`lanrurugi-search`/
# `lanrurugi-backup`/`lanrurugi-api`/... — any test hitting a real `RedisDbs::connect`) need a real
# Redis reachable at `redis://127.0.0.1:6379`, the same hardcoded default `crates/lanrurugi-storage/
# src/redis.rs` itself uses. This container runs with `--network host`, so "reachable at 6379" just
# means *something* is listening on the host's own loopback at that port — nothing here requires
# it to be this specific container. Confirmed live (2026-08-24): before this existed, `mise run
# test` had been silently reporting only its first-encountered failures for a long time — cargo's
# default fail-fast behavior stopped the whole `--workspace` run at the very first failing test
# binary, and since `lanrurugi-api` (which has a couple of Redis-dependent tests) sorts before
# most other Redis-dependent crates in the workspace member list, dozens to low hundreds of
# further Redis-dependent failures across other crates had never actually been seen, only ever
# assumed to be "the same known two" — `--no-fail-fast` (`.mise.toml`'s own `[tasks.test]`) is what
# surfaced the real scope of this for the first time.
REDIS_TEST_CONTAINER=""
if ! (exec 3<>"/dev/tcp/127.0.0.1/6379") 2>/dev/null; then
  echo "note: no Redis reachable at 127.0.0.1:6379 — starting a temporary one for this test run" >&2
  REDIS_TEST_CONTAINER="$("$CMD" run -d --rm --network host \
    "docker.io/library/redis:7.4.9-bookworm" redis-server --save "" --appendonly no)"
  # Give it a moment to actually start accepting connections — a fixed short sleep is fine here
  # (not the PSI-style polling loop used elsewhere in this script): redis-server binds its listen
  # socket within milliseconds of starting, this isn't waiting on anything resource-dependent.
  for _ in $(seq 1 20); do
    (exec 3<>"/dev/tcp/127.0.0.1/6379") 2>/dev/null && break
    sleep 0.5
  done
else
  echo "note: reusing an already-listening Redis at 127.0.0.1:6379 for this test run" >&2
fi

# Stamped on exit regardless of success/failure (`trap ... EXIT`, not just appended after the run
# below) — the cooldown at the top of this script must see "a container just ran" even when that
# run failed or was interrupted, otherwise a failing command retried immediately (a very real
# pattern: fix a compile error, rerun right away) would skip the cooldown entirely on exactly the
# retry that matters most. Combined with the temporary Redis container's own teardown (only ever
# torn down here if *this* script started it — an already-running Redis found above is left alone,
# since this script didn't start it and has no business stopping someone else's service) in the
# same trap, since bash only honors the most recently registered handler for a given signal.
trap '[ -n "$REDIS_TEST_CONTAINER" ] && "$CMD" stop -t 5 "$REDIS_TEST_CONTAINER" >/dev/null 2>&1; date +%s > "$COOLDOWN_STAMP"' EXIT

"$CMD" run --rm --network host --cpus="$cpus" \
  -v "$REPO_ROOT":/workspace \
  -v "$REPO_ROOT/.cargo-target":/workspace/target \
  -v "$REPO_ROOT/.cargo-registry":/usr/local/cargo/registry \
  -w /workspace -e CARGO_BUILD_JOBS="$cargo_jobs" "${ENV_FILE_ARGS[@]}" \
  lanrurugi-dev "$@"
