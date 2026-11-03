import { describe, expect, it } from "vitest"

import {
  addDays,
  bucketCount,
  bucketKey,
  bucketSeries,
  clampWindow,
  type DailyPoint,
  daysBetween,
  extendWindow,
  MAX_BUCKETS,
  mondayOf,
  panWindow,
  presetWindow,
  resolveUnit,
  zoomWindow,
} from "@/pages/Stats/timeline"

const point = (date: string, values: Partial<DailyPoint> = {}): DailyPoint => ({ date, ...values })

describe("UTC calendar arithmetic", () => {
  it("adds and subtracts whole days across month and year edges", () => {
    expect(addDays("2026-03-01", -1)).toBe("2026-02-28")
    expect(addDays("2026-12-31", 1)).toBe("2027-01-01")
    expect(addDays("2024-02-28", 1)).toBe("2024-02-29") // leap year
  })

  it("counts whole days between dates, including negative spans", () => {
    expect(daysBetween("2026-03-01", "2026-03-08")).toBe(7)
    expect(daysBetween("2026-03-08", "2026-03-01")).toBe(-7)
    expect(daysBetween("2026-03-01", "2026-03-01")).toBe(0)
  })

  it("resolves a week to its Monday, including Sunday and across a year boundary", () => {
    // 2026-03-04 is a Wednesday; 2026-03-08 a Sunday; 2027-01-01 a Friday.
    expect(mondayOf("2026-03-04")).toBe("2026-03-02")
    expect(mondayOf("2026-03-02")).toBe("2026-03-02")
    expect(mondayOf("2026-03-08")).toBe("2026-03-02")
    expect(mondayOf("2027-01-01")).toBe("2026-12-28")
  })

  it("builds bucket keys per unit without shifting the day in a negative-offset zone", () => {
    expect(bucketKey("day", "2026-01-01")).toBe("2026-01-01")
    expect(bucketKey("week", "2026-01-01")).toBe("2025-12-29")
    expect(bucketKey("month", "2026-01-31")).toBe("2026-01")
  })
})

describe("resolveUnit", () => {
  it("picks the finest unit that stays under the bucket cap", () => {
    expect(resolveUnit("auto", "2026-09-01", "2026-09-21")).toBe("day") // 21 days
    expect(resolveUnit("auto", "2026-06-01", "2026-10-01")).toBe("week") // ~18 weeks
    expect(resolveUnit("auto", "2024-01-01", "2026-10-01")).toBe("month")
  })

  it("honours a locked unit even when the window would look better coarser or finer", () => {
    expect(resolveUnit("month", "2026-09-01", "2026-09-21")).toBe("month")
    expect(resolveUnit("day", "2024-01-01", "2026-10-01")).toBe("day")
  })
})

describe("bucketCount", () => {
  it("agrees with the buckets actually emitted, at every unit", () => {
    const from = "2026-02-25"
    const to = "2026-04-03"
    for (const unit of ["day", "week", "month"] as const) {
      const counted = bucketCount(unit, from, to)
      const emitted = bucketSeries([], unit, from, to).length
      expect(counted).toBe(emitted)
    }
  })

  it("is one for a single-day window and zero for an inverted one", () => {
    expect(bucketCount("day", "2026-03-01", "2026-03-01")).toBe(1)
    expect(bucketCount("day", "2026-03-02", "2026-03-01")).toBe(0)
  })

  it("never reports more buckets than the cap for a window auto would coarsen", () => {
    expect(bucketCount("month", "2024-01-01", "2026-10-01")).toBeLessThanOrEqual(MAX_BUCKETS + 4)
  })
})

