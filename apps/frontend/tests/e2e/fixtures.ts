import { ChildProcess, spawn } from "node:child_process"
import { execSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

import { test as base } from "@playwright/test"

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const FRONTEND_ROOT = path.resolve(__dirname, "../..")
const REPO_ROOT = path.resolve(FRONTEND_ROOT, "../..")

// Per-worker isolation (spec FR-014): each worker gets its own Redis *instance* (own port), own
// backend process, and own frontend preview server — not a single shared Redis instance
// partitioned by logical database number. That simpler-sounding approach was tried first and
// rejected: `RedisDbs::connect` (crates/lanrurugi-storage/src/redis.rs) takes one bare base URL and
// internally opens FIVE fixed-offset logical databases per backend process (archive/minion/config/
// search/metrics), not one — against Redis's default 16-DB ceiling that allows at most 3
// non-overlapping workers, and was only discovered by starting a real backend process and hitting
// "Invalid database number" (research.md §4 documents this in full). A separate Redis instance per
// worker sidesteps the DB-count ceiling entirely.
//
// Keyed by `testInfo.parallelIndex` (stable, bounded 0..workers-1) — NOT `workerIndex` (unbounded,
// changes across worker-process restarts within a run).
// `proc`/`logPath` (both optional — only the backend process passes them) let this fail fast and
// specific instead of always burning the full timeout: previously, a backend that crashed/panicked
// immediately on startup (e.g. a malformed policy file, a `.expect()` firing) surfaced only as a
// generic "did not become healthy within 30000ms" with zero indication *why* — confirmed live,
// 2026-08-27, when a real CI run's every single E2E test failed this way and the actual cause
// (a `cargo test`-only Redis-key collision unrelated to `serve` itself, as it turned out) took
// significant investigation to rule in/out purely because this fixture gave no diagnostic signal
// of its own. If the process has already exited by the time this notices, this reads the process's
// own stdout/stderr (redirected to `logPath` by the caller, since `spawnTracked`'s default
// `stdio: 'ignore'` swallows it) and throws immediately with that content inlined, rather than
// polling `fetch` uselessly against a process that's already gone.
async function waitForHealthy(url: string, timeoutMs = 30_000, proc?: ChildProcess, logPath?: string) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    if (proc && proc.exitCode !== null) {
      const log = logPath && fs.existsSync(logPath) ? fs.readFileSync(logPath, "utf8") : "(no log captured)"
      throw new Error(`process for ${url} exited early with code ${proc.exitCode}:\n${log}`)
    }
    try {
      const res = await fetch(url)
      if (res.ok) return
    } catch {
      // not up yet
    }
    if (Date.now() > deadline) {
      const log = logPath && fs.existsSync(logPath) ? fs.readFileSync(logPath, "utf8") : "(no log captured)"
      throw new Error(`${url} did not become healthy within ${timeoutMs}ms; process output so far:\n${log}`)
    }
    await new Promise((r) => setTimeout(r, 200))
  }
}

// Tracks every spawned child across this worker process's lifetime so an explicit `process.on`
// safety net (below) can reap them even if the worker is force-killed (Ctrl+C, a crashed test)
// before the fixture's own teardown code runs — without this, `spawn()`'d processes silently
// outlive the worker that started them, and a *later* run's fixture then reuses the same ports/
// library directories as a still-running, already-populated leftover process (this exact failure
// mode was hit during implementation: a stale worker-0 backend from a previous run kept answering
// on port 3100 with already-uploaded fixture content, making later uploads of the same fixture
// fail with a spurious "already exists" 409).
const spawnedChildren = new Set<ChildProcess>()

function killAllSpawnedChildren() {
  for (const proc of spawnedChildren) {
    if (!proc.killed) proc.kill("SIGKILL")
  }
}
process.on("exit", killAllSpawnedChildren)
process.on("SIGINT", () => {
  killAllSpawnedChildren()
  process.exit(130)
})
process.on("SIGTERM", () => {
  killAllSpawnedChildren()
  process.exit(143)
})

