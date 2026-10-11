import { describe, expect, it } from "vitest"

import type { CropAlignment } from "@/api/types"
import { computeEdgeBands, computePadGeometry, needsSyntheticPad } from "@/pages/Upload/AlignmentBandOverlay"

const IDENTITY: CropAlignment = { scale: 1, offset_x: 0, offset_y: 0 }

describe("computeEdgeBands", () => {
  it("is all-zero for an identity alignment on either side", () => {
    expect(computeEdgeBands("a", IDENTITY)).toEqual({ top: 0, bottom: 0, left: 0, right: 0 })
    expect(computeEdgeBands("b", IDENTITY)).toEqual({ top: 0, bottom: 0, left: 0, right: 0 })
  })

  it("marks B's own border when B's content occupies a smaller, offset sub-rect of its own frame", () => {
    // A's full [0,1] frame maps into B's [0.05, 0.85] range on each axis (scale 0.8, offset 0.05)
    // — i.e. B has an 8%-ish margin on the top/left edge and a bigger one on bottom/right.
    const alignment: CropAlignment = { scale: 0.8, offset_x: 0.05, offset_y: 0.05 }
    const bBands = computeEdgeBands("b", alignment)
    expect(bBands.left).toBeCloseTo(0.05)
    expect(bBands.top).toBeCloseTo(0.05)
    expect(bBands.right).toBeCloseTo(1 - 0.85)
    expect(bBands.bottom).toBeCloseTo(1 - 0.85)
  })

  it("marks no border on A's own side for the same alignment — A's whole frame is matched content", () => {
    const alignment: CropAlignment = { scale: 0.8, offset_x: 0.05, offset_y: 0.05 }
    const aBands = computeEdgeBands("a", alignment)
    expect(aBands).toEqual({ top: 0, bottom: 0, left: 0, right: 0 })
  })

  it("handles an asymmetric offset producing uneven band widths per edge", () => {
    // Same scale (0.9) but a much larger offset than (1-scale) allows on the low end — pushes the
    // valid B range toward the high end of [0,1], producing a thick left/top band and a thin (or
    // zero) right/bottom band.
    const alignment: CropAlignment = { scale: 0.9, offset_x: 0.08, offset_y: 0.02 }
    const bBands = computeEdgeBands("b", alignment)
    expect(bBands.left).toBeCloseTo(0.08)
    expect(bBands.top).toBeCloseTo(0.02)
    expect(bBands.right).toBeCloseTo(1 - 0.98)
    expect(bBands.bottom).toBeCloseTo(1 - 0.92)
  })

  it("clamps to zero rather than going negative when the valid range already covers the whole frame", () => {
    // A negative offset / scale > 1 combination where B's own frame is entirely within the
    // matched region (no border at all) must not produce a negative band width.
    const alignment: CropAlignment = { scale: 1.2, offset_x: -0.05, offset_y: -0.05 }
    const bBands = computeEdgeBands("b", alignment)
    expect(bBands.left).toBe(0)
    expect(bBands.top).toBe(0)
  })
})

describe("computeEdgeBands as real-pixel offsets (AlignmentBandOverlay's own use)", () => {
  it("converts B's own border fractions into real pixel offsets against B's own resolution", () => {
    // B's frame is 1000x1000 real pixels; content occupies [100, 900] on X (10% border each
    // side, scale 0.8) and [50, 950] on Y (5% border each side, scale 0.9) — matching an
    // unscaled paste of an 800x900 content image onto a bigger, bordered canvas.
    const bWidth = 1000;
    const bHeight = 1000;
    const alignment: CropAlignment = { scale: 0.8, offset_x: 0.1, offset_y: 0.05 }
    const bBands = computeEdgeBands("b", alignment)
    expect(bBands.left * bWidth).toBeCloseTo(100)
    expect(bBands.right * bWidth).toBeCloseTo(100)
    expect(bBands.top * bHeight).toBeCloseTo(50)
    // scale 0.8 + offset 0.05 means content spans Y [0.05, 0.85] — an asymmetric 5%/15% split,
    // not 5%/5%; the bottom band is the complement of the high end, 1 - 0.85 = 0.15.
    expect(bBands.bottom * bHeight).toBeCloseTo(150)
  })
})

describe("needsSyntheticPad", () => {
  it("is false for an identity alignment regardless of resolution difference", () => {
    expect(needsSyntheticPad(IDENTITY, 1000, 1000, 2000, 2000)).toBe(false)
  })

  it("is true for the smaller side against a real, estimated (non-exact) alignment", () => {
    // The exact real case that exposed the original bug: a search-ESTIMATED alignment is almost
    // never precisely `scale: 1, offset: 0` even for the genuinely border-free side (here,
    // A: 1057x1500 vs B: 1151x1680, with B really being the bordered/larger one) — pixel-count
    // comparison must still correctly identify A as needing the pad despite that imprecision.
    const alignment: CropAlignment = { scale: 1.008, offset_x: 0.0304, offset_y: 0.0625 }
    expect(needsSyntheticPad(alignment, 1057, 1500, 1151, 1680)).toBe(true)
    // And B (the larger side) must NOT get a pad against A's own smaller resolution.
    expect(needsSyntheticPad(alignment, 1151, 1680, 1057, 1500)).toBe(false)
  })

  it("is false when this side isn't smaller than the other, even with a non-identity alignment", () => {
    const alignment: CropAlignment = { scale: 0.8, offset_x: 0.05, offset_y: 0.05 }
    expect(needsSyntheticPad(alignment, 1000, 1000, 1000, 1000)).toBe(false)
  })

  it("is false when either side's resolution is zero (not yet known)", () => {
    const alignment: CropAlignment = { scale: 0.8, offset_x: 0.05, offset_y: 0.05 }
    expect(needsSyntheticPad(alignment, 0, 0, 1000, 1000)).toBe(false)
  })
})

describe("computePadGeometry", () => {
  it("matches computeEdgeBands(otherSide, ...) exactly — offsetX/Y is the other side's own left/top band", () => {
    const alignment: CropAlignment = { scale: 0.8, offset_x: 0.1, offset_y: 0.05 }
    const pad = computePadGeometry("a", alignment)
    const otherBands = computeEdgeBands("b", alignment)
    expect(pad.offsetX).toBeCloseTo(otherBands.left)
    expect(pad.offsetY).toBeCloseTo(otherBands.top)
    expect(pad.contentWidth).toBeCloseTo(1 - otherBands.left - otherBands.right)
    expect(pad.contentHeight).toBeCloseTo(1 - otherBands.top - otherBands.bottom)
  })
})
