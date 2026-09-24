/** Page-turn decision tree, ported from legacy reader.js's `changePage`/`goToPage`. All
 * 1-indexed, matching legacy and the rest of this app's archive-page API. */

export type ChangePageTarget = "prev" | "next" | "first" | "last"

export interface Spread {
  /** The page shown in the left DOM slot (`#img`). */
  left: number
  /** The page shown in the right DOM slot (`#img_doublepage`), or `null` if only one page is
   * showing. */
  right: number | null
}

/** Resolves a navigation intent plus current state into the next 1-indexed page. Widespread
 * detection happens by measuring already-loaded `<img>` elements at the call site (`Reader.tsx`). */
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

/** Decides which one or two pages to display and in which DOM slot — manga mode swaps left/right
 * so pages read highest-index-first. `widespreadCheck` requires a loaded image's natural size. */
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

  return mangaMode ? { left: partner, right: currentPage } : { left: currentPage, right: partner }
}
