import { useHoverPageOrder, useSetHoverPageOrder } from "@/api/hooks"

/** How `BookmarkHoverGrid` orders a single archive's own bookmarked pages within its popup —
 * distinct from `BookmarksPage.tsx`'s own `sort` (which orders the *list of archives*, not the
 * pages inside any one archive's preview). Four options: by when each page was bookmarked
 * (newest/oldest first) or by page number itself (ascending/descending). */
export type HoverGridPageOrder = "bookmarkedAtDesc" | "bookmarkedAtAsc" | "pageAsc" | "pageDesc"

const DEFAULT_ORDER: HoverGridPageOrder = "pageDesc"

function isValidOrder(value: string | null | undefined): value is HoverGridPageOrder {
  return value === "bookmarkedAtDesc" || value === "bookmarkedAtAsc" || value === "pageAsc" || value === "pageDesc"
}

/** Persisted server-side (Redis, via `GET`/`PUT /bookmarks/hover-page-order`) rather than
 * `localStorage` — a per-instance preference the user expects to follow them across devices/
 * browsers, unlike `useLibrary.ts`'s own view-option toggles. Falls back to `DEFAULT_ORDER` both
 * while the query is still in flight and once it resolves with `order: null` (never explicitly
 * saved before). */
export function useHoverGridPageOrder(): [HoverGridPageOrder, (order: HoverGridPageOrder) => void] {
  const query = useHoverPageOrder()
  const setOrderMutation = useSetHoverPageOrder()
  const order = isValidOrder(query.data?.order) ? query.data.order : DEFAULT_ORDER
  function setOrder(next: HoverGridPageOrder) {
    setOrderMutation.mutate(next)
  }
  return [order, setOrder]
}

/** Sorts a copy of `pages` per `order`. */
export function sortPagesByOrder<T extends { page: number; bookmarked_at: number }>(
  pages: T[],
  order: HoverGridPageOrder,
): T[] {
  const sorted = pages.slice()
  switch (order) {
    case "bookmarkedAtDesc":
      return sorted.sort((a, b) => b.bookmarked_at - a.bookmarked_at)
    case "bookmarkedAtAsc":
      return sorted.sort((a, b) => a.bookmarked_at - b.bookmarked_at)
    case "pageDesc":
      return sorted.sort((a, b) => b.page - a.page)
    case "pageAsc":
      return sorted.sort((a, b) => a.page - b.page)
  }
}
