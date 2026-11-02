/** Pure time-series maths for the `/stats` charts: bucketing a daily series into day/week/month,
 *  and the window arithmetic behind range presets, ctrl+wheel zoom and drag-to-pan.
 *
 *  Everything here works on the backend's own `YYYY-MM-DD` UTC calendar strings. Calendar maths
 *  goes through `Date.UTC`, never local `Date` construction or `toLocaleDateString` — a local
 *  `new Date("2026-01-01")` is parsed as UTC midnight and then re-read in the viewer's own zone,
 *  which silently shifts the day (and therefore which bucket a point lands in) for anyone west of
 *  UTC. Kept free of React so `tests/unit/statsTimeline.test.ts` can cover the edges directly. */

export type TimelineUnit = "auto" | "day" | "week" | "month"

export type ResolvedUnit = "day" | "week" | "month"

export interface DailyPoint {
  date: string
  archives?: number
  pages?: number
  last_read?: number
}

export interface TimelineBucket {
  /** Bucket start: the day itself, that week's Monday, or `YYYY-MM`. */
  key: string
  /** First and last day this bucket covers, inclusive — what a drill-down is queried with, so the
   *  list under a clicked bar is exactly the bars's span rather than a re-derived guess. */
  from: string
  to: string
  /** How many daily points fell in this bucket — the tooltip's own denominator for "per day". */
  days: number
  archives: number
  pages: number
  last_read: number
}

const DAY_MS = 86_400_000

/** How many buckets a window may hold before `auto` coarsens the unit. Matches the backend's own
 *  former rule, which the client now owns so granularity changes cost no request. */
export const MAX_BUCKETS = 30

/** `auto` never produces fewer than this many buckets either: a three-day-old library reads better
 *  as a week of daily bars than as one bar filling the axis. */
export const MIN_BUCKETS = 7

function parse(date: string): number {
  const [y, m, d] = date.split("-").map(Number)
  return Date.UTC(y, (m ?? 1) - 1, d ?? 1)
}

function toIso(ms: number): string {
  return new Date(ms).toISOString().slice(0, 10)
}

/** `YYYY-MM-DD` plus/minus whole days. */
export function addDays(date: string, days: number): string {
  return toIso(parse(date) + days * DAY_MS)
}

/** Whole days between two calendar dates (negative when `to` precedes `from`). */
export function daysBetween(from: string, to: string): number {
  return Math.round((parse(to) - parse(from)) / DAY_MS)
}

/** The Monday of the week containing `date` (UTC). */
export function mondayOf(date: string): string {
  // getUTCDay: 0 = Sunday, so Sunday belongs to the week that started six days earlier.
  const sinceMonday = (new Date(parse(date)).getUTCDay() + 6) % 7
  return addDays(date, -sinceMonday)
}

export function bucketKey(unit: ResolvedUnit, date: string): string {
  switch (unit) {
    case "day":
      return date
    case "week":
      return mondayOf(date)
    case "month":
      return date.slice(0, 7)
  }
}

/** How many buckets `unit` needs for `[from, to]` — counted by walking the bucket starts, which is
 *  also exactly what {@link bucketSeries} emits, so the two can never disagree. */
export function bucketCount(unit: ResolvedUnit, from: string, to: string): number {
  if (daysBetween(from, to) < 0) return 0
  let count = 0
  let cursor = bucketKey(unit, from)
  for (;;) {
    count += 1
    if (unit === "day") cursor = addDays(cursor, 1)
    else if (unit === "week") cursor = addDays(cursor, 7)
    else {
      const [y, m] = cursor.split("-").map(Number)
      cursor = m === 12 ? `${y + 1}-01` : `${y}-${String(m + 1).padStart(2, "0")}`
    }
    if (cursor > bucketKey(unit, to)) return count
    if (count > 5000) return count // defensive: a malformed range cannot spin forever
  }
}

/** The unit actually drawn: the reader's locked choice, or — for `auto` — the finest unit whose
 *  bucket count stays readable for this window. */
export function resolveUnit(unit: TimelineUnit, from: string, to: string): ResolvedUnit {
  if (unit !== "auto") return unit
  for (const candidate of ["day", "week", "month"] as const) {
    if (bucketCount(candidate, from, to) <= MAX_BUCKETS) return candidate
  }
  return "month"
}

