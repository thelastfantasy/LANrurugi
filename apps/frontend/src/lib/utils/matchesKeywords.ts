/** Splits `query` on whitespace into lowercased keywords — mirrors
 * `lanrurugi-api::bookmarks::card_matches_query`'s own `q_lower.split_whitespace()` exactly, so
 * the two stay in lockstep. */
export function splitKeywords(query: string | undefined): string[] {
  return (query ?? "")
    .split(/\s+/)
    .map((k) => k.trim().toLowerCase())
    .filter(Boolean)
}

/** Every keyword must appear as a substring of *some* string in `fields` (case-insensitive) —
 * different keywords may each match a different field, same AND-across-keywords/
 * OR-across-fields rule the backend's `card_matches_query` applies. `keywords` must already be
 * lowercased (`splitKeywords`'s own output) — this doesn't lowercase them itself so a caller
 * checking many field-sets against the same query only pays that cost once. */
export function matchesKeywords(fields: (string | null | undefined)[], keywords: string[]): boolean {
  if (keywords.length === 0) return true
  const lowered = fields.filter((f): f is string => !!f).map((f) => f.toLowerCase())
  return keywords.every((kw) => lowered.some((f) => f.includes(kw)))
}
