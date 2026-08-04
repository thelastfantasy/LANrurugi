// Cross-archive "read next/previous archive in these search results" navigation — verified
// against legacy's `setupArchiveNavigation`/`loadPreviousDatatablesArchives`/
// `loadNextDatatablesArchives`/`readPreviousArchive`/`readNextArchive`
// (`~/LANraragi/public/js/reader.js:2044-2231`). The index page (`Library.tsx`) writes the
// "current search results" handoff on archive-open; the reader reads/mutates it as the user
// steps across archive boundaries. All key names match legacy exactly since they're pure
// per-browser scratch state, not synced with any server value.

const SEARCH_IDS_KEY = "currArchiveIds"
const PREV_IDS_KEY = "previousArchiveIds"
const NEXT_IDS_KEY = "nextArchiveIds"
const DT_PAGE_KEY = "currDatatablesPage"
const NAV_STATE_KEY = "navigationState" // sessionStorage, not localStorage — legacy's own choice
const CURRENT_SEARCH_KEY = "currentSearch"
const SELECTED_CATEGORY_KEY = "selectedCategory"
const INDEX_SORT_KEY = "indexSort"
const INDEX_ORDER_KEY = "indexOrder"
const DT_PAGE_SIZE_KEY = "datatablesPageSize"
const GROUP_TANKS_KEY = "grouptanks"
const HIDE_COMPLETED_KEY = "hidecompleted"

export interface IndexSearchState {
  filter: string
  category: string
  sortby: string
  order: "asc" | "desc"
  pageSize: number
  groupbyTanks: boolean
  hidecompleted: boolean
}

/** Called by the index page right before navigating into the reader — records which search
 * produced the current results page and the full ordered ID list, so the reader can page across
 * archive boundaries without re-deriving the search itself. */
export function recordSearchNavigation(ids: string[], datatablesPage: number, state: IndexSearchState) {
  localStorage.setItem(SEARCH_IDS_KEY, JSON.stringify(ids))
  localStorage.setItem(DT_PAGE_KEY, String(datatablesPage))
  localStorage.setItem(CURRENT_SEARCH_KEY, state.filter)
  localStorage.setItem(SELECTED_CATEGORY_KEY, state.category)
  localStorage.setItem(INDEX_SORT_KEY, state.sortby)
  localStorage.setItem(INDEX_ORDER_KEY, state.order)
  localStorage.setItem(DT_PAGE_SIZE_KEY, String(state.pageSize))
  localStorage.setItem(GROUP_TANKS_KEY, String(state.groupbyTanks))
  localStorage.setItem(HIDE_COMPLETED_KEY, String(state.hidecompleted))
  // Cached adjacent-page ID lists are now stale for a *new* search — clear them so the reader
  // re-fetches instead of cross-linking two different searches' results.
  localStorage.removeItem(PREV_IDS_KEY)
  localStorage.removeItem(NEXT_IDS_KEY)
  sessionStorage.setItem(NAV_STATE_KEY, "datatables")
}

function readIndexSearchState(): IndexSearchState {
  return {
    filter: localStorage.getItem(CURRENT_SEARCH_KEY) ?? "",
    category: localStorage.getItem(SELECTED_CATEGORY_KEY) ?? "",
    sortby: localStorage.getItem(INDEX_SORT_KEY) ?? "title",
    order: localStorage.getItem(INDEX_ORDER_KEY) === "desc" ? "desc" : "asc",
    pageSize: Number(localStorage.getItem(DT_PAGE_SIZE_KEY)) || 100,
    groupbyTanks: localStorage.getItem(GROUP_TANKS_KEY) !== "false",
    hidecompleted: localStorage.getItem(HIDE_COMPLETED_KEY) === "true",
  }
}

async function fetchSearchIds(state: IndexSearchState, start: number): Promise<string[] | null> {
  const params = new URLSearchParams({
    filter: state.filter,
    sortby: state.sortby,
    order: state.order,
    start: String(start),
    groupby_tanks: String(state.groupbyTanks),
    hidecompleted: String(state.hidecompleted),
  })
  if (state.category === "NEW_ONLY") params.set("newonly", "true")
  else if (state.category === "UNTAGGED_ONLY") params.set("untaggedonly", "true")
  else if (state.category) params.set("category", state.category)

  const response = await fetch(`/api/search/ids?${params}`)
  if (!response.ok) return null
  const result = (await response.json()) as { data: string[] }
  return result.data.length > 0 ? result.data : null
}

