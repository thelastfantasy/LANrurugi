import type { CategoryMetadata } from "@/api/types"

/** Pinned-first, then alphabetical — matches legacy's `loadCategories` sort
 * (`~/LANraragi/public/js/mod/index.js`). Shared so every category list stays consistent.
 * `localeCompare` with no locale argument falls back to the runtime's own default locale
 * (`Intl.Collator`'s own docs) — on a real device this means the *browser's* locale, not this
 * app's own language setting, so the exact same category list sorts differently on a PC Chrome
 * (`navigator.language` `zh-CN`) vs. a phone Chrome with a different default (confirmed live,
 * 2026-09-01: Chinese-named categories sorted before Latin-named ones on PC, after them on
 * mobile). Pinning the locale to `"en"` makes the order deterministic across every device. */
export function sortCategories(categories: CategoryMetadata[]): CategoryMetadata[] {
  return [...categories].sort((a, b) => {
    if (a.pinned !== b.pinned) return b.pinned - a.pinned
    return a.name.localeCompare(b.name, "en")
  })
}
