export const PAGE_SIZE = 100

export const NEW_ONLY = "NEW_ONLY"

export const UNTAGGED_ONLY = "UNTAGGED_ONLY"

export const TANKOUBON_ONLY = "TANKOUBON_ONLY"

/** Quick filters that are really search-syntax atoms — `has:source` has no boolean Redis parameter
 *  of its own the way `untaggedonly` does, so these two only shape the filter text. */
export const SOURCE_ONLY = "SOURCE_ONLY"

export const NO_SOURCE_ONLY = "NO_SOURCE_ONLY"

export const CATEGORY_BUTTON_CAP = 10
