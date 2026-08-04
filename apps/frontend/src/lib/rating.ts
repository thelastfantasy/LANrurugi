// Shared `rating:` tag parsing/formatting — used by `RatingWidget.tsx` (reader's own interactive
// widget), `ArchiveOverviewOverlay.tsx`'s `TagsTable` (read-only display), and `Library.tsx`'s
// right-click rating submenu.
//
// Storage format: a plain decimal string (`rating:4.5`), read to a full tenth's precision
// (`0.0`-`5.0`) — a deliberate departure from legacy's own `rating:⭐⭐⭐` (N repetitions of the star
// emoji, `.length`-counted, so representing anything but a whole number is impossible in that
// encoding at all). `parseRating` still recognizes the old repeated-star format on read — any
// already-existing `rating:⭐⭐⭐` tag written before this change keeps meaning the same 3.0 rating
// it always did, rather than silently reading as 0 — but every write from this app's own UI from
// here on always produces the new decimal format. A one-time migration script
// (`scripts/migrate-ratings.ts`) converts existing stored tags to the new format directly in
// Redis, so old-format reads are a permanent-but-rarely-hit compatibility path, not the norm.

const STAR = "⭐"

/** `rating:` tag value → a 0-5 float, or `null` if `raw` is empty/unparseable. Tries the new
 * decimal format first (`"4.5"` → `4.5`), then falls back to counting repeated star characters
 * (legacy's own format, and this app's pre-decimal-support format — `"⭐⭐⭐"` → `3`). */
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

/** A 0-5 float → the `rating:` tag value to store (this app's own decimal format, always — never
 * writes the legacy star-repeat format). Rounded to one decimal place since the UI only ever
 * produces whole/half-star values (`0, 0.5, 1, ...`), but a directly-edited or externally-authored
 * tag can carry finer precision (`rating:4.3`) — `parseRating` reads that back losslessly; this
 * function just guards against float drift (`4.5000000001`) when a UI-driven value round-trips
 * through arithmetic. */
export function formatRating(value: number): string {
  return (Math.round(value * 10) / 10).toString()
}
