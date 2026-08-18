import fs from "node:fs"

import { fixturePath } from "./fixturePath"
import { expect, test } from "./fixtures"

// Covers issue #89's own acceptance criteria for the `/stats` 3D tag cloud — the specific
// behaviors a manual click-through can't reliably re-verify every time this component changes:
// real tags actually render (not just "the component mounted"), a tag click drives the detailed
// stats accordion open and highlights the matching row, and `prefers-reduced-motion` degrades to a
// non-drifting sphere rather than silently ignoring the preference. Does NOT re-test the 3D
// library's own physics (drift/rotation math, hover-scale easing) — that's `TagCloud@2.5.0`'s own
// concern, not this app's; `tests/unit/tagCloud.test.ts` already covers this app's own
// weight->level/density/font-scale math directly without needing a real browser.
test.describe("stats tag cloud", { tag: "@stats" }, () => {
  test.beforeEach(async ({ page }) => {
    await page.request.post("/api/login", { form: { password: "kamimamita" } })
  })

  // Same real, valid zip fixture `upload.spec.ts`'s own suite already relies on — a hand-built
  // minimal zip byte sequence isn't actually valid enough for the backend's real libarchive-based
  // ingestion to accept (confirmed live: a bare 4-byte EOCD-only buffer 200'd nothing and failed
  // the very first upload assertion here).
  const sampleZipBase = fs.readFileSync(fixturePath("sample.zip"))

  async function uploadTaggedArchive(
    page: import("@playwright/test").Page,
    filename: string,
    tags: string,
  ): Promise<string> {
    // The backend dedupes uploads by *content hash*, not filename (`upload.spec.ts`'s own
    // `race-check` test relies on the same fact) — every call in this suite must therefore append
    // its own unique bytes, or the second/third upload of this suite's otherwise-identical
    // `sampleZipBase` 409s as "already exists" (live-confirmed: `{"error":"This file already
    // exists in the Library.","success":0}`), never reaching this suite's own tag-setup step.
    const zip = Buffer.concat([sampleZipBase, Buffer.from(`${filename}-${Math.random()}`)])
    const res = await page.request.put("/api/archives/upload", {
      multipart: { file: { name: filename, mimeType: "application/zip", buffer: zip } },
    })
    expect(res.ok()).toBe(true)
    const body = (await res.json()) as { id: string }
    const metaRes = await page.request.put(`/api/archives/${body.id}/metadata`, {
      params: { tags },
    })
    expect(metaRes.ok()).toBe(true)
    return body.id
  }

  // How many archives to tag with a "make sure the sphere actually renders this one" tag — this
  // suite's own worker backend accumulates real tags across every test that's run against it
  // (fixtures.ts's backend is worker-scoped, not per-test), so a tag this test needs *visibly
  // rendered in the capped/density-scaled 3D sphere* (not just present in the uncapped detailed
  // stats list below it) needs a weight comfortably higher than anything else realistically
  // already in there, not just "more than 1" — live-confirmed flaky at weight 1 once the worker
  // had accumulated enough tags from other tests in this same suite.
  const HIGH_WEIGHT_ARCHIVE_COUNT = 20

  /** Tags `HIGH_WEIGHT_ARCHIVE_COUNT` distinct archives with the same tag so it ranks near the top
   * of the sphere's own weight ordering regardless of whatever else this shared worker's library
   * already has in it. Uploads concurrently (not a serial loop) — `HIGH_WEIGHT_ARCHIVE_COUNT`
   * sequential round-trips (upload + metadata-plugin ingestion + metadata update each) blew past
   * Playwright's default 30s per-test timeout in practice; the backend itself handles concurrent
   * uploads fine (`upload.spec.ts`'s own "repeated first-time uploads" test already exercises
   * that), so there's no reason this suite needs to serialize them. */
  async function uploadHighWeightTag(page: import("@playwright/test").Page, tag: string, namePrefix: string) {
    await Promise.all(
      Array.from({ length: HIGH_WEIGHT_ARCHIVE_COUNT }, (_, i) => uploadTaggedArchive(page, `${namePrefix}-${i}.zip`, tag)),
    )
  }

  test("renders real tags from the library and reflects weight via distinct font sizes", async ({ page }) => {
    const suffix = Date.now()
    // This suite's own worker backend (fixtures.ts) is shared and long-lived across every prior
    // test that ran against it, so the library realistically already has other real tags in it by
    // the time this test runs — `sharedtag` needs a weight that's unambiguously higher than
    // *anything* else already in there, not just higher than this test's own single low-weight
    // tag, or `TagCloud.tsx`'s own `MAX_TAGS`/density cap can legitimately drop the low-weight one
    // before this test ever gets to look at it (live-confirmed: an earlier version of this test
    // used weight 3 vs. 1 and flaked exactly this way once the worker had accumulated enough
    // tags from other suites). `HIGH_WEIGHT_ARCHIVE_COUNT` archives sharing the tag is comfortably
    // past what any other single tag in this test file's own fixtures could realistically reach.
    // Concurrent, not serial — see `uploadHighWeightTag`'s own docs on why. `uniquealpha` gets
    // exactly 2 archives (not 1) — `Stats.tsx` itself calls `useStats(2)`, i.e. only weight >= 2
    // tags are fetched/displayed at all; a weight-1 tag never appears in the sphere *or* the
    // detailed list regardless of any cap, which is the page's own real `minweight` filter working
    // as intended, not a bug (an earlier version of this test used weight 1 and failed here for
    // exactly that reason — confirmed live via a direct `/archives/{id}/metadata` check showing the
    // tag really was saved, just correctly excluded from `/database/stats?minweight=2`).
    await Promise.all(
      Array.from({ length: HIGH_WEIGHT_ARCHIVE_COUNT }, (_, i) => {
        const tags = i < 2 ? `female:sharedtag${suffix},other:uniquealpha${suffix}` : `female:sharedtag${suffix}`
        return uploadTaggedArchive(page, `stats-${i}-${suffix}.zip`, tags)
      }),
    )

    await page.goto("/stats")
    const sphereItem = page.locator(".tag-cloud-3d-item", { hasText: `sharedtag${suffix}` })
    await expect(sphereItem).toBeVisible()

    // Detailed stats list below the sphere shows every tag with its own real weight, independent
    // of the 3D sphere's own MAX_TAGS/density cap — this is where the low-weight tag's own
    // existence and exact count get verified, not the (capped, possibly-excluded) sphere.
    // `[data-section-id="detailed-stats"]` (not the title's own translated text — this test
    // container's browser locale is `en-US`, so a hardcoded Chinese string like "详细统计" never
    // matches and this line hung until Playwright's own 30s test timeout, live-confirmed via a
    // failure screenshot that showed the real page rendered in English, "Detailed Stats").
    await page.locator('[data-section-id="detailed-stats"] .collapsible-title').click()
    await expect(page.locator(`[data-tag-key="female:sharedtag${suffix}"]`)).toContainText(`(${HIGH_WEIGHT_ARCHIVE_COUNT})`)
    await expect(page.locator(`[data-tag-key="other:uniquealpha${suffix}"]`)).toContainText("(2)")

    // Weight -> size: the sphere's own highest-weight tag renders measurably larger than
    // *whichever* tag its own sphere considers lowest-weight among what it actually rendered
    // (not necessarily this test's own `uniquealpha`, which the density cap may have excluded) —
    // still a real, meaningful assertion that `levelFor`'s weight->size mapping is live and
    // working, without assuming any specific tag survives the cap.
    const allSphereFontSizes = await page
      .locator(".tag-cloud-3d-item .tag-cloud-3d-inner")
      .evaluateAll((els) => els.map((el) => parseFloat(getComputedStyle(el).fontSize)))
    const sharedFontSize = parseFloat(
      await sphereItem.locator(".tag-cloud-3d-inner").evaluate((el) => getComputedStyle(el).fontSize),
    )
    expect(sharedFontSize).toBe(Math.max(...allSphereFontSizes))
    expect(Math.min(...allSphereFontSizes)).toBeLessThan(sharedFontSize)
  })

  test("clicking a sphere tag opens the detailed stats accordion and highlights the matching row", async ({
    page,
  }) => {
    const suffix = Date.now()
    await uploadHighWeightTag(page, `other:clicktarget${suffix}`, `stats-click-${suffix}`)

    await page.goto("/stats")
    const sphereItem = page.locator(".tag-cloud-3d-item", { hasText: `clicktarget${suffix}` })
    await expect(sphereItem).toBeVisible()

    const row = page.locator(`[data-tag-key="other:clicktarget${suffix}"]`)
    await expect(row).not.toHaveClass(/tag-list-highlighted/)

    // Not `sphereItem.click()` — the sphere drifts/auto-rotates continuously by design (issue
    // #89's own core requirement), so Playwright's default click action (which waits for the
    // target to be "stable", i.e. motionless across consecutive frames) never succeeds against a
    // tag that's always moving; live-confirmed via a real failure log showing dozens of "element
    // is not stable" retries before Playwright's own 30s test timeout. Dispatching a real `click`
    // `MouseEvent` directly at the element's current position is what an actual visitor's click
    // does too — they hit whatever position the tag happens to occupy at that instant, no
    // different in kind from this.
    await sphereItem.dispatchEvent("click")

    await expect(page.locator('[data-section-id="detailed-stats"] .collapsible-title')).toHaveClass(/active/)
    await expect(row).toHaveClass(/tag-list-highlighted/)
    await expect(row).toBeInViewport()
  })

  test("prefers-reduced-motion renders a static (non-drifting) sphere", async ({ page }) => {
    const suffix = Date.now()
    await uploadHighWeightTag(page, `other:motiontag${suffix}`, `stats-motion-${suffix}`)

    await page.emulateMedia({ reducedMotion: "reduce" })
    await page.goto("/stats")
    const item = page.locator(".tag-cloud-3d-item", { hasText: `motiontag${suffix}` })
    await expect(item).toBeVisible()

    // No pointer interaction at all — a reduced-motion sphere is `pause()`d immediately after
    // construction (TagCloud.tsx) and never auto-resumes on its own, so its `transform` should be
    // completely unchanged across this wait regardless of how long it is.
    const transformBefore = await item.evaluate((el) => (el as HTMLElement).style.transform)
    await page.waitForTimeout(1500)
    const transformAfter = await item.evaluate((el) => (el as HTMLElement).style.transform)
    expect(transformAfter).toBe(transformBefore)
  })

  // The empty-tags/loading render paths are deliberately NOT covered here — this suite's own
  // worker backend (fixtures.ts) is shared and long-lived across every test file that runs
  // against it, so "the library currently has zero tags" is never a reliable precondition to set
  // up in an e2e test. `tests/unit/tagCloudComponent.test.tsx` covers the empty-state render
  // directly instead, with no backend involved at all.
})
