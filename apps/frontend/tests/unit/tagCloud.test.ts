import { describe, expect, it } from "vitest"

import { densityScale, escapeHtml, fontSizeScale, levelFor, sphereSizeRatio } from "@/components/Display/TagCloud"

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

describe("densityScale", () => {
  it("returns full density (1) at and above the full-width threshold", () => {
    expect(densityScale(480)).toBe(1)
    expect(densityScale(960)).toBe(1)
  })

  it("does not shrink an ordinary desktop-height-constrained container", () => {
    // A live-reported regression: `#tagCloud`'s own `maxHeight: 70vh` (Stats.tsx) means a normal
    // desktop browser window under ~800px tall gives this component a container whose *short
    // side* is height-, not width-, constrained — e.g. 560px tall on a 1280x800 viewport. An
    // earlier, higher threshold (640) treated that as "small" and shrank density on an entirely
    // ordinary desktop window; 480 must sit below that common case.
    expect(densityScale(560)).toBe(1)
  })

  it("returns the floor ratio at zero/negative widths", () => {
    expect(densityScale(0)).toBeCloseTo(0.4)
    expect(densityScale(-100)).toBeCloseTo(0.4)
  })

  it("interpolates linearly between the floor and full-width threshold", () => {
    // Halfway across the 0-480 range (240px) should read back halfway between 0.4 and 1.0.
    expect(densityScale(240)).toBeCloseTo(0.4 + (1 - 0.4) * 0.5, 5)
  })

  it("shrinks a real phone-width container well below full density", () => {
    // ~306px is this project's own live-measured `#tagCloud` container width on a 390px-wide
    // phone viewport (80% of 390, minus layout chrome) — the concrete case this was added for.
    expect(densityScale(306)).toBeLessThan(0.85)
    expect(densityScale(306)).toBeGreaterThan(0.4)
  })
})

describe("fontSizeScale", () => {
  it("mirrors densityScale's own thresholds but with its own 0.7 floor", () => {
    expect(fontSizeScale(480)).toBe(1)
    expect(fontSizeScale(0)).toBeCloseTo(0.7)
    expect(fontSizeScale(-50)).toBeCloseTo(0.7)
  })

  it("does not shrink an ordinary desktop-height-constrained container", () => {
    expect(fontSizeScale(560)).toBe(1)
  })

  it("interpolates linearly between its own floor and the full-width threshold", () => {
    expect(fontSizeScale(240)).toBeCloseTo(0.7 + (1 - 0.7) * 0.5, 5)
  })
})
