import fs from "node:fs"

import { fixturePath } from "./fixturePath"
import { expect, test } from "./fixtures"

async function uploadArchive(page: import("@playwright/test").Page, fixtureName: string) {
  const buffer = fs.readFileSync(fixturePath(fixtureName))
  const res = await page.request.put("/api/archives/upload", {
    multipart: { file: { name: fixtureName, mimeType: "application/zip", buffer } },
  })
  const body = (await res.json()) as { success: number; id: string }
  return body.id
}

test.describe("reader", { tag: "@reader" }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
  })

  // Covers the reader toolbar icon-spacing regression (data-model.md Regression Fixture #4): JSX
  // doesn't preserve the whitespace text nodes between sibling elements that legacy's
  // hand-indented HTML template produces "for free", so each toolbar `<a>` needs an explicit
  // marginRight — without it icons render flush against each other (0px instead of 3px).
  //
  // To verify: temporarily remove the `marginRight: 3` style from Reader.tsx's toolbar `<a>`
  // elements, confirm this test fails, then restore it.
  test("toolbar icons have a visible gap between them", async ({ page }) => {
    const id = await uploadArchive(page, "sample.cbz")
    await page.goto(`/reader/${id}`)

    const icons = page.locator(".absolute-options.absolute-left a")
    const first = await icons.nth(0).boundingBox()
    const second = await icons.nth(1).boundingBox()
    expect(first).not.toBeNull()
    expect(second).not.toBeNull()
    const gap = second!.x - (first!.x + first!.width)
    expect(gap).toBeGreaterThanOrEqual(2)
  })

  // Covers the dead-whitespace-below-a-short-page regression (data-model.md Regression Fixture
  // #5): `.loading`'s `min-height: 75vh` must be removed once the image has actually finished
  // loading, not applied unconditionally — otherwise a page shorter than 75vh leaves a dead gap
  // below the rendered image.
  //
  // To verify: temporarily make `#i3`'s className unconditionally include 'loading' again,
  // confirm this test fails, then restore it.
  test("no dead whitespace below a loaded page image", async ({ page }) => {
    const id = await uploadArchive(page, "sample.cbz")
    await page.goto(`/reader/${id}`)

    const image = page.locator("#i3 img.reader-image").first()
    await image.waitFor({ state: "visible" })
    await expect(page.locator("#i3")).not.toHaveClass(/loading/)
  })

  // Covers fit-mode switching, required by spec FR-003 (new coverage for existing functionality,
  // not a historical regression fixture): the setting must actually change the rendered image's
  // sizing and persist across a reload.
  test("fit-mode switching changes rendered image sizing and persists", async ({ page }) => {
    const id = await uploadArchive(page, "sample.cbz")
    await page.goto(`/reader/${id}`)
    // The reader opens with the Archive Overview overlay shown by default (readerSettings'
    // showOverlayByDefault, matching legacy) — its full-screen #overlay-shade backdrop intercepts
    // clicks on everything behind it, including the toolbar. Dismiss it (same Escape handling the
    // reader itself wires up) before interacting with the toolbar.
    await page.keyboard.press("Escape")

    await page.click('a[title="Reader Options"]')
    await page.click('input[value="Width"]')

    const image = page.locator("#i3 img.reader-image").first()
    await expect(image).toHaveCSS("width", /.+/)
    const widthValue = await image.evaluate((el) => (el as HTMLElement).style.width)
    expect(widthValue).toBe("100%")

    await page.reload()
    await page.keyboard.press("Escape")
    await page.click('a[title="Reader Options"]')
    const widthButton = page.locator('input[value="Width"]')
    await expect(widthButton).toHaveClass(/toggled/)
  })
})
