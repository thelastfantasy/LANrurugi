import http from "node:http"

import { expect, test } from "./fixtures"
import { MockLlmServer, respondContent } from "./mockLlmServer"

// T047 (US1-US3/US6 full journey) — lookup → select → generate → trial-run → save, against a
// local fixture site with a stable, known page structure and a mocked LLM backend (plan.md's
// Testing note: no live external site or real LLM key dependency in CI).
//
// `LANRURUGI_DEEPSEEK_BASE_URL` must be set on *this process's own env* before `fixtures.ts`'s
// worker-scoped backend-spawn fixture runs (it inherits `process.env` by default, since
// `fixtures.ts` passes no explicit `env` override for the backend spawn) — top-level module code
// in a Playwright spec file runs before any fixture is instantiated for that worker, so setting it
// here at import time is early enough.
//
// MOCK_LLM_PORT's value must stay identical to plugin-wizard-login-detection.spec.ts's own — see
// that file's own doc comment on why: the backend process is spawned once per worker and lives for
// every test that worker runs, so whichever spec file's top-level assignment happens to execute
// last inside a shared worker silently wins for the rest of that worker's lifetime; a mismatched
// port here previously sent this file's own generate calls to the *other* file's mock server port,
// where nothing was listening (confirmed live, 2026-08-26).
const MOCK_LLM_PORT = 6410 + Number(process.env.TEST_PARALLEL_INDEX ?? 0)
const FIXTURE_SITE_PORT = 6420 + Number(process.env.TEST_PARALLEL_INDEX ?? 0)
process.env.LANRURUGI_DEEPSEEK_BASE_URL = `http://127.0.0.1:${MOCK_LLM_PORT}`

/** A tiny, fixed-structure "target site" the generated plugin's real trial run fetches against —
 * real network I/O (localhost), not mocked at the plugin-execution layer, since the whole point of
 * a trial run is exercising the actual Deno sandbox + a real `fetch()` call. */
function buildFixtureSite(): http.Server {
  return http.createServer((req, res) => {
    if (req.url?.startsWith("/work/")) {
      res.writeHead(200, { "Content-Type": "text/html" });
      res.end(`<html><body><h1 id="title">Fixture Work ${req.url.slice(6)}</h1></body></html>`)
      return
    }
    res.writeHead(404)
    res.end("not found")
  })
}

/** A minimal, real metadata plugin — this stands in for what the mocked LLM "generates": valid
 * `.ts` source the real Deno sandbox loads and executes for real against `buildFixtureSite()`
 * above, extracting the title out of the fixture page's own known markup. */
function generatedMetadataPluginSource(fixtureBase: string): string {
  return `
export function pluginInfo() {
  return {
    namespace: "wizard-e2e-fixture",
    type: "metadata",
    parameters: [],
    declared_permissions: { net: ["127.0.0.1"], read: false, write: false },
    generated_by_wizard: true,
    name: "Wizard E2E Fixture Plugin",
    author: "e2e",
    description: "Generated for plugin-wizard.spec.ts",
    version: "1.0.0",
  };
}

export async function execMetadata(hostArgs) {
  const resp = await fetch(hostArgs.url);
  const html = await resp.text();
  const match = html.match(/<h1 id="title">([^<]*)<\\/h1>/);
  return { title: match ? match[1] : "unknown", tags: "source:wizard-e2e" };
}
`.replace(/127\.0\.0\.1/g, new URL(fixtureBase).hostname)
}

