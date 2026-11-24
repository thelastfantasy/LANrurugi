/**
 * Shared relative-time formatting: "just now" / "5 minutes ago" for a recent instant, the short
 * absolute stamp once it is a week old, and the exact instant a `title` shows on hover. Pure —
 * takes the clock (`now`) instead of reading it, so every caller and every test agrees.
 */
import type { TFunction } from "i18next"

const SECOND = 1000
const MINUTE = 60 * SECOND
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR
const WEEK = 7 * DAY

/** Locale-aware parts, falling back to English for a tag `Intl` refuses. */
function partsOf(date: Date, locale: string, options: Intl.DateTimeFormatOptions) {
  try {
    return new Intl.DateTimeFormat(locale, options).formatToParts(date)
  } catch {
    return new Intl.DateTimeFormat("en", options).formatToParts(date)
  }
}

function pick(parts: Intl.DateTimeFormatPart[], type: Intl.DateTimeFormatPartTypes): string {
  return parts.find((part) => part.type === type)?.value ?? ""
}

/** `MM/DD HH:MM`, carrying the year only once it stops being the current one — the same rule the
 *  schedule labels on this page follow. Used for anything older than a week. */
function shortAbsolute(date: Date, locale: string, now: Date): string {
  const parts = partsOf(date, locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  })
  const sameYear = date.getFullYear() === now.getFullYear()
  const day = sameYear
    ? `${pick(parts, "month")}/${pick(parts, "day")}`
    : `${pick(parts, "year")}/${pick(parts, "month")}/${pick(parts, "day")}`
  return `${day} ${pick(parts, "hour")}:${pick(parts, "minute")}`
}

/** The full stamp an element carries in its `title`, so a relative label stays checkable:
 *  `YYYY/MM/DD HH:MM:SS`. */
export function absoluteTimestamp(iso: string, locale: string): string {
  const date = new Date(iso)
  if (Number.isNaN(date.getTime())) return iso
  const parts = partsOf(date, locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  })
  return (
    `${pick(parts, "year")}/${pick(parts, "month")}/${pick(parts, "day")} ` +
    `${pick(parts, "hour")}:${pick(parts, "minute")}:${pick(parts, "second")}`
  )
}

/**
 * "just now" / "5 minutes ago" for a recent instant, and the short absolute stamp once it is a week
 * old. `now` is injectable so the buckets stay testable without waiting.
 */
export function formatRelativeTime(
  iso: string,
  t: TFunction,
  locale: string,
  now: Date = new Date(),
): string {
  const date = new Date(iso)
  // An unparseable stamp is shown as it arrived rather than as "invalid date".
  if (Number.isNaN(date.getTime())) return iso

  const elapsed = now.getTime() - date.getTime()
  // Future instants (clock skew, a timestamp used for "next run") read better as an absolute time.
  if (elapsed < 0) return shortAbsolute(date, locale, now)

  if (elapsed < MINUTE) return t("time.justNow")
  if (elapsed < HOUR) {
    const minutes = Math.floor(elapsed / MINUTE)
    return t("time.minutesAgo", { count: minutes, n: minutes })
  }
  if (elapsed < DAY) {
    const hours = Math.floor(elapsed / HOUR)
    return t("time.hoursAgo", { count: hours, n: hours })
  }
  if (elapsed < WEEK) {
    const days = Math.floor(elapsed / DAY)
    return t("time.daysAgo", { count: days, n: days })
  }
  return shortAbsolute(date, locale, now)
}
