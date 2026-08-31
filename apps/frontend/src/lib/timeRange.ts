function nowPartsInTimezone(timezone: string): { year: number; month: number; day: number; weekday: number } {
  const options: Intl.DateTimeFormatOptions = {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    weekday: "short",
    timeZone: timezone || undefined,
  }
  const parts = new Intl.DateTimeFormat("en-US", options).formatToParts(new Date())
  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? ""
  const weekdayNames = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
  return {
    year: Number(get("year")),
    month: Number(get("month")),
    day: Number(get("day")),
    weekday: weekdayNames.indexOf(get("weekday")),
  }
}

function midnightSecondsFor(year: number, month: number, day: number, timezone: string): number {
  const guessUtc = Date.UTC(year, month - 1, day, 0, 0, 0)
  const rendered = new Intl.DateTimeFormat("en-US", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
    timeZone: timezone || undefined,
  }).formatToParts(new Date(guessUtc))
  const get = (type: string) => rendered.find((p) => p.type === type)?.value ?? "0"
  const renderedUtcMs = Date.UTC(
    Number(get("year")),
    Number(get("month")) - 1,
    Number(get("day")),
    Number(get("hour")),
    Number(get("minute")),
    Number(get("second")),
  )
  const offsetMs = renderedUtcMs - guessUtc
  return Math.floor((guessUtc - offsetMs) / 1000)
}

export type TimeRangePreset = "last_hour" | "today" | "this_week" | "this_month"

/** `[start, end)` Unix-seconds bounds for `preset`. `this_week` starts Monday. */
export function presetTimeRange(preset: TimeRangePreset, timezone: string): { start: number; end: number } {
  const nowSeconds = Math.floor(Date.now() / 1000)
  if (preset === "last_hour") {
    return { start: nowSeconds - 3600, end: nowSeconds }
  }
  const { year, month, day, weekday } = nowPartsInTimezone(timezone)
  const todayStart = midnightSecondsFor(year, month, day, timezone)
  if (preset === "today") {
    return { start: todayStart, end: nowSeconds }
  }
  if (preset === "this_week") {
    const daysSinceMonday = weekday === 0 ? 6 : weekday - 1
    const mondayDate = new Date(todayStart * 1000 - daysSinceMonday * 86400 * 1000)
    const weekStart = midnightSecondsFor(
      mondayDate.getUTCFullYear(),
      mondayDate.getUTCMonth() + 1,
      mondayDate.getUTCDate(),
      timezone,
    )
    return { start: weekStart, end: nowSeconds }
  }
  // this_month
  const monthStart = midnightSecondsFor(year, month, 1, timezone)
  return { start: monthStart, end: nowSeconds }
}
