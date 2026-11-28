import { describe, expect, it } from "vitest"

import zh from "@/i18n/locales/zh.json"
import { labelAfterPreset } from "@/schedule/planLabel"

const PRESETS = ["每小时", "每天 03:00", "每周日 03:00", "每月 1 日 03:00"]

/** The real preset button texts, straight from the Chinese locale. */
const REAL_PRESETS = [
  "every-5-minutes",
  "every-15-minutes",
  "hourly",
  "every-6-hours",
  "daily",
  "weekdays",
  "weekly",
  "monthly",
  "monthly-first-weekday",
  "every-3-months-first-weekday",
  "yearly-march-first-weekday",
  "custom",
].map((slug) => (zh as Record<string, string>)[`settings.snapshotCronPreset.${slug}`])

describe("labelAfterPreset", () => {
  it("names a plan that has no name yet", () => {
    expect(labelAfterPreset("", "每天 03:00", PRESETS)).toBe("每天 03:00")
    expect(labelAfterPreset("   ", "每周日 03:00", PRESETS)).toBe("每周日 03:00")
  })

  it("replaces a name an earlier preset wrote, so switching presets renames the plan", () => {
    expect(labelAfterPreset("每天 03:00", "每周日 03:00", PRESETS)).toBe("每周日 03:00")
  })

  it("keeps a name the user wrote themselves", () => {
    expect(labelAfterPreset("我的计划", "每周日 03:00", PRESETS)).toBe("我的计划")
  })

  it("recognises a cadence-only name from an older preset text", () => {
    expect(REAL_PRESETS).toContain("每周日 03:00")
    expect(labelAfterPreset("每周日", "每小时 03:00", REAL_PRESETS)).toBe("每小时 03:00")
    expect(labelAfterPreset("每周日 03:00", "每小时 03:00", REAL_PRESETS)).toBe("每小时 03:00")
    expect(labelAfterPreset("我的计划", "每小时 03:00", REAL_PRESETS)).toBe("我的计划")
  })
})
