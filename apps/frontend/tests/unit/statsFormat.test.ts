import { describe, expect, it } from "vitest"

import {
  formatBytes,
  formatPercent,
  formatProgress,
  formatTimestamp,
  ingestSourceLabelKey,
  percentOf,
  progressBucketLabelKey,
  progressBucketSearchQuery,
  searchURL,
  seriesColor,
  shortDay,
  shortMonth,
  sizeBucketLabelKey,
  timelineLabel,
  timelineUnitKey,
} from "@/pages/Stats/format"

describe("formatBytes", () => {
  it("uses the unit the value actually lands in", () => {
    expect(formatBytes(0)).toBe("0 B")
    expect(formatBytes(512)).toBe("512 B")
    expect(formatBytes(1024)).toBe("1.00 KB")
    expect(formatBytes(1024 * 1024)).toBe("1.00 MB")
    expect(formatBytes(1024 ** 3)).toBe("1.00 GB")
  })

  it("drops decimals once a value is large enough for them to be noise", () => {
    expect(formatBytes(150 * 1024 * 1024)).toBe("150 MB")
    expect(formatBytes(15.5 * 1024 * 1024)).toBe("15.5 MB")
  })

  it("treats a missing or nonsense size as zero rather than rendering NaN", () => {
    expect(formatBytes(Number.NaN)).toBe("0 B")
    expect(formatBytes(-1)).toBe("0 B")
  })
})

describe("percentOf / formatPercent", () => {
  it("guards a zero (or missing) denominator instead of dividing by it", () => {
    expect(percentOf(3, 0)).toBe(0)
    expect(formatPercent(3, 0)).toBe("0.0%")
    expect(percentOf(Number.NaN, 10)).toBe(0)
  })

  it("renders one decimal", () => {
    expect(formatPercent(1, 3)).toBe("33.3%")
    expect(formatPercent(1, 1)).toBe("100.0%")
  })
})

describe("translation key helpers", () => {
  it("builds the literal keys the locale files carry (i18next runs with keySeparator: false)", () => {
    expect(sizeBucketLabelKey("lt_1mb")).toBe("stats.sizeBucket.lt_1mb")
    expect(progressBucketLabelKey("unread")).toBe("stats.progressBucket.unread")
    expect(ingestSourceLabelKey("scanner")).toBe("stats.ingestSource.scanner")
  })
})

describe("shortDay / shortMonth", () => {
  it("trims the backend's UTC calendar strings without going through Date", () => {
    // A `new Date("2026-01-01")` round trip would render 2025-12-31 in any negative-offset
    // timezone — this must stay a pure string operation.
    expect(shortDay("2026-01-01")).toBe("01-01")
    expect(shortMonth("2026-01")).toBe("26-01")
  })

  it("passes anything that is not a full date through unchanged", () => {
    expect(shortDay("nonsense")).toBe("nonsense")
    expect(shortMonth("nonsense")).toBe("nonsense")
  })
})

describe("progressBucketSearchQuery", () => {
  it("maps each bucket to the exact search that reproduces it", () => {
    // Two comparisons, not the single-token range: `read:0-25` would include the unread bucket,
    // whose own progress is exactly 0.
    expect(progressBucketSearchQuery("1_20")).toBe("read:>0% read:<=20%")
    expect(progressBucketSearchQuery("21_40")).toBe("read:>20% read:<=40%")
    expect(progressBucketSearchQuery("41_60")).toBe("read:>40% read:<=60%")
    expect(progressBucketSearchQuery("61_80")).toBe("read:>60% read:<=80%")
    expect(progressBucketSearchQuery("81_99")).toBe("read:>80% read:<100%")
    expect(progressBucketSearchQuery("finished")).toBe("read:>=100%")
    expect(progressBucketSearchQuery("unread")).toBe("is:unread")
  })

  it("covers every bucket key the server can emit", () => {
    // The server's own `progress_buckets` keys — a new one there must not silently produce an
    // empty search (which would look like "no archives in this bucket").
    for (const key of ["unread", "1_20", "21_40", "41_60", "61_80", "81_99", "finished"]) {
      expect(progressBucketSearchQuery(key)).not.toBe("")
    }
    expect(progressBucketSearchQuery("nonsense")).toBe("")
  })
})

describe("searchURL", () => {
  it("builds the same library query URL the tag links use", () => {
    expect(searchURL("is:reading")).toBe("/?q=is%3Areading")
    expect(searchURL("read:>0% read:<=25%")).toBe("/?q=read%3A%3E0%25%20read%3A%3C%3D25%25")
  })
})

describe("timelineLabel / timelineUnitKey", () => {
  it("formats a bucket key by the unit the backend chose", () => {
    expect(timelineLabel("2026-03", "month")).toBe("26-03")
    expect(timelineLabel("2026-03-02", "day")).toBe("03-02")
    // A week bucket is keyed by its Monday, so it reads like a date too.
    expect(timelineLabel("2026-03-02", "week")).toBe("03-02")
  })

  it("names the unit for the sentence under the chart", () => {
    expect(timelineUnitKey("day")).toBe("stats.unit.day")
    expect(timelineUnitKey("week")).toBe("stats.unit.week")
    expect(timelineUnitKey("month")).toBe("stats.unit.month")
  })
})

describe("formatTimestamp", () => {
  it("renders an epoch-second timestamp and a dash for the never-read zero", () => {
    expect(formatTimestamp(0)).toBe("-")
    expect(formatTimestamp(1_700_000_000)).not.toBe("")
    expect(formatTimestamp(1_700_000_000)).not.toBe("-")
  })
})

describe("formatProgress", () => {
  it("includes the percentage when the page count is known, and only the page when it is not", () => {
    expect(formatProgress(50, 100)).toBe("50 / 100 (50%)")
    expect(formatProgress(3, 0)).toBe("3")
  })
})

describe("seriesColor", () => {
  it("wraps around rather than running off the end of the palette", () => {
    const palette = ["a", "b"]
    expect(seriesColor(palette, 0)).toBe("a")
    expect(seriesColor(palette, 1)).toBe("b")
    expect(seriesColor(palette, 2)).toBe("a")
  })

  it("falls back to the inherited text colour when no palette resolved", () => {
    expect(seriesColor([], 3)).toBe("currentColor")
  })
})