// `logPath`, when given, redirects stdout+stderr into that file (append mode — `logPath` itself is
// truncated by the caller first, e.g. `fs.rmSync` on a stale one) instead of the default
// `stdio: 'ignore'` — see `waitForHealthy`'s own docs for why: a process that panics/exits
// immediately on startup otherwise leaves zero trace of *why* anywhere in CI output.
function spawnTracked(command: string, args: string[], opts: Parameters<typeof spawn>[2], logPath?: string): ChildProcess {
  const stdio = logPath
    ? (["ignore", fs.openSync(logPath, "a"), fs.openSync(logPath, "a")] as const)
    : ("ignore" as const)
  const proc = spawn(command, args, { ...opts, stdio })
  spawnedChildren.add(proc)
  proc.once("exit", () => spawnedChildren.delete(proc))
  // `spawn` failing outright (command not found, exec permission denied, etc.) fires an 'error'
  // event asynchronously rather than throwing — with no listener, Node treats that as an uncaught
  // exception, and this was previously silently swallowed by `stdio: 'ignore'` leaving no trace at
  // all in CI logs (a real failure here surfaced only as a generic 30s Playwright fixture-setup
  // timeout with zero indication of which of the three spawned processes never actually started).
  proc.on("error", (err) => {
    console.error(`[fixtures.ts] failed to spawn "${command}": ${err.message}`)
  })
  return proc
}

// Best-effort: if a previous run's process on this exact port was orphaned (e.g. the whole worker
// was SIGKILLed before even the safety net above could run), refuse to silently reuse it — kill
// whatever's listening there first, so this run never talks to stale state from a different run.
//
// `fuser` (the obvious choice) is NOT installed on either this project's local dev container or
// GitHub's own `ubuntu-latest` runner image (confirmed directly against both — neither ships
// `psmisc`), so `fuser -k` silently failed with "command not found" every single time, and the
// surrounding try/catch swallowed that, meaning this cleanup step has never actually run. Uses
// `/proc/net/tcp{,6}` + `/proc/<pid>/fd` directly instead — no external tool dependency at all,
// works on any Linux without needing anything beyond what's already guaranteed to exist.
function killWhateverIsListeningOn(port: number) {
  const hexPort = port.toString(16).toUpperCase().padStart(4, "0")
  const inodes = new Set<string>()
  for (const procFile of ["/proc/net/tcp", "/proc/net/tcp6"]) {
    let contents: string
    try {
      contents = fs.readFileSync(procFile, "utf8")
    } catch {
      continue
    }
    for (const line of contents.split("\n").slice(1)) {
      const fields = line.trim().split(/\s+/)
      const localAddress = fields[1]
      const inode = fields[9]
      if (!localAddress || !inode || inode === "0") continue
      const [, portHex] = localAddress.split(":")
      if (portHex === hexPort) inodes.add(inode)
    }
  }
  if (inodes.size === 0) return

  for (const pidDir of fs.readdirSync("/proc").filter((name) => /^\d+$/.test(name))) {
    const fdDir = `/proc/${pidDir}/fd`
    let fds: string[]
    try {
      fds = fs.readdirSync(fdDir)
    } catch {
      continue // process exited, or we don't have permission — skip
    }
    for (const fd of fds) {
      let link: string
      try {
        link = fs.readlinkSync(`${fdDir}/${fd}`)
      } catch {
        continue
      }
      const match = /^socket:\[(\d+)\]$/.exec(link)
      if (match && inodes.has(match[1])) {
        try {
          process.kill(Number(pidDir), "SIGKILL")
        } catch {
          // already gone — fine
        }
        break
      }
    }
  }
}

function fsRmSyncQuiet(dir: string) {
  fs.rmSync(dir, { recursive: true, force: true })
}

