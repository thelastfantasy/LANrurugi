import fs from "node:fs"

import { fixturePath } from "./fixturePath"
import { expect, test } from "./fixtures"

// Covers the orphaned-search-index regression (data-model.md Regression Fixture #3): deleting an
// archive must remove its id from `LRR_TANKGROUPED`/`LRR_UNTAGGED`/`LRR_NEW`/`LRR_TITLES`/
// `INDEX_<tag>`, or a lingering ghost id surfaces as `/search/random` (and this journey's real UI
// entry point, the reader's "Switch to another random archive" link) returning broken/empty
// results. Exercises the actual UI click-through (spec FR-003's "using random-archive navigation"),
// not just the raw `/api/search/random` HTTP endpoint.
//
// To verify this test actually catches the regression: temporarily revert the
// `remove_archive_index` call site in crates/lanrurugi-search/src/indexer.rs's `delete_archive`,
// confirm this test fails, then restore it.
test.describe("archive lifecycle", { tag: "@archive-lifecycle" }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
  })

  // `sample.zip`/`sample.cbz`/`sample.epub` are byte-identical copies with different extensions
  // (T009) — the archive id is a content hash, not filename-derived, so uploading two of them
  // unmodified in the same test collides with a 409 "already exists". Appending unique trailing
  // padding (harmless as zip trailing garbage — the zip's own EOCD record still anchors correctly
  // from the end) gives each upload genuinely distinct content without touching the checked-in
  // fixtures themselves.
  async function uploadArchive(page: import("@playwright/test").Page, fixtureName: string) {
    const original = fs.readFileSync(fixturePath(fixtureName))
    const buffer = Buffer.concat([original, Buffer.from(`unique-${Date.now()}-${Math.random()}`)])
    const res = await page.request.put("/api/archives/upload", {
      multipart: { file: { name: fixtureName, mimeType: "application/zip", buffer } },
    })
    expect(res.ok()).toBe(true)
    const body = (await res.json()) as { success: number; id: string }
    expect(body.success).toBe(1)
    return body.id
  }

  test("deleting an archive does not break random-archive navigation", async ({ page }) => {
    // A second archive must remain after deletion for /random to have something valid to return.
    const survivorId = await uploadArchive(page, "sample.cbz")
    const toDeleteId = await uploadArchive(page, "sample.epub")

    const deleteRes = await page.request.delete(`/api/archives/${toDeleteId}`)
    expect(deleteRes.ok()).toBe(true)

    await page.goto(`/reader/${survivorId}`)
    // The reader opens with the Archive Overview overlay shown by default (readerSettings'
    // showOverlayByDefault, matching legacy) — its full-screen #overlay-shade backdrop intercepts
    // clicks on everything behind it, including the "Switch to another random archive" link this
    // test needs to click. Dismiss it first (same Escape handling the reader itself wires up).
    await page.keyboard.press("Escape")
    const [randomResponse] = await Promise.all([
      page.waitForResponse((r) => r.url().includes("/api/search/random")),
      page.getByText("Switch to another random archive").click(),
    ])
    expect(randomResponse.ok()).toBe(true)
    const randomBody = (await randomResponse.json()) as { data: { arcid: string }[] }
    expect(randomBody.data.length).toBeGreaterThan(0)
    expect(randomBody.data[0].arcid).toBeTruthy()
    expect(randomBody.data[0].arcid).not.toBe(toDeleteId)

    await expect(page).toHaveURL(/\/reader\//)
  })
})
