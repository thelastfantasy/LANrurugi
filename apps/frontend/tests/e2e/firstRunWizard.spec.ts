import { expect, test } from "./fixtures"

// Covers the first-run wizard's two entry points and the one piece of its behaviour that only a
// real browser + real backend can exercise: the `first_run_wizard_pending` → redirect →
// marker-write → no-second-redirect loop across a reload.
//
// The shared worker fixture seeds `first_run_wizard_done 1` (see fixtures.ts's own comment on
// that line) so every *other* spec keeps landing on the library after login. These tests re-arm
// the field through the settings API and restore it in a `finally`, since a worker runs its tests
// one at a time and a leaked `false` here would redirect the next spec.
test.describe("first-run wizard", { tag: "@first-run-wizard" }, () => {
  test("a fresh instance opens the wizard once, and skipping it stops it coming back", async ({
    page,
  }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
    const rearm = await page.request.put("/api/settings", {
      data: { first_run_wizard_done: false },
    })
    expect(rearm.ok()).toBe(true)

    try {
      // Landing on the library is what a fresh instance's first authenticated page load looks
      // like (`Login.tsx` navigates here), so this is the path the gate has to intercept.
      await page.goto("/")
      await expect(page).toHaveURL("/welcome")

      // The wizard is all-optional by design: the one button that ends it without doing anything.
      await page.getByRole("button", { name: "Skip setup and start using LANrurugi" }).click()
      await expect(page).toHaveURL("/")

      // The marker must have been written, not just a client-side flag set: a reload has to stay
      // on the library rather than bouncing straight back into the wizard.
      await page.reload()
      await expect(page).toHaveURL("/")
    } finally {
      await page.request.put("/api/settings", { data: { first_run_wizard_done: true } })
    }
  })

  test("the Settings page re-runs the wizard and comes back to Settings", async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })

    await page.goto("/config")
    await page.getByRole("button", { name: "Run setup wizard again" }).click()
    await expect(page).toHaveURL("/welcome")

    // `?from=settings` is the whole difference between this run and the automatic one — it must
    // hand the user back to where they launched it from, not dump them in the library.
    await page.getByRole("button", { name: "Skip setup and start using LANrurugi" }).click()
    await expect(page).toHaveURL("/config")
  })

  test("each step can be visited and skipped individually", async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })

    await page.goto("/welcome")
    await page.getByRole("button", { name: "Next" }).click()

    // Every step's own skip advances to the next one without saving anything.
    await expect(page.getByRole("heading", { name: "Admin password" })).toBeVisible()
    await page.getByRole("button", { name: "Skip this step" }).click()

    await expect(page.getByRole("heading", { name: "DeepSeek API key" })).toBeVisible()
    await page.getByRole("button", { name: "Skip this step" }).click()

    await expect(page.getByText("Import from a LANraragi backup")).toBeVisible()
    await page.getByRole("button", { name: "Next" }).click()

    await expect(page.getByRole("heading", { name: "You're all set" })).toBeVisible()
    await page.getByRole("button", { name: "Submit and go to the library" }).click()
    await expect(page).toHaveURL("/")
  })
})