async function loadAdjacentDatatablesArchives(direction: "prev" | "next"): Promise<string[] | null> {
  const cacheKey = direction === "prev" ? PREV_IDS_KEY : NEXT_IDS_KEY
  const cached = localStorage.getItem(cacheKey)
  if (cached) return JSON.parse(cached) as string[]

  const currentPage = Number(localStorage.getItem(DT_PAGE_KEY)) || 1
  if (direction === "prev" && currentPage <= 1) return null
  const targetPage = direction === "prev" ? currentPage - 1 : currentPage + 1
  const state = readIndexSearchState()
  return fetchSearchIds(state, (targetPage - 1) * state.pageSize)
}

export interface ArchiveNavState {
  /** Ordered archive IDs for the search-results page the reader was opened from. Empty when
   * cross-archive navigation isn't available (direct navigation, no referrer, etc). */
  ids: string[]
  index: number
}

/** Mirrors legacy's `setupArchiveNavigation` — resolves whether cross-archive nav applies to this
 * reader session at all (same-origin referrer + a "datatables" navigation handoff present), and
 * if so, eagerly prefetches the adjacent search-results page when the opened archive is at either
 * edge of the current page's list (so the very first `,`/`.` press doesn't have to wait on it). */
export async function setupArchiveNavigation(archiveId: string): Promise<ArchiveNavState> {
  const referrer = document.referrer
  const isDirectNavigation = !referrer || !referrer.includes(window.location.host)
  if (isDirectNavigation) {
    sessionStorage.removeItem(NAV_STATE_KEY)
    return { ids: [], index: -1 }
  }

  if (sessionStorage.getItem(NAV_STATE_KEY) !== "datatables") {
    return { ids: [], index: -1 }
  }

  const idsJson = localStorage.getItem(SEARCH_IDS_KEY)
  if (!idsJson) return { ids: [], index: -1 }

  let ids: string[]
  try {
    ids = JSON.parse(idsJson) as string[]
  } catch {
    return { ids: [], index: -1 }
  }
  const index = ids.indexOf(archiveId)
  if (index === -1) return { ids: [], index: -1 }

  if (index === 0) {
    const prev = await loadAdjacentDatatablesArchives("prev")
    if (prev) localStorage.setItem(PREV_IDS_KEY, JSON.stringify(prev))
  }
  if (index === ids.length - 1) {
    const next = await loadAdjacentDatatablesArchives("next")
    if (next) localStorage.setItem(NEXT_IDS_KEY, JSON.stringify(next))
  }

  return { ids, index }
}

/** Mirrors legacy's `readPreviousArchive`/`readNextArchive`: steps within the cached ID list, or,
 * at an edge, consumes the pre-fetched adjacent page (shifting the whole cache window). Returns
 * `null` (and lets the caller show a toast) when there's nowhere further to go. */
export function resolveAdjacentArchive(
  nav: ArchiveNavState,
  direction: "prev" | "next",
): string | null {
  if (nav.ids.length === 0) return null

  const atEdge = direction === "prev" ? nav.index === 0 : nav.index === nav.ids.length - 1
  if (!atEdge) {
    return nav.ids[direction === "prev" ? nav.index - 1 : nav.index + 1]
  }

  const cacheKey = direction === "prev" ? PREV_IDS_KEY : NEXT_IDS_KEY
  const cachedJson = localStorage.getItem(cacheKey)
  if (!cachedJson) return null
  const cachedIds = JSON.parse(cachedJson) as string[]
  if (cachedIds.length === 0) return null

  const currentIdsJson = JSON.stringify(nav.ids)
  const currentPage = Number(localStorage.getItem(DT_PAGE_KEY)) || 1

  if (direction === "prev") {
    localStorage.removeItem(PREV_IDS_KEY)
    localStorage.setItem(SEARCH_IDS_KEY, cachedJson)
    localStorage.setItem(NEXT_IDS_KEY, currentIdsJson)
    localStorage.setItem(DT_PAGE_KEY, String(currentPage - 1))
    return cachedIds[cachedIds.length - 1]
  }

  localStorage.removeItem(NEXT_IDS_KEY)
  localStorage.setItem(SEARCH_IDS_KEY, cachedJson)
  localStorage.setItem(PREV_IDS_KEY, currentIdsJson)
  localStorage.setItem(DT_PAGE_KEY, String(currentPage + 1))
  return cachedIds[0]
}
