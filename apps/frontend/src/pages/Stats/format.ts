/** Pure formatting/shape helpers for the `/stats` dashboard — kept out of the components so they
 *  can be unit-tested without a DOM or a real backend (see `tests/unit/statsFormat.test.ts`). */

const UNITS = ["B", "KB", "MB", "GB", "TB", "PB"] as const

/** Binary-prefix byte size, at most two decimals, with the unit the value actually lands in.
 *  `0` renders as `0 B` rather than `0.00 B` — a zero size is a fact, not a measurement. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B"
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  // A whole number of bytes (or whole KB/MB) reads better without a trailing `.00`.
  const decimals = unit === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2
  return `${value.toFixed(decimals)} ${UNITS[unit]}`
}

/** `part` as a percentage of `total`, one decimal, guarding `total === 0`. */
export function percentOf(part: number, total: number): number {
  if (!Number.isFinite(part) || !Number.isFinite(total) || total <= 0) return 0
  return (part / total) * 100
}

export function formatPercent(part: number, total: number): string {
  return `${percentOf(part, total).toFixed(1)}%`
}

/** i18n key for a size bucket's label — the backend sends only the stable key. */
export function sizeBucketLabelKey(key: string): string {
  return `stats.sizeBucket.${key}`
}

/** i18n key for a reading-progress bucket's label. */
export function progressBucketLabelKey(key: string): string {
  return `stats.progressBucket.${key}`
}

/** i18n key for an ingestion source's label; an unrecognized source is shown verbatim by
 *  `t()`'s own key-fallback, which is the honest thing to do with a source this build predates. */
export function ingestSourceLabelKey(source: string): string {
  return `stats.ingestSource.${source}`
}

/** `YYYY-MM-DD` → the axis/tooltip short form (`MM-DD`), without going through `Date` (the
 *  backend's dates are UTC calendar days, and `new Date("2026-10-14")` is parsed as UTC midnight
 *  then re-rendered in local time, which can shift the day). */
export function shortDay(iso: string): string {
  const parts = iso.split("-")
  return parts.length === 3 ? `${parts[1]}-${parts[2]}` : iso
}

/** `YYYY-MM` → `YY-MM`, same no-`Date` reasoning as {@link shortDay}. */
export function shortMonth(iso: string): string {
  const parts = iso.split("-")
  return parts.length === 2 ? `${parts[0].slice(2)}-${parts[1]}` : iso
}

/** Axis/tooltip label for one timeline bucket, whose key format depends on the bucket size the
 *  backend chose (`YYYY-MM` for months, `YYYY-MM-DD` otherwise — a week keyed by its Monday). */
export function timelineLabel(key: string, unit: "day" | "week" | "month"): string {
  return unit === "month" ? shortMonth(key) : shortDay(key)
}

/** i18n key naming a timeline's bucket size, for the sentence under each chart. */
export function timelineUnitKey(unit: "day" | "week" | "month"): string {
  return `stats.unit.${unit}`
}

/** Unix seconds → `YYYY-MM-DD` in local time (a real timestamp the user is reading on their own
 *  clock, unlike the backend's UTC calendar-day strings above). */
export function formatTimestamp(seconds: number): string {
  if (!seconds) return "-"
  return new Date(seconds * 1000).toLocaleDateString()
}

/** `page / pages` with a percentage, or a bare page count when the archive's page count is
 *  unknown (a failed catalogue). */
export function formatProgress(page: number, pages: number): string {
  if (pages <= 0) return `${page}`
  return `${page} / ${pages} (${percentOf(page, pages).toFixed(0)}%)`
}

/** The library search that reproduces one reading-progress bucket exactly.
 *
 *  Deliberately the two-comparison form rather than the single-token range (`read:40-50`): the
 *  range is inclusive at both ends, so it cannot express "started, but not at 0%" — and a bucket
 *  link that quietly includes the neighbouring bucket's members is worse than a longer URL. The
 *  `%` suffix is what makes `read:` mean a share of the page count (`read:40` alone is forty
 *  *pages*); the engine accepts `%` and its normalized `*` spelling interchangeably. */
export function progressBucketSearchQuery(key: string): string {
  switch (key) {
    case "unread":
      return "is:unread"
    case "1_20":
      return "read:>0% read:<=20%"
    case "21_40":
      return "read:>20% read:<=40%"
    case "41_60":
      return "read:>40% read:<=60%"
    case "61_80":
      return "read:>60% read:<=80%"
    case "81_99":
      return "read:>80% read:<100%"
    case "finished":
      return "read:>=100%"
    default:
      return ""
  }
}

/** The library URL for a raw search query — the same `/?q=` target `getTagSearchURL` builds, for
 *  queries that are not a single tag (`is:reading`, a progress range, …). */
export function searchURL(query: string): string {
  return `/?q=${encodeURIComponent(query)}`
}

/** The series palette index for slice `index`, wrapped so a chart with more slices than the
 *  palette has colours repeats the ramp instead of rendering transparent slices. */
export function seriesColor(colors: string[], index: number): string {
  if (colors.length === 0) return "currentColor"
  return colors[index % colors.length]
}
