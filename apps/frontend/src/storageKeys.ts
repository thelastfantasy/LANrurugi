// `localStorage` keys shared across more than one page/component — kept in one place so the two
// (or more) files reading/writing the same key can't drift apart on the string itself.

/** Multi-select batch-operation "handshake" — legacy's own `localStorage.getItem/setItem/
 * removeItem("msmSelection")` key name, verified against `~/LANraragi/public/js/mod/index.js`.
 * Library writes the current multi-select set here before navigating to Batch, which reads it
 * once on mount and clears it immediately after (a one-shot handoff, not a persisted setting). */
export const MSM_SELECTION_KEY = 'msmSelection'

/** NOT the thumbnail grid's own layout — legacy's grid is a plain CSS `flex-wrap` with no
 * configurable column count at all. `columnCount` (`~/LANraragi/public/js/mod/index.js`'s
 * `getColumnCount()`: `parseInt(...) : 2` when unset) only ever controls how many *custom
 * namespace columns* (e.g. "Artist", "Series") the compact **table** view renders beyond Title —
 * see `CUSTOM_COLUMN_PREFIX` below. Verified against the real legacy source, not assumed. */
export const COLUMN_COUNT_KEY = 'columnCount'

export const DEFAULT_COLUMN_COUNT = 2

/** Prefix for each compact-table custom column's chosen namespace — real keys are
 * `customColumn1`, `customColumn2`, etc. Defaults to `artist`/`series` for columns 1/2
 * (`~/LANraragi/public/js/mod/index.js`), `Header N` for any column beyond that. */
export const CUSTOM_COLUMN_PREFIX = 'customColumn'

export const DEFAULT_CUSTOM_COLUMNS = ['artist', 'series']

// The remaining keys below all mirror legacy's own Library-index `localStorage` keys 1:1 (names
// and semantics verified against `~/LANraragi/public/js/mod/index.js`), so a value a user already
// set through legacy (if migrating a browser profile) or through this app keeps meaning the same
// thing either way.

/** Recently Added carousel open/closed state — `"1"`/`"0"` string, not a real boolean (matches
 * legacy's own storage exactly). */
export const CAROUSEL_OPEN_KEY = 'carouselOpen'

/** Which of the 4 carousel modes (`ondeck`/`random`/`inbox`/`untagged`) is active. */
export const CAROUSEL_TYPE_KEY = 'carouselType'

/** `"true"`/unset — crops thumbnails to fill their box vs showing natural aspect ratio. */
export const CROP_THUMBS_KEY = 'cropthumbs'

/** `"true"`/unset — excludes >85%-read archives from the grid, carousel, and search. */
export const HIDE_COMPLETED_KEY = 'hidecompleted'

/** `"false"`/unset — collapses Tankoubon volumes into one grouped entry in search results. */
export const GROUP_TANKS_KEY = 'grouptanks'

/** `"1"` (thumbnail grid, default) or `"0"` (compact table). */
export const INDEX_VIEW_MODE_KEY = 'indexViewMode'

/** Persisted sort-by field and order, restored on next visit. */
export const INDEX_SORT_KEY = 'indexSort'

export const INDEX_ORDER_KEY = 'indexOrder'
