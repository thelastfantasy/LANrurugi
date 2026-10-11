import { expect, test } from "./fixtures"

// Covers issue #92's own acceptance criteria for 403/404 error pages — an unknown route showing
// real 404 content (not a blank body inside Layout's own nav/Footer shell), a Reader archive/
// Tankoubon id that doesn't exist rendering the same 404 content *inline* with the URL staying put
// (never a `navigate()` to some other route), and a legitimate deep link still working normally
// (no regression from the new catch-all route).
test.describe("error pages", { tag: "@error-pages" }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
  })

  test("an unknown top-level route shows 404 content inside the normal Layout shell", async ({ page }) => {
    await page.goto("/not-found-page")

    // Layout's own nav (`Layout.tsx`) is still there — the catch-all route is nested under
    // `Layout`, not a bare top-level sibling, so this isn't a blank/chrome-less page.
    await expect(page.locator("#nb")).toBeVisible()
    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible()
    await expect(page.getByText("This page doesn't exist, or the address is incorrect.")).toBeVisible()

    const returnButton = page.getByRole("button", { name: "Return to Library" })
    await expect(returnButton).toBeVisible()
    await returnButton.click()
    await expect(page).toHaveURL("/")
  })

  test("a multi-segment unknown route also hits the 404 page", async ({ page }) => {
    await page.goto("/foo/bar/baz")
    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible()
  })

  test("opening the reader on a nonexistent archive id shows 404 content inline, URL unchanged", async ({
    page,
  }) => {
    await page.goto("/reader/nonexistent-archive-id-e2e-test")

    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible()
    // The core issue #92 requirement: no navigation away from the URL that produced the 404.
    await expect(page).toHaveURL("/reader/nonexistent-archive-id-e2e-test")

    await expect(page.getByRole("button", { name: "Return to Library" })).toBeVisible()
  })

  test("opening the reader on a nonexistent Tankoubon id shows the same inline 404 content", async ({ page }) => {
    await page.goto("/reader/TANK_nonexistent-e2e-test")

    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible()
    await expect(page).toHaveURL("/reader/TANK_nonexistent-e2e-test")
  })

  test("a legitimate deep link still loads normally (no regression from the catch-all route)", async ({ page }) => {
    // `?section=` deep-linking (routes.ts's own `settings()` builder) is exactly the kind of
    // legitimate multi-part path a catch-all route could accidentally start swallowing if it were
    // ever misplaced ahead of the real routes in App.tsx's route table.
    await page.goto("/config?section=security")
    await expect(page.getByRole("heading", { name: "Page Not Found" })).not.toBeVisible()
    await expect(page.locator('[data-section-id="security"]')).toBeVisible()
  })

  test("an unknown route while logged out shows 404 content and does NOT redirect to /login", async ({
    page,
  }) => {
    // A real regression this suite caught during implementation: `Layout.tsx`'s own
    // `useApplyTheme`/`useApplySettingsLanguage` both unconditionally called `useSettings()` (the
    // auth-gated `GET /settings`, which 401s pre-login since that response also carries the API
    // key), and `client.ts`'s own *global* 401 handler force-navigates to `/login` on any 401
    // regardless of whether the calling hook already had a public-endpoint fallback ready for the
    // resulting *data* — a real "the 404 page flashes, then it jumps to /login anyway"
    // double-navigation, defeating this whole file's own "stay on the page/URL" requirement for
    // every page rendered while logged out, not just this one.
    await page.request.post("/api/logout")

    await page.goto("/some-unknown-route-while-logged-out")

    await expect(page.getByRole("heading", { name: "Page Not Found" })).toBeVisible()
    await expect(page).toHaveURL("/some-unknown-route-while-logged-out")
  })
})
