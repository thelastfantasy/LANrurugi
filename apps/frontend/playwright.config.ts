import { defineConfig, devices } from '@playwright/test'

// Layer 2 (spec User Story 1 Acceptance Scenario 4): real-browser end-to-end coverage against a
// real running backend + Redis (research.md §2-4). Chromium + Firefox per spec Clarifications
// (WebKit explicitly out of scope). retries/screenshot/trace policy per research.md §2.
//
// No static `webServer` entries: spec FR-014 requires genuine per-worker isolation, and (per
// research.md §4) a single backend process's Redis footprint is 5 fixed-offset logical databases,
// not 1 — so a single shared Redis instance can host at most 3 non-overlapping workers, an
// awkward and silently-breaking ceiling. Each worker instead starts its own Redis instance, own
// backend process, and own frontend preview server, all on ports derived from
// `testInfo.parallelIndex`, via the worker-scoped fixture in `tests/e2e/fixtures.ts`.
export default defineConfig({
  testDir: './tests/e2e',
  fullyParallel: true,
  retries: process.env.CI ? 1 : 0,
  workers: process.env.CI ? 4 : undefined,
  reporter: process.env.CI ? [['html'], ['github']] : 'html',
  globalTeardown: './tests/e2e/global-teardown.ts',
  use: {
    screenshot: 'only-on-failure',
    trace: process.env.CI ? 'on-first-retry' : 'retain-on-failure',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
    { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
  ],
})
