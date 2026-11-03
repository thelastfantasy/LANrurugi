/** Human-readable binary byte sizes: `1.18 MB`, `0 B`. Pure, so the share card bundle can use it
 * too (the `/stats` module it moves out of is a page module). */

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
