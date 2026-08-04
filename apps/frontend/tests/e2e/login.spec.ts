import { expect, test } from "./fixtures"

// Covers the sign-in journey required by spec FR-003. A fresh Redis DB has no `password` field
// set, so the backend falls back to legacy's own default password hash
// (crates/lanrurugi-api/src/auth.rs::load, DEFAULT_PASSWORD_HASH) — "kamimamita" (see
// crates/lanrurugi-core/src/password.rs's own test verifying this literal legacy default).
test.describe("login", { tag: "@login" }, () => {
  test("valid credentials reach an authenticated session", async ({ page }) => {
    await page.goto("/login")
    await page.fill("#pw_field", "kamimamita")
    await page.click('input[type="submit"]')

    await expect(page).toHaveURL("/")
    const status = await page.request.get("/api/login/status")
    expect(status.ok()).toBe(true)
    expect(await status.json()).toMatchObject({ logged_in: true })
  })

  test("invalid credentials surface a clear failure", async ({ page }) => {
    await page.goto("/login")
    await page.fill("#pw_field", "definitely-wrong-password")
    await page.click('input[type="submit"]')

    await expect(page.getByText("Wrong Password.")).toBeVisible()
    await expect(page).toHaveURL("/login")
  })
})
