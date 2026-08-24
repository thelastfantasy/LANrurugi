import type { CategoryMetadata } from "@/api/types"

/** Pinned-first, then alphabetical — matches legacy's own `loadCategories` sort
 * (`~/LANraragi/public/js/mod/index.js`). Shared by every place that lists categories (the
 * Library page's category bar/dropdown and the Categories management page's own select), so they
 * never drift out of sync with each other the way they once did (the management page used to
 * render `GET /categories`'s raw, unsorted order instead of this). */
export function sortCategories(categories: CategoryMetadata[]): CategoryMetadata[] {
  return [...categories].sort((a, b) => {
    if (a.pinned !== b.pinned) return b.pinned - a.pinned
    return a.name.localeCompare(b.name)
  })
}
