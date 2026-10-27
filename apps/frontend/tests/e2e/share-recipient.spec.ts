import fs from "node:fs"

import { fixturePath } from "./fixturePath"
import { expect, test } from "./fixtures"

// 017 US1's browser journey: an administrator shares an archive with a download limit of 2; people
// with only the link browse, read and download it without an account, the third downloader finds
// the downloads used up, and removing the read permission hides Read.
test.describe("share recipient", { tag: "@share" }, () => {
  test("browse, read and download a share with only its link", async ({ page, browser, baseURL }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
    const buffer = fs.readFileSync(fixturePath("sample.zip"))
    const upload = await page.request.put("/api/archives/upload", {
      multipart: { file: { name: "share-journey-sample.zip", mimeType: "application/zip", buffer } },
    })
    expect(upload.ok()).toBe(true)
    const { id: archiveId } = (await upload.json()) as { id: string }

    const created = await page.request.post("/api/shares", {
      data: {
        items: [{ kind: "archive", id: archiveId }],
        title: "Share journey",
        download_limit: 2,
        permissions: { import: true, read: true, download: true },
      },
    })
    expect(created.status()).toBe(201)
    const share = (await created.json()) as { id: string; code: { code: string } }
    const link = `${baseURL}/s/${share.code.code}`

    // Two recipients, each in a fresh browser with no session, read and download.
    for (let i = 0; i < 2; i++) {
      const context = await browser.newContext()
      const visitor = await context.newPage()
      await visitor.goto(link)
      await expect(visitor.getByRole("heading", { name: "Share journey" })).toBeVisible()
      if (i === 0) {
        await visitor.getByRole("button", { name: "Read" }).click()
        await expect(visitor.locator("img.share-reader-page")).toBeVisible()
        await visitor.getByRole("button", { name: "Close" }).click()
      }
      const download = visitor.waitForEvent("download")
      await visitor.getByRole("link", { name: "Download" }).click()
      expect((await download).suggestedFilename()).toContain("share-journey-sample")
      await context.close()
    }

    // The third finds the downloads used up, but can still browse.
    const third = await browser.newContext()
    const late = await third.newPage()
    await late.goto(link)
    await expect(late.locator(".share-notice.share-status-exhausted")).toBeVisible()
    await expect(late.getByRole("link", { name: "Download" })).toHaveCount(0)
    await third.close()

    // Without the read permission, Read is not offered.
    const patched = await page.request.patch(`/api/shares/${share.id}`, {
      data: { permissions: { import: true, read: false, download: true } },
    })
    expect(patched.ok()).toBe(true)
    const fourth = await browser.newContext()
    const noRead = await fourth.newPage()
    await noRead.goto(link)
    await expect(noRead.getByRole("heading", { name: "Share journey" })).toBeVisible()
    await expect(noRead.getByRole("button", { name: "Read" })).toHaveCount(0)
    await fourth.close()
  })
})
