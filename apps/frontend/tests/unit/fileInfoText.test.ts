import { describe, expect, it } from "vitest"

import { fileInfoText } from "@/lib/utils/fileInfoText"

const ORIGIN = "http://localhost:3000"

function pageUrl(name: string) {
  return `${ORIGIN}/api/archives/abc/files/thumbnail?path=${encodeURIComponent(name)}`
}

describe("fileInfoText", () => {
  it("formats a single page with known dimensions and size", () => {
    const urls = [pageUrl("page1.jpg")]
    const result = fileInfoText(urls, { left: 1, right: null }, { 1: { width: 800, height: 1200 } }, { 1: 245 }, ORIGIN)
    expect(result).toBe("page1.jpg :: 800 x 1200 :: 245 KB")
  })

  it("falls back to just the filename when dimensions are not yet known", () => {
    const urls = [pageUrl("page1.jpg")]
    const result = fileInfoText(urls, { left: 1, right: null }, {}, {}, ORIGIN)
    expect(result).toBe("page1.jpg")
  })

  it("formats a double-page spread with combined width and matching height", () => {
    const urls = [pageUrl("page1.jpg"), pageUrl("page2.jpg")]
    const result = fileInfoText(
      urls,
      { left: 1, right: 2 },
      { 1: { width: 800, height: 1200 }, 2: { width: 750, height: 1200 } },
      { 1: 245, 2: 210 },
      ORIGIN,
    )
    expect(result).toBe("page1.jpg - page2.jpg :: 1550 x 1200 :: 455 KB")
  })

  it("falls back to just the two filenames when a spread has no dimensions yet", () => {
    const urls = [pageUrl("page1.jpg"), pageUrl("page2.jpg")]
    const result = fileInfoText(urls, { left: 1, right: 2 }, {}, {}, ORIGIN)
    expect(result).toBe("page1.jpg - page2.jpg")
  })
})
