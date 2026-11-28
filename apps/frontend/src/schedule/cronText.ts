import type { TFunction } from "i18next"
import i18n from "i18next"

/**
 * Turns a 6–7 field snapshot plan (`秒 分 时 日 月 星期 年`, the year optional — same grammar the
 * backend parses in `crates/lanrurugi-api/src/snapshots/cron.rs`) into a sentence a person can read.
 *
 * Returning `""` is the contract for "cannot describe this": the caller then shows the raw
 * expression instead. Nothing here guesses a time — an unrecognised shape is never approximated.
 */

const FIELD_COUNT = 7

type Field =
  | { kind: "any" }
  | { kind: "every"; n: number }
  | { kind: "values"; values: number[] }

interface Bounds {
  min: number
  max: number
}

const BOUNDS: Bounds[] = [
  { min: 0, max: 59 }, // seconds
  { min: 0, max: 59 }, // minutes
  { min: 0, max: 23 }, // hours
  { min: 1, max: 31 }, // days of month
  { min: 1, max: 12 }, // months
  { min: 0, max: 7 }, // weekdays (0 and 7 are both Sunday)
  { min: 1970, max: 9999 }, // years
]

function parseField(raw: string, bounds: Bounds, sundayIsSeven = false): Field | null {
  if (raw === "*" || raw === "?") return { kind: "any" }

  const every = /^\*\/(\d+)$/.exec(raw)
  if (every) {
    const n = Number(every[1])
    return n >= 1 ? { kind: "every", n } : null
  }

  const values = new Set<number>()
  for (const item of raw.split(",")) {
    if (!item) return null
    const range = /^(\d+)-(\d+)$/.exec(item)
    if (range) {
      const start = Number(range[1])
      const end = Number(range[2])
      if (start > end) return null
      for (let value = start; value <= end; value += 1) values.add(value)
      continue
    }
    // Named fields (`MON`, `JAN`) and `a/n` steps are valid for the backend but have no sentence
    // here; the raw expression is the better answer for them.
    if (!/^\d+$/.test(item)) return null
    values.add(Number(item))
  }
  for (const value of values) {
    if (value < bounds.min || value > bounds.max) return null
  }
  if (sundayIsSeven && values.has(7)) {
    values.delete(7)
    values.add(0)
  }
  return { kind: "values", values: [...values].sort((a, b) => a - b) }
}

function isAny(field: Field): boolean {
  return field.kind === "any"
}

function single(field: Field): number | null {
  return field.kind === "values" && field.values.length === 1 ? field.values[0] : null
}

function pad(value: number): string {
  return String(value).padStart(2, "0")
}

/** `HH:MM`, with seconds only when the plan actually names a non-zero one. */
function formatTime(hour: number, minute: number, second: number): string {
  const clock = `${pad(hour)}:${pad(minute)}`
  return second === 0 ? clock : `${clock}:${pad(second)}`
}

/** Current UTC offset of `tzId` in minutes, so a name probe can be placed at local noon. */
function zoneOffsetMinutes(tzId?: string): number {
  try {
    const parts = new Intl.DateTimeFormat("en-US", {
      timeZone: tzId,
      timeZoneName: "shortOffset",
    }).formatToParts(new Date())
    const zone = parts.find((part) => part.type === "timeZoneName")?.value ?? ""
    const match = /GMT([+-])(\d{1,2})(?::(\d{2}))?/.exec(zone)
    if (!match) return 0
    const sign = match[1] === "-" ? -1 : 1
    return sign * (Number(match[2]) * 60 + Number(match[3] ?? 0))
  } catch {
    return 0
  }
}