describe("bucketSeries", () => {
  const points = [
    point("2026-03-02", { archives: 2 }),
    point("2026-03-03", { archives: 3 }),
    point("2026-03-09", { archives: 1 }),
    // Outside the window — must not be counted.
    point("2026-04-01", { archives: 99 }),
  ]

  it("sums daily values into weekly buckets, keyed by Monday", () => {
    const buckets = bucketSeries(points, "week", "2026-03-02", "2026-03-15")
    expect(buckets.map((b) => b.key)).toEqual(["2026-03-02", "2026-03-09"])
    expect(buckets[0].archives).toBe(5)
    expect(buckets[0].days).toBe(2)
    expect(buckets[1].archives).toBe(1)
    expect(buckets[1].days).toBe(1)
  })

  it("carries each bucket's own day span, which is what a drill-down is queried with", () => {
    const buckets = bucketSeries([], "week", "2026-03-02", "2026-03-18")
    expect(buckets.map((b) => [b.from, b.to])).toEqual([
      ["2026-03-02", "2026-03-08"],
      ["2026-03-09", "2026-03-15"],
      ["2026-03-16", "2026-03-18"],
    ])
    // A month bucket spans whole days too, and its first days are trimmed to the window.
    const monthly = bucketSeries([], "month", "2026-03-15", "2026-04-10")
    expect(monthly.map((b) => [b.key, b.from, b.to])).toEqual([
      ["2026-03", "2026-03-15", "2026-03-31"],
      ["2026-04", "2026-04-01", "2026-04-10"],
    ])
  })

  it("zero-fills buckets with no data so a quiet stretch stays visible", () => {
    const buckets = bucketSeries([], "week", "2026-03-02", "2026-03-22")
    expect(buckets.map((b) => b.key)).toEqual(["2026-03-02", "2026-03-09", "2026-03-16"])
    expect(buckets.every((b) => b.archives === 0 && b.days === 0)).toBe(true)
  })

  it("splits the two reading series into the same buckets independently", () => {
    const buckets = bucketSeries(
      [
        point("2026-03-02", { pages: 10, last_read: 1 }),
        point("2026-03-04", { pages: 5, last_read: 2 }),
      ],
      "week",
      "2026-03-02",
      "2026-03-08",
    )
    expect(buckets).toHaveLength(1)
    expect(buckets[0].pages).toBe(15)
    expect(buckets[0].last_read).toBe(3)
  })

  it("returns nothing for an inverted range instead of spinning", () => {
    expect(bucketSeries(points, "day", "2026-03-05", "2026-03-01")).toEqual([])
  })
})

describe("window arithmetic", () => {
  const bounds = { from: "2026-01-01", to: "2026-03-31" }

  it("clamps a window to the data's own bounds", () => {
    expect(clampWindow({ from: "2025-06-01", to: "2027-01-01" }, bounds)).toEqual(bounds)
  })

  it("keeps at least one day when a window collapses", () => {
    const collapsed = clampWindow({ from: "2026-03-31", to: "2026-03-31" }, bounds)
    expect(daysBetween(collapsed.from, collapsed.to)).toBeGreaterThanOrEqual(1)
    expect(collapsed.to).toBe("2026-03-31")
  })

  it("builds presets from the end of the data, and 'all' from the start", () => {
    expect(presetWindow(7, bounds)).toEqual({ from: "2026-03-24", to: "2026-03-31" })
    expect(presetWindow(null, bounds)).toEqual(bounds)
    // A preset longer than the data clamps to the data rather than running off the front.
    expect(presetWindow(3650, bounds)).toEqual(bounds)
  })

  it("zooms in around the pointed-at day, keeping it in place", () => {
    const window = { from: "2026-03-01", to: "2026-03-31" }
    const zoomed = zoomWindow(window, 2, 0.5, bounds)
    expect(daysBetween(zoomed.from, zoomed.to)).toBe(15)
    // The midpoint (2026-03-16) stays the midpoint.
    expect(zoomed.from).toBe("2026-03-08")
    expect(zoomed.to).toBe("2026-03-23")
  })

  it("zooms out and never leaves the data's bounds", () => {
    const zoomed = zoomWindow({ from: "2026-03-25", to: "2026-03-31" }, 0.5, 1, bounds)
    expect(zoomed.to).toBe("2026-03-31")
    expect(zoomed.from >= bounds.from).toBe(true)
    expect(daysBetween(zoomed.from, zoomed.to)).toBeLessThanOrEqual(daysBetween(bounds.from, bounds.to))
  })

  it("extends a shift-clicked range forwards and backwards alike", () => {
    const anchor = { from: "2026-03-02", to: "2026-03-08" }
    expect(extendWindow(anchor, { from: "2026-03-16", to: "2026-03-22" })).toEqual({
      from: "2026-03-02",
      to: "2026-03-22",
    })
    // Backwards: the anchor stays the *later* end and the clicked bucket becomes the start.
    expect(extendWindow(anchor, { from: "2026-02-16", to: "2026-02-22" })).toEqual({
      from: "2026-02-16",
      to: "2026-03-08",
    })
    // Clicking the anchor itself (or a bucket inside the span) is a no-op, not a shrinking.
    expect(extendWindow(anchor, anchor)).toEqual(anchor)
    expect(extendWindow(anchor, { from: "2026-03-04", to: "2026-03-05" })).toEqual(anchor)
  })

  it("pans by whole days, sliding rather than shrinking at the edges", () => {
    const window = { from: "2026-03-01", to: "2026-03-11" }
    const panned = panWindow(window, 5, bounds)
    expect(panned).toEqual({ from: "2026-03-06", to: "2026-03-16" })
    // Pushing past the front keeps the width and stops at the bound.
    const pinned = panWindow(window, -100, bounds)
    expect(pinned.from).toBe(bounds.from)
    expect(daysBetween(pinned.from, pinned.to)).toBe(10)
  })
})
