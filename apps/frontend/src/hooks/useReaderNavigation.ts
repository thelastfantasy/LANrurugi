// Page-turn decision tree, ported from legacy reader.js's `changePage`/`goToPage`
// (`~/LANraragi/public/js/reader.js:1938`/`:1337`) — manga mode negates the numeric offset and
// swaps "first"/"last", double-page mode tries to pair `page`/`page+1` as a spread unless either
// image is a "widespread" (auto-detected: `naturalWidth > naturalHeight`), in which case it falls
// back to single-page display and re-anchors. All 1-indexed, matching legacy and the rest of this
// app's archive-page API.

export type ChangePageTarget = "prev" | "next" | "first" | "last"

export interface Spread {
  /** The page shown in the left DOM slot (`#img`). */
  left: number
  /** The page shown in the right DOM slot (`#img_doublepage`), or `null` if only one page is
   * showing (first/last page, or a widespread fallback). */
  right: number | null
}

/** Resolves a navigation intent (`target`) plus current state into the next 1-indexed page. Pure
 * function — doesn't touch the DOM or fetch anything; widespread detection happens by measuring
 * already-loaded `<img>` elements at the call site (see `Reader.tsx`), same as legacy. */
export function computeNextPage(
  target: ChangePageTarget,
  currentPage: number,
  totalPages: number,
  mangaMode: boolean,
  doublePageMode: boolean,
  isShowingSpread: boolean,
): number {
  if (target === "first") return mangaMode ? totalPages : 1
  if (target === "last") return mangaMode ? 1 : totalPages

  let offset = target === "next" ? 1 : -1
  if (doublePageMode && isShowingSpread) offset *= 2
  if (mangaMode) offset = -offset

  return clamp(currentPage + offset, 1, totalPages)
}

export function clamp(page: number, min: number, max: number): number {
  return Math.min(Math.max(page, min), max)
}

/** Given the current page and whether double-page mode is on, decides which two (or one) pages to
 * display and in which DOM slot — manga mode swaps left/right so pages read highest-index-first.
 * `widespreadCheck(page)` returns true if that single page's image is wider than tall (legacy's
 * "widespread" detection) — caller supplies it since it requires a loaded image's natural size. */
export function computeSpread(
  currentPage: number,
  totalPages: number,
  doublePageMode: boolean,
  mangaMode: boolean,
  widespreadCheck: (page: number) => boolean | undefined,
): Spread {
  const atEdge = currentPage <= 1 || currentPage >= totalPages
  if (!doublePageMode || atEdge) {
    return { left: currentPage, right: null }
  }

  const partner = currentPage + 1
  if (partner > totalPages) {
    return { left: currentPage, right: null }
  }

  const eitherIsWidespread = widespreadCheck(currentPage) || widespreadCheck(partner)
  if (eitherIsWidespread) {
    return { left: currentPage, right: null }
  }

  // Normal LTR: currentPage on the left, partner on the right. Manga mode flips which page goes
  // in which slot so the higher page number reads first (right-to-left).
  return mangaMode ? { left: partner, right: currentPage } : { left: currentPage, right: partner }
}
