import type { TFunction } from "i18next"
import i18n from "i18next"
import { beforeAll, describe, expect, it } from "vitest"

import en from "@/i18n/locales/en.json"
import zh from "@/i18n/locales/zh.json"
import { describeCron } from "@/schedule/cronText"

// The real locale files, not a stub: the sentences are what the page renders, so a missing or
// mistranslated key has to fail here rather than show up as a fallback string in the UI.
beforeAll(async () => {
  await i18n.init({
    lng: "zh",
    fallbackLng: "en",
    resources: { en: { translation: en }, zh: { translation: zh } },
    interpolation: { escapeValue: false },
  })
})

const t = i18n.t.bind(i18n) as unknown as TFunction
const TZ = "Asia/Tokyo"

/** Runs one assertion under another language, then puts the suite back on Chinese. */
async function inLanguage<T>(language: string, run: () => T): Promise<T> {
  await i18n.changeLanguage(language)
  try {
    return run()
  } finally {
    await i18n.changeLanguage("zh")
  }
}

describe("describeCron", () => {
  it("every second and every-N step expressions", async () => {
    expect(describeCron("* * * * * * *", t, TZ)).toBe("每秒")
    expect(describeCron("*/15 * * * * * *", t, TZ)).toBe("每 15 秒")
    expect(describeCron("0 */5 * * * * *", t, TZ)).toBe("每 5 分钟")
    expect(describeCron("0 0 */6 * * * *", t, TZ)).toBe("每 6 小时")
    expect(describeCron("0 0 0 */2 * * *", t, TZ)).toBe("每 2 天")
    expect(describeCron("0 0 3 */2 * * *", t, TZ)).toBe("每 2 天 03:00")
    expect(await inLanguage("en", () => describeCron("*/15 * * * * * *", t, TZ))).toBe(
      "Every 15 seconds",
    )
    expect(await inLanguage("en", () => describeCron("* * * * * * *", t, TZ))).toBe(
      "Every second",
    )
  })

  it("a fixed time of day", async () => {
    expect(describeCron("0 0 3 * * * *", t, TZ)).toBe("每天 03:00")
    expect(describeCron("30 0 3 * * * *", t, TZ)).toBe("每天 03:00:30")
    expect(await inLanguage("en", () => describeCron("0 0 3 * * * *", t, TZ))).toBe(
      "Every day at 03:00",
    )
  })

  it("a weekday, a weekday list and a weekday range", async () => {
    expect(describeCron("0 0 3 * * 0 *", t, TZ)).toBe("每周日 03:00")
    expect(describeCron("0 0 3 * * 1,3 *", t, TZ)).toBe("每周一、周三 03:00")
    // Three or more consecutive days are a range; two stay a list.
    expect(describeCron("0 30 9 * * 1-5", t, TZ)).toBe("每周一至周五 09:30")
    expect(describeCron("0 0 3 * * 1-3 *", t, TZ)).toBe("每周一至周三 03:00")
    expect(describeCron("0 0 3 * * 1-2 *", t, TZ)).toBe("每周一、周二 03:00")
    // A range covering the whole week is just "every day", and `6-0` wraps past Sunday.
    expect(describeCron("0 0 3 * * 0-6 *", t, TZ)).toBe("每天 03:00")
    expect(describeCron("0 0 3 * * 6-0 *", t, TZ)).toBe("每周六至周日 03:00")
    expect(describeCron("0 0 3 * * 5-1 *", t, TZ)).toBe("每周五至周一 03:00")
    expect(await inLanguage("en", () => describeCron("0 0 3 * * 0 *", t, TZ))).toBe(
      "Every Sunday at 03:00",
    )
    expect(await inLanguage("en", () => describeCron("0 0 3 * * 1,3 *", t, TZ))).toBe(
      "Every Monday, Wednesday at 03:00",
    )
  })

  it("a day of the month", async () => {
    expect(describeCron("0 30 9 1 * * *", t, TZ)).toBe("每月 1 日 09:30")
    expect(describeCron("0 30 9 1,15 * * *", t, TZ)).toBe("每月 1、15 日 09:30")
    expect(await inLanguage("en", () => describeCron("0 30 9 1 * * *", t, TZ))).toBe(
      "Day 1 of every month at 09:30",
    )
  })

  it("the n-th weekday of the month", async () => {
    expect(describeCron("0 0 3 * * 0#1 *", t, TZ)).toBe("每月第一个周日 03:00")
    expect(describeCron("0 0 3 * * 1#2 *", t, TZ)).toBe("每月第二个周一 03:00")
    // The expression's own meaning, whether or not the month happens to be that long.
    expect(describeCron("0 0 3 * * 0#5 *", t, TZ)).toBe("每月第五个周日 03:00")
    expect(await inLanguage("en", () => describeCron("0 0 3 * * 0#1 *", t, TZ))).toBe(
      "The first Sunday of every month at 03:00",
    )
    expect(await inLanguage("en", () => describeCron("0 0 3 * * 1#2 *", t, TZ))).toBe(
      "The second Monday of every month at 03:00",
    )
  })

  it("a month and a day", async () => {
    expect(describeCron("0 0 3 1 3 * *", t, TZ)).toBe("每年 3 月 1 日 03:00")

    // The month field narrows the same shapes: a step, one or more named months.
    expect(describeCron("0 0 3 * */3 0#1 *", t, TZ)).toBe("每 3 个月的第一个周日 03:00")
    expect(describeCron("0 0 0 * 3 0#1 *", t, TZ)).toBe("每年 3 月的第一个周日 00:00")
    expect(describeCron("0 0 3 * 3,6 0#1 *", t, TZ)).toBe("每年 3、6 月的第一个周日 03:00")
    expect(describeCron("0 0 0 15 3 * *", t, TZ)).toBe("每年 3 月 15 日 00:00")
    expect(describeCron("0 0 3 * 3,9 0 *", t, TZ)).toBe("每年 3、9 月的每周日 03:00")

    expect(await inLanguage("en", () => describeCron("0 0 3 * */3 0#1 *", t, TZ))).toBe(
      "The first Sunday of every 3 months at 03:00",
    )
    expect(await inLanguage("en", () => describeCron("0 0 0 * 3 0#1 *", t, TZ))).toBe(
      "The first Sunday of March every year at 00:00",
    )
    expect(await inLanguage("en", () => describeCron("0 0 0 15 3 * *", t, TZ))).toBe(
      "Every year on March 15 at 00:00",
    )
    expect(await inLanguage("en", () => describeCron("0 0 3 * 3,9 0 *", t, TZ))).toBe(
      "Every Sunday in March, September at 03:00",
    )
    expect(await inLanguage("en", () => describeCron("0 0 3 1 3 * *", t, TZ))).toBe(
      "Every year on March 1 at 03:00",
    )
  })

  it("a day of the month together with a weekday is a union, not an intersection", async () => {
    expect(describeCron("0 0 3 1 * 1 *", t, TZ)).toBe("每月 1 日与每周一 03:00")
    expect(await inLanguage("en", () => describeCron("0 0 3 1 * 1 *", t, TZ))).toBe(
      "Day 1 of every month and every Monday at 03:00",
    )
  })

  it("returns an empty string for shapes it cannot put into words", async () => {
    // A named weekday is valid for the backend but has no sentence here.
    expect(describeCron("0 0 3 * * MON *", t, TZ)).toBe("")
    // A fixed year narrows the plan in a way none of the sentences express.
    expect(describeCron("0 0 3 * * * 2027", t, TZ)).toBe("")
    expect(describeCron("not a cron", t, TZ)).toBe("")
    expect(describeCron("", t, TZ)).toBe("")
  })

  it("still reads the six field form", () => {
    expect(describeCron("0 0 3 * * 0", t, TZ)).toBe("每周日 03:00")
  })

  // The weekday words come from the locale table, so each language reads naturally.
  it("names weekdays in the active language", async () => {
    expect(await inLanguage("en", () => describeCron("0 0 3 * * 6-0 *", t, TZ))).toBe(
      "Every Saturday through Sunday at 03:00",
    )
    expect(await inLanguage("en", () => describeCron("0 0 3 * * 0 *", t, TZ))).toBe(
      "Every Sunday at 03:00",
    )
  })

  // The page's preset chips (Backup.tsx's CRON_PRESETS): every one of them has to produce a
  // sentence, or the chip offers a shape the row then cannot describe.
  it("describes every preset the page offers", () => {
    const presets = [
      "0 */5 * * * * *",
      "0 */15 * * * * *",
      "0 0 * * * * *",
      "0 0 */6 * * * *",
      "0 0 3 * * * *",
      "0 0 3 * * 1-5 *",
      "0 0 3 * * 0 *",
      "0 0 3 1 * * *",
      "0 0 3 * * 0#1 *",
    ]
    for (const cron of presets) {
      expect(describeCron(cron, t, TZ), cron).not.toBe("")
    }
  })
})