export const test = base.extend<object, { workerBaseURL: string }>({
  workerBaseURL: [
    // eslint-disable-next-line no-empty-pattern
    async ({}, use, workerInfo) => {
      const redisPort = 6390 + workerInfo.parallelIndex
      const backendPort = 3100 + workerInfo.parallelIndex
      const frontendPort = 5200 + workerInfo.parallelIndex
      const libraryDir = path.join(REPO_ROOT, `.e2e-worker-${workerInfo.parallelIndex}`)

      for (const port of [redisPort, backendPort, frontendPort]) killWhateverIsListeningOn(port)
      fsRmSyncQuiet(libraryDir)

      // `redis-server` persists to `dump.rdb` in its CWD by default and *reloads it on startup* —
      // running every worker's instance from a shared `cwd` (e.g. the repo root) means every
      // "fresh" Redis instance actually loads the same on-disk snapshot regardless of port,
      // silently defeating per-worker (and per-run) isolation entirely (this was caught by
      // directly inspecting a supposedly-brand-new instance's keys during implementation — it
      // already had 4 archives from earlier runs). `--save ""` disables persistence outright
      // (these are throwaway test instances; durability across a restart is never needed), and
      // each worker additionally gets its own CWD as a second independent safeguard.
      const redisDir = path.join(REPO_ROOT, `.e2e-worker-${workerInfo.parallelIndex}`, "redis")
      fs.mkdirSync(redisDir, { recursive: true })
      const redis = spawnTracked(
        "redis-server",
        ["--port", String(redisPort), "--daemonize", "no", "--save", ""],
        { cwd: redisDir },
      )
      // A fixed 300ms sleep before the first PING worked reliably in local testing but was too
      // optimistic for a real CI runner (confirmed: this exact spot is where CI first started
      // failing — a single un-retried `execSync` PING threw an uncaught ECONNREFUSED that
      // Playwright only ever surfaced as a generic 30s fixture-setup timeout, not the real
      // connection-refused error). Poll instead of sleeping a fixed guess.
      {
        const deadline = Date.now() + 10_000
        for (;;) {
          try {
            execSync(`redis-cli -p ${redisPort} PING`, { stdio: "ignore" })
            break
          } catch {
            if (Date.now() > deadline) {
              throw new Error(`redis-server on port ${redisPort} did not respond to PING within 10s`)
            }
            await new Promise((resolve) => setTimeout(resolve, 100))
          }
        }
      }
      execSync(`redis-cli -p ${redisPort} FLUSHALL`, { stdio: "ignore" })

      fs.mkdirSync(libraryDir, { recursive: true })
      const backendLogPath = path.join(libraryDir, "backend.log")
      const backend = spawnTracked(
        path.join(REPO_ROOT, "target/debug/lanrurugi-server"),
        [
          "serve",
          "--redis-url",
          `redis://127.0.0.1:${redisPort}`,
          "--library-path",
          path.join(libraryDir, "library"),
          "--thumb-dir",
          path.join(libraryDir, "thumb"),
          "--temp-dir",
          path.join(libraryDir, "temp"),
          "--bind",
          `127.0.0.1:${backendPort}`,
        ],
        { cwd: REPO_ROOT },
        backendLogPath,
      )
      // /api/login/status is merged in before the auth middleware (crates/lanrurugi-server/src/
      // app.rs), so it's reachable without a session — unlike /api/info, which is auth-protected.
      await waitForHealthy(`http://127.0.0.1:${backendPort}/api/login/status`, 30_000, backend, backendLogPath)

      // `pnpm exec vite preview` directly, not `pnpm run preview -- ...` — the latter's argument
      // passthrough silently dropped `--strictPort` (confirmed by direct comparison: `pnpm run
      // preview -- --port 5299 --strictPort` fell through to Vite's own "port in use, trying
      // another one" fallback-port behavior instead of failing, so the port this fixture computed
      // and the port Vite actually bound could silently diverge).
      //
      // `--host 127.0.0.1` (not the default, which resolves "localhost" to whichever address
      // family the OS prefers) matters here for a second, independently-confirmed reason:
      // `--strictPort` itself does NOT strictly enforce port exclusivity the way its name implies
      // — verified directly by starting a listener on a port first, then launching `vite preview
      // --port <same> --strictPort` against it: Vite printed "Port ... is in use on a wildcard
      // address, but localhost:... is available" and bound successfully anyway, rather than
      // erroring. Under repeated worker restarts (e.g. a flaky CI runner triggering retries),
      // this let a `vite preview` process silently coexist on a port a previous, still-orphaned
      // instance already held on a different address scope — the new instance never actually
      // became reachable at the exact `127.0.0.1:<port>` address this fixture's own
      // `waitForHealthy` probes, hanging until the 30s Playwright fixture-setup timeout with zero
      // indication of why. Binding explicitly to `127.0.0.1` removes the address-scope ambiguity
      // `--strictPort` alone doesn't close.
      const frontend = spawnTracked(
        "pnpm",
        ["exec", "vite", "preview", "--port", String(frontendPort), "--strictPort", "--host", "127.0.0.1"],
        {
          cwd: FRONTEND_ROOT,
          env: { ...process.env, LANRURUGI_E2E_BACKEND_PORT: String(backendPort) },
        },
      )
      const baseURL = `http://127.0.0.1:${frontendPort}`
      await waitForHealthy(baseURL)

      await use(baseURL)

      frontend.kill("SIGKILL")
      backend.kill("SIGKILL")
      redis.kill("SIGKILL")
    },
    { scope: "worker", auto: true },
  ],

  baseURL: async ({ workerBaseURL }, use) => {
    await use(workerBaseURL)
  },
})

export { expect } from "@playwright/test"
