// Storage format is decimal (`rating:4.5`); `parseRating` still reads legacy's star-repeat format
// for backward compat, but writes always use decimal.
const STAR = "⭐"

export function parseRating(raw: string | undefined | null): number | null {
  if (!raw) return null
  const trimmed = raw.trim()
  if (!trimmed) return null
  const asNumber = Number(trimmed)
  if (Number.isFinite(asNumber) && /^\d+(\.\d+)?$/.test(trimmed)) {
    return Math.max(0, Math.min(5, asNumber))
  }
  const starCount = [...trimmed].filter((c) => c === STAR).length
  return starCount > 0 ? starCount : null
}

export function formatRating(value: number): string {
  return (Math.round(value * 10) / 10).toString()
}