test.describe("plugin wizard", { tag: "@plugin-wizard" }, () => {
  let mockLlm: MockLlmServer
  let fixtureSite: http.Server
  let fixtureBase: string

  test.beforeEach(async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
    // `lanrurugi_llm::resolve_api_key` checks Redis-persisted `llm_api_key` *before* ever falling
    // back to the `DEEPSEEK_API_KEY` env var this file points at the mock server — with neither
    // set, generation fails immediately with "API key not configured" before any HTTP request is
    // even attempted, which previously looked exactly like a hung/silent failure (no request ever
    // reached `MockLlmServer`, confirmed live by inheriting the backend's own stdio during CI
    // diagnosis, 2026-08-26). The mock server never validates the key's actual value, so any
    // non-empty string satisfies this check.
    await page.request.put("/api/settings", { data: { llm_api_key: "mock-test-key" } })

    mockLlm = new MockLlmServer()
    await mockLlm.listen(MOCK_LLM_PORT)

    fixtureSite = buildFixtureSite()
    await new Promise<void>((resolve) => fixtureSite.listen(FIXTURE_SITE_PORT, "127.0.0.1", resolve))
    fixtureBase = `http://127.0.0.1:${FIXTURE_SITE_PORT}`
  })

  test.afterEach(async () => {
    await mockLlm.close()
    await new Promise<void>((resolve) => fixtureSite.close(() => resolve()))
  })

  test("full journey: lookup, generate, trial-run, and save a metadata plugin", async ({ page }) => {
    mockLlm.enqueue(respondContent(generatedMetadataPluginSource(fixtureBase)))

    await page.goto("/config/plugins/wizard")

    // Step 1 "查找域名": domain lookup for a domain with no existing coverage.
    await page.locator('input[type="text"]').first().fill("wizard-e2e-fixture.invalid")
    await page.getByRole("button", { name: "Look up" }).click()
    await expect(page.getByText("Select which plugin types to create")).toBeVisible()

    // Step 2 "选择类型": select "metadata" as the target type via its checkbox (no `.checklist`
    // class on this list — see `TypeSelectionStep.tsx`'s own doc comment on why it was dropped).
    await page.getByRole("checkbox", { name: "Metadata" }).check()
    await page.getByRole("button", { name: "Next", exact: true }).click()

    // Step 3 "共享链接": one domain-level textarea, not one link input per type — every selected
    // type's generate/trial-run reads from this same shared list (`SharedLinksForm.tsx`).
    await page
      .getByLabel(/^Links \(one per line/)
      .fill([`${fixtureBase}/work/1`, `${fixtureBase}/work/2`, `${fixtureBase}/work/3`].join("\n"))
    await page.getByRole("button", { name: "Next", exact: true }).click()

    // Step 4 "生成与保存": only "metadata" was selected, so its own `TypeWizardPanel` is the only
    // one mounted — no tab-switch needed for a single-type run.
    // US2: generate — the mocked LLM returns the fixture plugin source above.
    await page.getByRole("button", { name: "Generate", exact: true }).click()
    // "Trial run" only renders once a DraftRevision exists — its appearance is itself evidence
    // generation succeeded and produced an active revision.
    await expect(page.getByRole("button", { name: "Trial run" })).toBeEnabled({ timeout: 10_000 })

    // US3: trial-run against all three fixture links — one `.ptbox` for this trial-run round,
    // containing an independent success/failure line per link (AC4 — no single pass/fail verdict
    // masking individual link outcomes); expect all three lines to say "Success".
    await page.getByRole("button", { name: "Trial run" }).click()
    const trialRunBox = page.locator(".ptbox").last()
    await expect(trialRunBox.getByText("Success", { exact: true })).toHaveCount(3, { timeout: 15_000 })

    // US6: confirm-save — filename defaults from the domain, save should succeed.
    await page.getByRole("button", { name: "Confirm and install" }).click()
    await expect(page.getByText(/Installed as custom\/metadata\//)).toBeVisible({ timeout: 10_000 })

    // The saved plugin is immediately real and listable, same as any hand-written one.
    const listRes = await page.request.get("/api/plugins/metadata")
    expect(listRes.ok()).toBe(true)
    const list = (await listRes.json()) as Array<{ namespace: string; generated_by_wizard?: boolean }>
    const saved = list.find((p) => p.namespace.startsWith("custom/metadata/"))
    expect(saved?.generated_by_wizard).toBe(true)
  })
})