/** One bucket per step of `unit` spanning `[from, to]`, oldest first, zeros where nothing happened
 *  — a gap in the data has to read as an empty stretch, not as a narrower axis. */
export function bucketSeries(
  points: DailyPoint[],
  unit: ResolvedUnit,
  from: string,
  to: string,
): TimelineBucket[] {
  if (daysBetween(from, to) < 0) return []
  const buckets = new Map<string, TimelineBucket>()
  const order: string[] = []
  for (let day = from; day <= to; day = addDays(day, 1)) {
    const key = bucketKey(unit, day)
    const existing = buckets.get(key)
    if (existing) existing.to = day
    else {
      buckets.set(key, { key, from: day, to: day, days: 0, archives: 0, pages: 0, last_read: 0 })
      order.push(key)
    }
  }
  for (const point of points) {
    if (point.date < from || point.date > to) continue
    const bucket = buckets.get(bucketKey(unit, point.date))
    if (!bucket) continue
    bucket.days += 1
    bucket.archives += point.archives ?? 0
    bucket.pages += point.pages ?? 0
    bucket.last_read += point.last_read ?? 0
  }
  // `order` and `buckets` are written together, so a miss is unreachable — but returning a
  // zero-filled bucket beats a `!` that a later edit could turn into a crash.
  return order.map(
    (key) =>
      buckets.get(key) ?? { key, from, to, days: 0, archives: 0, pages: 0, last_read: 0 },
  )
}

export interface Window {
  from: string
  to: string
}

/** Clamps a window to the data's own bounds, and never lets it invert or collapse below one day. */
export function clampWindow(window: Window, bounds: Window): Window {
  let to = window.to > bounds.to ? bounds.to : window.to
  let from = window.from < bounds.from ? bounds.from : window.from
  if (from >= to) {
    // Keep at least one day on the axis — zooming all the way in to a single day is legitimate.
    if (to > bounds.from) from = addDays(to, -1)
    else to = from < bounds.to ? addDays(from, 1) : from
  }
  return { from, to }
}

/** Range presets, in days; `null` = the whole available range. */
export function presetWindow(days: number | null, bounds: Window): Window {
  if (days === null) return { ...bounds }
  const span = daysBetween(bounds.from, bounds.to)
  const width = Math.min(days, span)
  return { from: addDays(bounds.to, -width), to: bounds.to }
}

/** Ctrl/⌘+wheel zoom around the pointer. `anchorRatio` is where the cursor sits in the plot
 *  (0 = left edge, 1 = right edge), and the day under it is what stays put — zooming away from the
 *  thing you are pointing at is the classic way to make a chart feel broken. `factor > 1` zooms in
 *  (fewer days), `< 1` zooms out. */
export function zoomWindow(window: Window, factor: number, anchorRatio: number, bounds: Window): Window {
  const span = Math.max(1, daysBetween(window.from, window.to))
  const ratio = Math.min(Math.max(anchorRatio, 0), 1)
  const anchorDate = addDays(window.from, Math.round(span * ratio))
  const target = Math.max(1, Math.round(span / factor))
  // The anchor date keeps the same fraction of the window on either side of it.
  const from = addDays(anchorDate, -Math.round(target * ratio))
  return clampWindow({ from, to: addDays(from, target) }, bounds)
}

/** The span a shift-click selects: from the anchor bucket's start to the clicked bucket's end —
 *  and the other way round when shift-clicking backwards, so extending a range works in both
 *  directions rather than silently collapsing to the earlier of the two. */
export function extendWindow(anchor: Window, clicked: Window): Window {
  return {
    from: anchor.from <= clicked.from ? anchor.from : clicked.from,
    to: anchor.to >= clicked.to ? anchor.to : clicked.to,
  }
}

/** Drag-to-pan: shift the whole window by whole days, keeping its width. */
export function panWindow(window: Window, deltaDays: number, bounds: Window): Window {
  const width = daysBetween(window.from, window.to)
  let from = addDays(window.from, deltaDays)
  let to = addDays(from, width)
  if (from < bounds.from) {
    from = bounds.from
    to = addDays(from, width)
  }
  if (to > bounds.to) {
    to = bounds.to
    from = addDays(to, -width)
    if (from < bounds.from) from = bounds.from
  }
  return { from, to }
}
