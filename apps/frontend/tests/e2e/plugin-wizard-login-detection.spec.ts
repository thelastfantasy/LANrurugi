import http from "node:http"

import { expect, test } from "./fixtures"
import { MockLlmServer, respondContent } from "./mockLlmServer"

// T048 (US7) — a metadata trial run fails against a fixture page that 403s without a session
// cookie; the mocked LLM's login-relevance classification call says `relevant: true`; the user
// adds and validates a login plugin from within the wizard without restarting it; accepting the
// suggestion re-associates the originally-failing draft and a fresh generate + trial-run against
// it (now carrying real cookies from a successful login) succeeds.
const MOCK_LLM_PORT = 6430 + Number(process.env.TEST_PARALLEL_INDEX ?? 0)
const FIXTURE_SITE_PORT = 6440 + Number(process.env.TEST_PARALLEL_INDEX ?? 0)
process.env.LANRURUGI_DEEPSEEK_BASE_URL = `http://127.0.0.1:${MOCK_LLM_PORT}`

const SESSION_COOKIE_NAME = "wizard_e2e_session"
/** Fixed self-declared `pluginInfo().namespace` the mocked login plugin always returns — what a
 * *different* plugin's `login_from` must reference to resolve back to it (`save.rs`'s
 * `declared_namespace` in its save response, distinct from the file-path namespace the UI
 * displays as "Installed as ..."). Known ahead of time since it's hardcoded in
 * `loginPluginSource()` below, so this test never needs to scrape it out of the DOM. */
const LOGIN_DECLARED_NAMESPACE = "wizard-e2e-login-fixture-login"

/** A fixture site with two behaviors: `/work/*` 403s unless the `wizard_e2e_session` cookie is
 * present (simulating a login-gated page), and `/login` "logs in" by echoing that cookie back —
 * real network I/O the generated plugins' own trial runs hit for real, not mocked at the
 * plugin-execution layer. */
function buildFixtureSite(): http.Server {
  return http.createServer((req, res) => {
    const cookies = req.headers.cookie ?? ""
    if (req.url === "/login") {
      res.writeHead(200, {
        "Content-Type": "application/json",
        "Set-Cookie": `${SESSION_COOKIE_NAME}=granted`,
      })
      res.end(JSON.stringify({ ok: true }))
      return
    }
    if (req.url?.startsWith("/work/")) {
      if (!cookies.includes(`${SESSION_COOKIE_NAME}=granted`)) {
        res.writeHead(403, { "Content-Type": "text/plain" })
        res.end("Forbidden: login required")
        return
      }
      res.writeHead(200, { "Content-Type": "text/html" })
      res.end(`<html><body><h1 id="title">Fixture Work ${req.url.slice(6)}</h1></body></html>`)
      return
    }
    res.writeHead(404)
    res.end("not found")
  })
}

function metadataPluginSource(fixtureBase: string, loginFrom?: string): string {
  const loginFromLine = loginFrom ? `login_from: ${JSON.stringify(loginFrom)},` : ""
  return `
export function pluginInfo() {
  return {
    namespace: "wizard-e2e-login-fixture",
    type: "metadata",
    parameters: [],
    declared_permissions: { net: ["${new URL(fixtureBase).hostname}"], read: false, write: false },
    generated_by_wizard: true,
    ${loginFromLine}
    name: "Wizard E2E Login-Gated Fixture Plugin",
    author: "e2e",
    description: "Generated for plugin-wizard-login-detection.spec.ts",
    version: "1.0.0",
  };
}

export async function execMetadata(hostArgs) {
  const headers = {};
  const cookies = hostArgs.user_agent_cookies ?? [];
  if (cookies.length > 0) {
    headers["Cookie"] = cookies.map(c => \`\${c.name}=\${c.value}\`).join("; ");
  }
  const resp = await fetch(hostArgs.url, { headers });
  if (!resp.ok) {
    throw new Error(\`HTTP \${resp.status}: \${await resp.text()}\`);
  }
  const html = await resp.text();
  const match = html.match(/<h1 id="title">([^<]*)<\\/h1>/);
  return { title: match ? match[1] : "unknown", tags: "source:wizard-e2e" };
}
`
}

