import type { CategoryMetadata } from "@/api/types"

/** Pinned-first, then alphabetical — matches legacy's `loadCategories` sort
 * (`~/LANraragi/public/js/mod/index.js`). Shared so every category list stays consistent. */
export function sortCategories(categories: CategoryMetadata[]): CategoryMetadata[] {
  return [...categories].sort((a, b) => {
    if (a.pinned !== b.pinned) return b.pinned - a.pinned
    return a.name.localeCompare(b.name)
  })
}
