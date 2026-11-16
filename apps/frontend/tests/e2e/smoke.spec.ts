import { seed } from "../seed/seed.mjs"
import { expect, test } from "./fixtures"

// #286: every main page, opened with the seed data loaded, must render without an uncaught error.
// The settings crash of #281 needed exactly such data (a subscription awaiting approval with titles
// by language), which no other spec creates — so this one fails on that kind of bug where they pass.

const PAGES = ["/", "/upload", "/activity", "/jobs", "/stats", "/bookmarks", "/config", "/config/plugins", "/config/categories"]

/** React logs a caught render error to the console rather than throwing it to the page. */
const REACT_ERROR = /Minified React error|Objects are not valid as a React child|The above error occurred|Rendered (more|fewer) hooks/

test.describe("smoke", { tag: "@smoke" }, () => {
  test.beforeEach(async ({ page }, testInfo) => {
    // The worker's own Redis, on the port `fixtures.ts` derives from the same index.
    seed({ port: 6390 + testInfo.parallelIndex })
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
  })

  for (const path of PAGES) {
    test(`${path} renders without errors`, async ({ page }) => {
      const problems: string[] = []
      page.on("pageerror", (error) => problems.push(`pageerror: ${error.message}`))
      page.on("console", (message) => {
        if (message.type() === "error" && REACT_ERROR.test(message.text())) problems.push(`console: ${message.text()}`)
      })

      await page.goto(path)
      await page.waitForLoadState("networkidle")
      if (path === "/config") {
        // Sections render their bodies only once opened. Always the first still-collapsed one: each
        // click takes a heading out of this set, so a list snapshotted up front runs past its end.
        // Bounded, since an opened section can reveal collapsible headings of its own.
        await page.locator(".collapsible-title").first().waitFor()
        // Visible only: a section nested in a collapsed one is rendered but hidden until its parent opens.
        const collapsed = page.locator(".collapsible-title:not(.active):visible")
        for (let opened = 0; opened < 40 && (await collapsed.count()) > 0; opened++) {
          // At the leading icon, not the centre: some headings carry buttons of their own there
          // (the subscriptions heading's history button opens a dialog over the page).
          await collapsed.first().click({ position: { x: 12, y: 12 } })
        }
        await page.waitForLoadState("networkidle")
        // The seeded approval with titles by language is what crashed this page in #281.
        await expect(page.getByText(/Seed work with titles by language|多言語タイトルのシード作品/).first()).toBeVisible()
      }

      await expect(page.locator(".section-error")).toHaveCount(0)
      expect(problems).toEqual([])
    })
  }
})
