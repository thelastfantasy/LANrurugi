import type { TFunction } from "i18next"
import i18n from "i18next"
import { beforeAll, describe, expect, it } from "vitest"

import en from "@/i18n/locales/en.json"
import zh from "@/i18n/locales/zh.json"
import { absoluteTimestamp, formatRelativeTime } from "@/lib/relativeTime"

// Real locale files: a missing `time.*` key must fail here, not render as a raw key on the page.
beforeAll(async () => {
  await i18n.init({
    lng: "zh",
    fallbackLng: "en",
    resources: { en: { translation: en }, zh: { translation: zh } },
    interpolation: { escapeValue: false },
  })
})

const t = i18n.t.bind(i18n) as unknown as TFunction

const NOW = new Date("2026-11-24T12:00:00Z")

/** `NOW` minus `ms`, as the ISO string the API hands over. */
function ago(ms: number): string {
  return new Date(NOW.getTime() - ms).toISOString()
}

const SECOND = 1000
const MINUTE = 60 * SECOND
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

async function inLanguage<T>(language: string, run: () => T): Promise<T> {
  await i18n.changeLanguage(language)
  try {
    return run()
  } finally {
    await i18n.changeLanguage("zh")
  }
}

describe("formatRelativeTime", () => {
  it("calls anything under a minute just now", async () => {
    expect(formatRelativeTime(ago(30 * SECOND), t, "zh-CN", NOW)).toBe("刚刚")
    await inLanguage("en", () => {
      expect(formatRelativeTime(ago(30 * SECOND), t, "en-US", NOW)).toBe("just now")
    })
  })

  it("counts minutes", async () => {
    expect(formatRelativeTime(ago(5 * MINUTE), t, "zh-CN", NOW)).toBe("5 分钟前")
    await inLanguage("en", () => {
      expect(formatRelativeTime(ago(5 * MINUTE), t, "en-US", NOW)).toBe("5 minutes ago")
    })
  })

  it("counts hours", async () => {
    expect(formatRelativeTime(ago(3 * HOUR), t, "zh-CN", NOW)).toBe("3 小时前")
    await inLanguage("en", () => {
      expect(formatRelativeTime(ago(3 * HOUR), t, "en-US", NOW)).toBe("3 hours ago")
    })
  })

  it("counts days up to a week", async () => {
    expect(formatRelativeTime(ago(2 * DAY), t, "zh-CN", NOW)).toBe("2 天前")
    await inLanguage("en", () => {
      expect(formatRelativeTime(ago(2 * DAY), t, "en-US", NOW)).toBe("2 days ago")
    })
  })

  it("uses the singular form for exactly one unit in English", async () => {
    await inLanguage("en", () => {
      expect(formatRelativeTime(ago(MINUTE), t, "en-US", NOW)).toBe("1 minute ago")
      expect(formatRelativeTime(ago(HOUR), t, "en-US", NOW)).toBe("1 hour ago")
      expect(formatRelativeTime(ago(DAY), t, "en-US", NOW)).toBe("1 day ago")
    })
  })

  it("falls back to a short absolute stamp once it is older than a week", () => {
    const label = formatRelativeTime(ago(30 * DAY), t, "zh-CN", NOW)
    // Timezone-independent: the shape and the absence of the (still current) year are the assertions.
    expect(label).toMatch(/^\d{2}\/\d{2} \d{2}:\d{2}$/)
  })

  it("carries the year once the stamp is from another year", () => {
    const label = formatRelativeTime(ago(400 * DAY), t, "zh-CN", NOW)
    expect(label).toMatch(/^\d{4}\/\d{2}\/\d{2} \d{2}:\d{2}$/)
  })

  it("leaves an unparseable stamp alone", () => {
    expect(formatRelativeTime("not a date", t, "zh-CN", NOW)).toBe("not a date")
  })
})

describe("absoluteTimestamp", () => {
  it("is the full instant kept in the tooltip", () => {
    expect(absoluteTimestamp(ago(2 * DAY), "zh-CN")).toMatch(/^\d{4}\/\d{2}\/\d{2} \d{2}:\d{2}:\d{2}$/)
  })
})