export function describeCron(cron: string, t: TFunction, tzId?: string): string {
  const parts = cron.trim().split(/\s+/).filter(Boolean)
  if (parts.length < 6 || parts.length > FIELD_COUNT) return ""
  const fields = parts.length === FIELD_COUNT ? parts : [...parts, "*"]

  // `0#1` — the n-th weekday of the month; the backend splits it off the weekday field first.
  const hash = fields[5].indexOf("#")
  const dowRaw = hash < 0 ? fields[5] : fields[5].slice(0, hash)
  const nthRaw = hash < 0 ? null : fields[5].slice(hash + 1)
  if (nthRaw !== null && !/^[1-5]$/.test(nthRaw)) return ""
  const nth = nthRaw === null ? null : Number(nthRaw)

  // A literal range is worth a sentence of its own ("周一至周五"), and `6-0` wraps past Sunday in a
  // way `parseField` rejects, so it is read here before the fields are expanded.
  const rangeMatch = /^(\d+)-(\d+)$/.exec(dowRaw)
  const weekdayRange = rangeMatch
    ? (() => {
        const rawFrom = Number(rangeMatch[1])
        const rawTo = Number(rangeMatch[2])
        const span = rawFrom <= rawTo ? rawTo - rawFrom + 1 : 8 - rawFrom + rawTo
        return { from: rawFrom % 7, to: rawTo % 7, span, wrapped: rawFrom > rawTo, rawFrom, rawTo }
      })()
    : null

  const parseable = [...fields]
  parseable[5] = dowRaw
  if (weekdayRange?.wrapped) {
    // `6-0` is `6,0` once expanded; parsing the split form keeps every later check unchanged.
    const days: number[] = []
    for (let day = weekdayRange.rawFrom; day <= 6; day += 1) days.push(day)
    for (let day = 0; day <= weekdayRange.rawTo; day += 1) days.push(day)
    parseable[5] = days.join(",")
  }
  const parsed = parseable.map((raw, index) => parseField(raw, BOUNDS[index], index === 5))
  if (parsed.some((field) => field === null)) return ""
  const [sec, min, hour, dom, mon, dow, year] = parsed as Field[]

  const locale = i18n.language || "en"
  const offsetMs = zoneOffsetMinutes(tzId) * 60_000
  const names = (options: Intl.DateTimeFormatOptions, instant: Date) => {
    try {
      return new Intl.DateTimeFormat(locale, { ...options, timeZone: tzId }).format(instant)
    } catch {
      // An empty or unknown zone id must not blank the whole sentence.
      return new Intl.DateTimeFormat(locale, options).format(instant)
    }
  }
  // The locale's own weekday words (周一/週一/月曜日/Monday) instead of `Intl`'s long names: those
  // read "星期日"/"日曜日", which cannot follow the "每" the sentence templates start with.
  const WEEKDAY_KEYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as const
  const weekdayName = (dayOfWeek: number) =>
    t(`settings.cronWeekday${WEEKDAY_KEYS[((dayOfWeek % 7) + 7) % 7]}`)
  // Chinese and Japanese write months as digits, which is also how the preset buttons word them
  // ("每年 3 月…", "毎年3月…"); other locales keep `Intl`'s long name ("March").
  const monthName = (month: number) => {
    // zh keeps just the digit so a list reads "3、6 月" (the templates supply the 月); ja glues the
    // unit on per month, which is how "3月、6月" is written.
    if (/^zh/i.test(locale)) return String(month)
    if (/^ja/i.test(locale)) return `${month}月`
    return names({ month: "long" }, new Date(Date.UTC(2024, month - 1, 15, 12) - offsetMs))
  }

  const list = (values: number[], format: (value: number) => string) => {
    const separator = t("settings.cronListSeparator")
    return values.map(format).join(separator)
  }

  const restAny = (from: number) => parsed.slice(from).every((field) => isAny(field as Field))
  const zeroOrAny = (field: Field) => isAny(field) || single(field) === 0

  // Every second / every N seconds, with nothing else narrowing it.
  if (restAny(1)) {
    if (isAny(sec)) return t("settings.cronEverySecond")
    if (sec.kind === "every") return t("settings.cronEverySeconds", { n: sec.n })
    if (single(sec) === 0) return t("settings.cronEveryMinute")
    return ""
  }

  // Every minute / every N minutes.
  if (restAny(2) && zeroOrAny(sec)) {
    if (min.kind === "every") return t("settings.cronEveryMinutes", { n: min.n })
    if (single(min) === 0) return t("settings.cronEveryHour")
  }

  // Every N hours.
  if (restAny(3) && zeroOrAny(sec) && zeroOrAny(min)) {
    if (hour.kind === "every") return t("settings.cronEveryHours", { n: hour.n })
  }

  const second = single(sec)
  const minute = single(min)
  const clockHour = single(hour)

  // Every N days, at midnight or at the plan's own time of day.
  if (restAny(4) && dom.kind === "every") {
    if (second !== null && minute !== null && clockHour !== null) {
      const at = formatTime(clockHour, minute, second)
      return second === 0 && minute === 0 && clockHour === 0
        ? t("settings.cronEveryDays", { n: dom.n })
        : t("settings.cronEveryDaysAt", { n: dom.n, time: at })
    }
    if (zeroOrAny(sec) && zeroOrAny(min) && zeroOrAny(hour)) {
      return t("settings.cronEveryDays", { n: dom.n })
    }
    return ""
  }

  if (second === null || minute === null || clockHour === null) return ""
  const time = formatTime(clockHour, minute, second)
  const months = mon.kind === "values" ? mon.values : null
  const monthList = months ? list(months, monthName) : ""

  // `每月第 N 个周X`, and the same shape narrowed to every N months or to named months.
  if (nth !== null) {
    const dayOfWeek = single(dow)
    if (dayOfWeek === null || !isAny(dom) || !isAny(year)) return ""
    const nthLabel = t(`settings.cronNth${nth}`)
    const weekday = weekdayName(dayOfWeek)
    if (isAny(mon)) return t("settings.cronMonthlyNth", { nth: nthLabel, weekday, time })
    if (mon.kind === "every") {
      return t("settings.cronEveryMonthsNth", { n: mon.n, nth: nthLabel, weekday, time })
    }
    if (months) return t("settings.cronYearlyNth", { months: monthList, nth: nthLabel, weekday, time })
    return ""
  }

  const days = dom.kind === "values" ? dom.values : null
  const weekdays = dow.kind === "values" ? dow.values : null
  const monthly = days ? list(days, (day) => String(day)) : ""
  const weekly = weekdays ? list(weekdays, weekdayName) : ""

  if (!days && weekdays && isAny(year) && months) {
    return t("settings.cronYearlyWeekly", { months: monthList, weekdays: weekly, time })
  }
  if (!days && weekdays && isAny(mon) && isAny(year)) {
    // A range covering the whole week fires every day, which is shorter to say than "周一至周日".
    if (weekdayRange && weekdayRange.span >= 7) return t("settings.cronDaily", { time })
    // Three or more consecutive days read better as a range; two are listed ("周一、周三"). A range
    // that wraps past Sunday keeps its own wording ("周六至周日") — the list form would hide the wrap.
    if (weekdayRange && (weekdayRange.wrapped || weekdayRange.span >= 3)) {
      return t("settings.cronWeeklyRange", {
        from: weekdayName(weekdayRange.from),
        to: weekdayName(weekdayRange.to),
        time,
      })
    }
    return t("settings.cronWeekly", { days: weekly, time })
  }
  if (days && !weekdays && isAny(mon) && isAny(year)) {
    return t("settings.cronMonthly", { days: monthly, time })
  }
  if (days && !weekdays && isAny(year) && months) {
    // One month and one day keep the older, shorter wording; lists get the general form.
    if (months.length === 1 && days.length === 1) {
      return t("settings.cronYearly", { month: monthName(months[0]), day: String(days[0]), time })
    }
    return t("settings.cronYearlyDate", { months: monthList, days: monthly, time })
  }
  // Both fixed: cron fires on either condition, so the sentence joins them rather than implying
  // "a Monday that is also the 1st".
  if (days && weekdays && isAny(mon) && isAny(year)) {
    return t("settings.cronMonthlyAndWeekly", { days: monthly, weekdays: weekly, time })
  }
  if (!days && !weekdays && isAny(mon) && isAny(year)) {
    return t("settings.cronDaily", { time })
  }
  return ""
}
