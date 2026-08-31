// `localStorage` keys shared across more than one page/component — kept in one place so the two
// (or more) files reading/writing the same key can't drift apart on the string itself.

/** Multi-select batch-operation "handshake" — legacy's own `localStorage` key name. Library writes
 * the set here before navigating to Batch, which reads+clears it once on mount (a one-shot handoff). */
export const MSM_SELECTION_KEY = "msmSelection"

/** NOT the thumbnail grid's own layout (a plain CSS `flex-wrap`, no configurable column count) —
 * controls how many custom namespace columns the compact table view renders beyond Title. */
export const COLUMN_COUNT_KEY = "columnCount"

export const DEFAULT_COLUMN_COUNT = 2

/** Prefix for each compact-table custom column's chosen namespace — real keys are
 * `customColumn1`, `customColumn2`, etc. */
export const CUSTOM_COLUMN_PREFIX = "customColumn"

export const DEFAULT_CUSTOM_COLUMNS = ["artist", "series"]

// The remaining keys below all mirror legacy's own Library-index `localStorage` keys 1:1, so a
// value a user already set through legacy keeps meaning the same thing in this app.

/** Recently Added carousel open/closed state — `"1"`/`"0"` string, not a real boolean (matches
 * legacy's own storage exactly). */
export const CAROUSEL_OPEN_KEY = "carouselOpen"

/** Which of the 4 carousel modes (`ondeck`/`random`/`inbox`/`untagged`) is active. */
export const CAROUSEL_TYPE_KEY = "carouselType"

/** `"true"`/unset — crops thumbnails to fill their box vs showing natural aspect ratio. */
export const CROP_THUMBS_KEY = "cropthumbs"

/** `"true"`/unset — excludes >85%-read archives from the grid, carousel, and search. */
export const HIDE_COMPLETED_KEY = "hidecompleted"

/** `"false"`/unset — collapses Tankoubon volumes into one grouped entry in search results. */
export const GROUP_TANKS_KEY = "grouptanks"

/** `"1"` (thumbnail grid, default) or `"0"` (compact table). */
export const INDEX_VIEW_MODE_KEY = "indexViewMode"

/** Persisted sort-by field and order, restored on next visit. */
export const INDEX_SORT_KEY = "indexSort"

export const INDEX_ORDER_KEY = "indexOrder"

/** Last-applied legacy theme filename — also hardcoded in `index.html`'s inline script (can't
 * `import` this constant); keep both in sync by hand if this key name ever changes. */
export const THEME_STORAGE_KEY = "lrrTheme"