function loginPluginSource(fixtureBase: string): string {
  return `
export function pluginInfo() {
  return {
    namespace: "wizard-e2e-login-fixture-login",
    type: "login",
    // Order matters — execLogin below destructures customargs positionally, and
    // trial_run.rs::customargs_for maps loginFieldValues onto these declared parameters in this
    // same declared order (by name, not by the analyze-login response's own field order).
    parameters: [
      { name: "account", description: "Test account", required: true, type: "string" },
      { name: "secret", description: "Test password", required: true, type: "string" },
    ],
    declared_permissions: { net: ["${new URL(fixtureBase).hostname}"], read: false, write: false },
    generated_by_wizard: true,
    name: "Wizard E2E Login Fixture Plugin",
    author: "e2e",
    description: "Generated for plugin-wizard-login-detection.spec.ts",
    version: "1.0.0",
  };
}

export async function execLogin(hostArgs) {
  const [account, secret] = hostArgs.customargs ?? [];
  if (!account || !secret) {
    throw new Error("missing credentials");
  }
  const resp = await fetch("${fixtureBase}/login");
  const setCookie = resp.headers.get("set-cookie") ?? "";
  const [name, value] = setCookie.split(";")[0].split("=");
  return { cookies: [{ name, value, domain: "${new URL(fixtureBase).hostname}", path: "/" }] };
}
`
}

