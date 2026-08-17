/** Preset time-range boundaries for the Activity page's time filter, computed in a given IANA
 * timezone (the server's configured `timezone`, same source `tagFormat.ts::formatTimestampForDisplay`
 * reads) via the browser's native `Intl.DateTimeFormat` — mirrors the backend's own
 * `lanrurugi_search::engine::parse_date_range` (`chrono_tz::Tz` + `NaiveDate::and_hms_opt`) in
 * spirit, but implemented client-side since these boundaries only ever feed a `GET /activity`
 * query, never a stored value. Returns Unix-seconds `[start, end)` — `end` is exclusive, matching
 * `ActivityFilter::end_ts`'s own `ZREVRANGEBYSCORE` upper-bound semantics. */

/** `yyyy-mm-dd` field values for "now" as seen in `timezone` — the same `formatToParts` trick
 * `tagFormat.ts` uses, since `Intl` doesn't otherwise expose "give me a `Date` anchored at
 * midnight in this timezone" directly. */
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

/** The Unix-seconds instant of local midnight `daysAgo` days before "today" (in `timezone`),
 * found via binary search over the UTC offset — avoids needing a full IANA tz database on the
 * client (`Date` + `Intl` alone can't directly construct "this wall-clock time in that zone"). */
function midnightSecondsFor(year: number, month: number, day: number, timezone: string): number {
  // A UTC timestamp using the same y/m/d fields is at most 26 hours off the real local midnight
  // for any real-world timezone (UTC-12 to UTC+14) — narrow that guess to the exact instant by
  // reading back what wall-clock date/time it renders as in `timezone` and correcting the offset.
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

/** `[start, end)` Unix-seconds bounds for `preset`, anchored at "now" in `timezone`. `this_week`
 * starts Monday (ISO week, matching the rest of this app's own date handling — no Sunday-start
 * locale variance). */
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
    // ISO week starts Monday; `weekday` is 0=Sun..6=Sat, so Monday is `weekday === 0 ? 6 : weekday - 1`
    // days after the most recent Monday.
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
