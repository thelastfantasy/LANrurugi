import fs from "node:fs"

import { fixturePath } from "./fixturePath"
import { expect, test } from "./fixtures"

// Covers the guest journey (007-guest-restricted-access, US2/US3's own Independent Test): with
// guest mode on and one category marked guest-visible, an unauthenticated visitor lands on a
// scoped library view (not /login), can open an in-scope archive in the reader, and sees no
// bookmark/download affordance there — the exact end-to-end path unit/integration coverage
// elsewhere in this feature can't exercise on its own.
test.describe("guest access", { tag: "@guest-access" }, () => {
  test("unauthenticated visitor browses an in-scope archive with no bookmark/download affordances", async ({
    page,
  }) => {
    // Set up as an authenticated admin first: upload one fixture archive, put it in a
    // guest-visible category, and flip the site-wide switch on.
    await page.request.post("/api/login", { form: { password: "kamimamita" } })

    const buffer = fs.readFileSync(fixturePath("sample.zip"))
    const uploadRes = await page.request.put("/api/archives/upload", {
      multipart: { file: { name: "guest-journey-sample.zip", mimeType: "application/zip", buffer } },
    })
    expect(uploadRes.ok()).toBe(true)
    const { id: archiveId } = (await uploadRes.json()) as { id: string }

    const categoryRes = await page.request.put("/api/categories", {
      form: { name: "Guest Journey Visible", search: "" },
    })
    expect(categoryRes.ok()).toBe(true)
    const { category_id: categoryId } = (await categoryRes.json()) as { category_id: string }

    await page.request.put(`/api/categories/${categoryId}/${archiveId}`)
    const updateRes = await page.request.put(`/api/categories/${categoryId}`, {
      form: { name: "Guest Journey Visible", search: "", pinned: "false", visible_to_guest: "true" },
    })
    expect(updateRes.ok()).toBe(true)

    const settingsRes = await page.request.put("/api/settings", { data: { guestmode: true } })
    expect(settingsRes.ok()).toBe(true)

    // Now become the unauthenticated visitor.
    await page.request.post("/api/logout")

    await page.goto("/")
    // A guest lands on the scoped library view, not a /login redirect.
    await expect(page).toHaveURL("/")
    await expect(page.getByText("Guest Journey Visible")).toBeVisible()

    await page.goto(`/reader/${archiveId}`)
    await expect(page).toHaveURL(`/reader/${archiveId}`)
    // The bookmark toggle (Reader.tsx's own `toggle-bookmark` icon) must render in its
    // logged-out `disabled` state, not the interactive one a real session gets. `.first()` —
    // `Reader.tsx`'s own `pagesel` toolbar (which this icon lives in) renders twice unconditionally
    // (`#i2`'s header copy and `#i4`'s footer copy), same as legacy's own reader layout; both
    // instances always carry the same class, so asserting on either is equivalent.
    await expect(page.locator(".toggle-bookmark").first()).toHaveClass(/disabled/)
    // No download link/button anywhere on the reader page for a guest.
    await expect(page.locator(`a[href="/api/archives/${archiveId}/download"]`)).toHaveCount(0)
  })
})