test.describe("plugin wizard login detection", { tag: "@plugin-wizard" }, () => {
  let mockLlm: MockLlmServer
  let fixtureSite: http.Server
  let fixtureBase: string

  test.beforeEach(async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })

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

  test("a login-related failure surfaces an AI-sourced suggestion, and accepting it fixes the draft", async ({
    page,
  }) => {
    // Call order: (1) generate metadata draft, (2) classify-login-relevance after the failed
    // trial run, (3) analyze-login (the credential-field form is AI-derived, not a hardcoded
    // account/password pair — see `TypeDetailsForm.tsx`'s own doc comment), (4) generate login
    // draft. The 5th call (metadata regenerated with `login_from`) is enqueued later, once the
    // login plugin's *real* saved namespace is known — the UI derives that namespace from the
    // domain at save time, which this test can't predict ahead of a real save round-trip, so it
    // can't be scripted upfront like the first four.
    mockLlm.enqueue(respondContent(metadataPluginSource(fixtureBase)))
    mockLlm.enqueue(respondContent(JSON.stringify({ relevant: true, reasoning: "403 Forbidden looks login-gated." })))
    mockLlm.enqueue(
      respondContent(
        JSON.stringify([
          { name: "account", description: "Test account", required: true },
          { name: "secret", description: "Test password", required: true },
        ]),
      ),
    )
    mockLlm.enqueue(respondContent(loginPluginSource(fixtureBase)))

    await page.goto("/config/plugins/wizard")

    await page.locator('input[type="text"]').first().fill("wizard-e2e-login-fixture.invalid")
    await page.getByRole("button", { name: "Look up" }).click()
    await expect(page.getByText("Select which plugin types to create")).toBeVisible()

    // Only "metadata" selected up front — no login type this time, so the failure must be
    // discovered via the trial run itself, not declared ahead of time (US7's whole premise).
    await page.getByRole("checkbox", { name: "Metadata" }).check()
    await page.getByRole("button", { name: "Next", exact: true }).click()

    // Shared-links step — one domain-level textarea, same three fixture pages as the "full
    // journey" test.
    await page
      .getByLabel(/^Links \(one per line/)
      .fill([`${fixtureBase}/work/1`, `${fixtureBase}/work/2`, `${fixtureBase}/work/3`].join("\n"))
    await page.getByRole("button", { name: "Next", exact: true }).click()

    // Generate & save step — only "metadata" selected so far, its own panel is the only one
    // mounted.
    await page.getByRole("button", { name: "Generate", exact: true }).click()
    await expect(page.getByRole("button", { name: "Trial run" })).toBeEnabled({ timeout: 10_000 })

    // The trial run fails (no login cookie yet) — the mocked classification call then fires
    // automatically inside trial_run.rs itself and returns `relevant: true`.
    await page.getByRole("button", { name: "Trial run" }).click()
    const trialRunBox = page.locator(".ptbox").last()
    await expect(trialRunBox.getByText("Failure", { exact: true })).toHaveCount(3, { timeout: 15_000 })
    await expect(page.getByText("This failure might be login-related")).toBeVisible()

    // Accept the suggestion: no existing login plugin for this domain, so "Add a login plugin".
    // This only adds "login" to `selectedTypes` — it does NOT switch the currently-shown panel
    // (`useWizardSession.ts`'s own `typeSelected` reducer docs) — so a tab row now appears
    // (>1 selected type) and must be clicked to actually see the new login `TypeWizardPanel`.
    await page.getByRole("button", { name: "Add a login plugin" }).click()
    await page.getByRole("tab", { name: "Login" }).click()

    // The login type's own per-type step: point login analysis at the fixture's `/login`
    // endpoint, run it (mocked LLM call #3 above), then fill whatever credential fields AI
    // decided this site needs — never a hardcoded "Test account"/"Test password" pair.
    await page.getByLabel("Login page or API documentation URL").fill(`${fixtureBase}/login`)
    await page.getByRole("button", { name: "Analyze login method" }).click()
    // `required: true` above appends a literal " *" to the rendered label
    // (`TypeDetailsForm.tsx`'s own `{param.required && " *"}`) — not an exact match.
    await expect(page.getByLabel("Test account *")).toBeVisible({ timeout: 10_000 })
    await page.getByLabel("Test account *").fill("test-user")
    await page.getByLabel("Test password *").fill("test-pass")

    await page.getByRole("button", { name: "Generate", exact: true }).click()
    await expect(page.getByRole("button", { name: "Trial run" })).toBeEnabled({ timeout: 10_000 })
    await page.getByRole("button", { name: "Trial run" }).click()
    await expect(page.getByText("Success", { exact: true }).first()).toBeVisible({ timeout: 15_000 })

    // Save the login plugin — this is what makes its (self-declared) namespace real/resolvable
    // (spec FR-025's amended requirement: association needs a saved namespace, not merely a
    // passing trial run).
    await page.getByRole("button", { name: "Confirm and install" }).click()
    await expect(page.getByText(/Installed as custom\/login\//)).toBeVisible({ timeout: 10_000 })

    // The 5th mocked response (metadata regenerated with `login_from` pointing at the login
    // plugin's own self-declared namespace) — enqueued now, consumed by the "link and regenerate"
    // action's own `generate` call below.
    mockLlm.enqueue(respondContent(metadataPluginSource(fixtureBase, LOGIN_DECLARED_NAMESPACE)))

    // Switch back to the metadata tab — the "link to <namespace> and regenerate" action is now
    // available there.
    await page.getByRole("tab", { name: "Metadata" }).click()
    await page.getByRole("button", { name: new RegExp(`Link to ${LOGIN_DECLARED_NAMESPACE}`) }).click()
    await page.getByRole("button", { name: "Generate", exact: true }).click()
    await expect(page.getByRole("button", { name: "Trial run" })).toBeEnabled({ timeout: 10_000 })
    await page.getByRole("button", { name: "Trial run" }).click()

    const finalTrialRunBox = page.locator(".ptbox").filter({ hasText: "Success" }).last()
    await expect(finalTrialRunBox.getByText("Success", { exact: true })).toHaveCount(3, { timeout: 15_000 })

    // T044/quickstart scenario 11 (FR-012, security-critical): the test credentials typed into the
    // login TypeSession must never appear in any outbound LLM request body — checked here by
    // directly inspecting every raw request this mocked DeepSeek endpoint actually received across
    // the whole journey (generate x2, classify, generate-with-login_from), not just by reading the
    // code and assuming FR-012 holds.
    for (const body of mockLlm.requestBodies()) {
      expect(body).not.toContain("test-user")
      expect(body).not.toContain("test-pass")
    }
  })
})
