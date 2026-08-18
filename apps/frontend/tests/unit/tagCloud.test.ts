import { describe, expect, it } from "vitest"

import { escapeHtml, levelFor, sphereSizeRatio } from "@/components/Display/TagCloud"

describe("levelFor", () => {
  it("maps the minimum weight to level 1 and the maximum to level 10", () => {
    expect(levelFor(1, 1, 100)).toBe(1)
    expect(levelFor(100, 1, 100)).toBe(10)
  })

  it("falls back to the middle level when every weight is equal (division would be undefined)", () => {
    expect(levelFor(5, 5, 5)).toBe(5)
  })

  it("linearly interpolates a mid-range weight", () => {
    expect(levelFor(50, 0, 100)).toBe(6)
  })
})

describe("escapeHtml", () => {
  it("escapes the four characters that could break out of an innerHTML assignment", () => {
    expect(escapeHtml("<script>alert(\"x\")</script> & co")).toBe(
      "&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; co",
    )
  })

  it("leaves ordinary tag text (including non-ASCII) untouched", () => {
    expect(escapeHtml("女性向け")).toBe("女性向け")
  })
})

describe("sphereSizeRatio", () => {
  it("returns full size (1) at and above the full-size tag count threshold", () => {
    expect(sphereSizeRatio(30)).toBe(1)
    expect(sphereSizeRatio(150)).toBe(1)
  })

  it("returns the minimum ratio for a single tag", () => {
    // 1/30 squared is tiny, so this should read back very close to the 0.3 floor.
    expect(sphereSizeRatio(1)).toBeCloseTo(0.3, 1)
  })

  it("returns the minimum ratio for zero/negative tag counts (never smaller, never a division blowup)", () => {
    expect(sphereSizeRatio(0)).toBeCloseTo(0.3)
    expect(sphereSizeRatio(-5)).toBeCloseTo(0.3)
  })

  it("stays close to the floor for a small tag count, per the squared curve's own front-loaded shrink", () => {
    // 5 tags out of a 30-tag full-size threshold: t = 5/30, ratio = 0.3 + 0.7*t^2 ≈ 0.319 — much
    // closer to the 0.3 floor than a linear curve's ~0.42 would land, which is the whole point of
    // squaring `t` (see the function's own docs).
    expect(sphereSizeRatio(5)).toBeCloseTo(0.3 + 0.7 * (5 / 30) ** 2, 5)
    expect(sphereSizeRatio(5)).toBeLessThan(0.35)
  })

  it("ramps up superlinearly as tag count approaches the full-size threshold", () => {
    const at20 = sphereSizeRatio(20)
    const at10 = sphereSizeRatio(10)
    // Doubling tagCount (10 -> 20) should more than double the *gain* above the floor, since the
    // curve is `t^2` (convex), not linear.
    expect(at20 - 0.3).toBeGreaterThan(2 * (at10 - 0.3))
  })
})
