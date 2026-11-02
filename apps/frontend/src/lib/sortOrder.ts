// Sort direction rules shared by the Library page's request builder and the sort control. Kept
// out of `useLibrary` so the mapping can be unit-tested without mounting the hook.

/** "Relevance" is not a field you can meaningfully order back-to-front — it always means
 *  most-relevant first. The engine's `asc` *is* that direction (`order_desc == false` ranks the
 *  highest score first, see `engine.rs`'s relevance branch), so this sort always sends `asc`
 *  whatever direction another field was left in. */
export const RELEVANCE_SORT = "relevance"

/** A field's own natural direction, applied when the user switches to it: newest first for dates,
 *  A→Z for everything else (title and tag namespaces). Carrying the previous field's direction
 *  over is what used to turn "Date" into oldest-first after sorting by Title descending. */
export function defaultSortDir(sortby: string): "asc" | "desc" {
  if (sortby === "date_added" || sortby === "lastread") return "desc"
  return "asc"
}

/** The direction actually sent to the API and written to the URL for `sortby`. Relevance
 *  overrides the field's currently displayed direction; every other field passes its own
 *  through unchanged. */
export function effectiveSortDir(
  sortby: string,
  dir: "asc" | "desc",
): "asc" | "desc" {
  if (sortby === RELEVANCE_SORT) return "asc"
  return dir
}
