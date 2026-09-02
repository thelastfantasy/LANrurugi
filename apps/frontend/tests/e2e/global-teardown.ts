import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const REPO_ROOT = path.resolve(__dirname, "../../../..")

// Implements spec FR-011's "clean, known state before the next run begins" by default. Skips the
// actual cleanup (but still runs, so the process doesn't error) when `process.env.KEEP` is set —
// the spec's Edge Case allowance for deliberately inspecting a failed run's environment afterward.
// The *next* run must still clean up normally regardless of whether this one was kept, so `KEEP`
// only ever affects the run it's set for and never persists as a mode.
//
// Each worker's own Redis/backend/frontend processes are already killed by
// tests/e2e/fixtures.ts's own worker-scoped teardown; this global step only removes the
// throwaway per-worker library/thumb/temp directories those processes wrote to.
export default async function globalTeardown() {
  if (process.env.KEEP) {
    console.log("KEEP is set — skipping teardown, leaving this run's environment for inspection.")
    return
  }

  const entries = fs.readdirSync(REPO_ROOT).filter((name) => name.startsWith(".e2e-worker-"))
  for (const entry of entries) {
    fs.rmSync(path.join(REPO_ROOT, entry), { recursive: true, force: true })
  }
}
