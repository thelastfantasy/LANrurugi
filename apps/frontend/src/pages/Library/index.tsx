import type { MouseEvent } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'

import { sendJson } from '../../api/client'
import {
  useCategories,
  useCreateTankoubon,
  useLoginStatus,
  useSearch,
  useServerInfo,
  useSetArchiveProgress,
  useSettings,
  useStats,
  useTankoubons,
} from '../../api/hooks'
import type { ArchiveMetadata } from '../../api/types'
import { PopupMenu, PopupMenuItem } from '../../components/PopupMenu'
import { confirmDialog, promptDialog } from '../../dialog'
import { buildSearchToken, buildTagList, splitTagsByNamespace } from '../../lib/tagFormat'
import { routes } from '../../routes'
import {
  COLUMN_COUNT_KEY,
  CROP_THUMBS_KEY,
  DEFAULT_COLUMN_COUNT,
  GROUP_TANKS_KEY,
  HIDE_COMPLETED_KEY,
  INDEX_ORDER_KEY,
  INDEX_SORT_KEY,
  INDEX_VIEW_MODE_KEY,
  MSM_SELECTION_KEY,
} from '../../storageKeys'
import { Z_OVERLAY_CONTENT } from '../../theme'
import { toast } from '../../toast'
import { useDocumentTitle } from '../../useDocumentTitle'
import { recordSearchNavigation } from '../Reader/crossArchiveNav'
import { ArchiveCard } from './ArchiveCard'
import { ArchiveContextMenu, DeleteConfirmDialog } from './ArchiveContextMenu'
import { CompactTable } from './CompactTable'
import { RecentlyAddedCarousel } from './RecentlyAddedCarousel'
import { SettingsMenu } from './SettingsMenu'
import { CATEGORY_BUTTON_CAP, type ContextMenuState, isTankoubonId, NEW_ONLY, PAGE_SIZE, UNTAGGED_ONLY } from './shared'

// Mirrors legacy's `~/LANraragi/templates/index.html.tt2` + `public/js/mod/index.js`/
// `index_datatables.js`/`index_contextmenu.js` — the library grid page. Split out of a single
// 2098-line `Library.tsx` into this directory (issue #49): `shared.tsx` holds the primitives with
// 2+ real consumers across the split files (`isTankoubonId`, `BookmarkIcon`, `TagLine`, plus the
// module-level constants/types both this file and the extracted feature files need);
// `ArchiveCard.tsx`/`RecentlyAddedCarousel.tsx`/`ArchiveContextMenu.tsx`/`CompactTable.tsx`/
// `SettingsMenu.tsx` are each a self-contained feature block; this file keeps only what's
// genuinely specific to the page itself — its own state, handlers, and the JSX shell composing
// the rest. Pure structural split, no behavior change.

// Matches `lanrurugi-api::search`'s fixed page size (`search.rs`'s `PAGE_SIZE` constant) —
// server-side pagination isn't configurable per-request, so "Go to Page" paginates through these
// fixed 100-archive chunks rather than the user's own `archives_per_page` display setting.

// Module-level (not component state/`localStorage`): persists across `Library` remounting
// mid-session (e.g. in-app nav back to `/`) but resets on an actual page reload/fresh tab,
// matching legacy's semantics (its own toast trigger only ever runs once per real HTTP page
// load). Without this, each SPA-internal remount re-fired the toast, stacking multiple copies on
// screen given its long `hideAfter: 25000` with no de-dup.
let defaultPasswordToastShownThisPageLoad = false

export default function Library() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const location = useLocation()
  const info = useServerInfo()
  const categories = useCategories()
  const tankoubons = useTankoubons()
  const createTankoubon = useCreateTankoubon()
  const loginStatus = useLoginStatus()
  const settings = useSettings()
  const stats = useStats(2)
  const loggedIn = loginStatus.data?.logged_in ?? true

  // `appliedFilter`/`selectedCategory`/`sortby`/`order`/`page` all derive directly from
  // `location.search` on every render — no `useState` mirror. `useLocation()` is React Router's
  // live subscription to the URL (updates on `navigate()` and browser back/forward), so it's the
  // single source of truth; each `setXxx` function below is a real `navigate()` call, a genuine
  // back/forward-navigable history entry, with `sortby`/`order` additionally persisted to
  // `localStorage` as a fallback for the next fresh visit with no URL params. One shared
  // `URLSearchParams` object keeps every derivation reading the same snapshot of the URL.
  const urlParams = useMemo(() => new URLSearchParams(location.search), [location.search])

  // The textbox's own live contents mid-typing, before applied (Enter/button/tag click) — not
  // URL-driven, unlike `appliedFilter` below. `null` means "not being actively edited", in which
  // case the input displays `appliedFilter` directly. Tracking this as its own nullable state
  // (rather than a `key`-remount trick) keeps the textbox in sync with browser back/forward
  // without racing which render the remount's initializer happens to read stale `location` from.
  const [filterInputOverride, setFilterInputOverride] = useState<string | null>(null)
  const appliedFilter = urlParams.get('q') ?? ''
  const filterInput = filterInputOverride ?? appliedFilter
  // Legacy's own `index_datatables.js` appends the current search term to the tab title — useful
  // for scanning browser history for which search produced which page.
  useDocumentTitle(appliedFilter || undefined)

  function buildSearch(overrides: {
    page?: number
    sortby?: string
    order?: 'asc' | 'desc'
    appliedFilter?: string
    selectedCategory?: string
  }): string {
    const nextPage = overrides.page ?? page
    const nextSortby = overrides.sortby ?? sortby
    const nextOrder = overrides.order ?? order
    const nextFilter = overrides.appliedFilter ?? appliedFilter
    const nextCategory = overrides.selectedCategory ?? selectedCategory
    const params = new URLSearchParams()
    if (nextPage !== 0) params.set('p', String(nextPage + 1))
    if (nextSortby !== 'title') params.set('sort', nextSortby)
    if (nextOrder !== 'asc') params.set('sortdir', nextOrder)
    if (nextFilter) params.set('q', nextFilter)
    if (nextCategory) params.set('c', nextCategory)
    return params.toString()
  }

  const selectedCategory = urlParams.get('c') ?? ''
  const [categoryOverflowOpen, setCategoryOverflowOpen] = useState(false)
  const [autocompleteOpen, setAutocompleteOpen] = useState(false)
  const sortby = urlParams.get('sort') ?? localStorage.getItem(INDEX_SORT_KEY) ?? 'title'
  const order: 'asc' | 'desc' = (() => {
    const fromUrl = urlParams.get('sortdir')
    if (fromUrl === 'asc' || fromUrl === 'desc') return fromUrl
    return (localStorage.getItem(INDEX_ORDER_KEY) as 'asc' | 'desc' | null) ?? 'asc'
  })()
  // The one and only setter for every URL-driven field — every call site below passes every
  // field it's changing in a single call (e.g. `navigateSearch({ appliedFilter: '', page: 0 })`,
  // not two separate calls), since `buildSearch()` reads its un-overridden fields from the current
  // render's closure values — two independent `navigate()` calls in the same handler would each
  // rebuild the URL from that same stale snapshot, silently overwriting each other's change.
  // `sortby`/`order` changes also persist to `localStorage` as a fallback for a future fresh visit.
  function navigateSearch(overrides: {
    page?: number
    sortby?: string
    order?: 'asc' | 'desc'
    appliedFilter?: string
    selectedCategory?: string
  }) {
    if (overrides.sortby !== undefined) localStorage.setItem(INDEX_SORT_KEY, overrides.sortby)
    if (overrides.order !== undefined) localStorage.setItem(INDEX_ORDER_KEY, overrides.order)
    navigate({ search: buildSearch(overrides) })
  }
  const [viewMode, setViewModeState] = useState<'thumbnail' | 'compact'>(
    () => (localStorage.getItem(INDEX_VIEW_MODE_KEY) === '0' ? 'compact' : 'thumbnail'),
  )
  function setViewMode(v: 'thumbnail' | 'compact') {
    setViewModeState(v)
    localStorage.setItem(INDEX_VIEW_MODE_KEY, v === 'compact' ? '0' : '1')
  }
  const [cropThumbs, setCropThumbsState] = useState(() => localStorage.getItem(CROP_THUMBS_KEY) !== 'false')
  function setCropThumbs(v: boolean) {
    setCropThumbsState(v)
    localStorage.setItem(CROP_THUMBS_KEY, String(v))
  }
  const [hideCompleted, setHideCompletedState] = useState(() => localStorage.getItem(HIDE_COMPLETED_KEY) === 'true')
  function setHideCompleted(v: boolean) {
    setHideCompletedState(v)
    localStorage.setItem(HIDE_COMPLETED_KEY, String(v))
  }
  const [groupbyTanks, setGroupbyTanksState] = useState(() => localStorage.getItem(GROUP_TANKS_KEY) !== 'false')
  function setGroupbyTanks(v: boolean) {
    setGroupbyTanksState(v)
    localStorage.setItem(GROUP_TANKS_KEY, String(v))
  }
  // Default and persistence key both mirror legacy's own `getColumnCount()`/`handleColumnNum()`
  // (`~/LANraragi/public/js/mod/index.js`) exactly — default of 2 when nothing's stored yet, and
  // every change is written back to the same `localStorage` key so it survives a reload/revisit.
  const [columns, setColumnsState] = useState(() => {
    const stored = localStorage.getItem(COLUMN_COUNT_KEY)
    const parsed = stored ? Number.parseInt(stored, 10) : NaN
    return Number.isFinite(parsed) && parsed > 0 ? parsed : DEFAULT_COLUMN_COUNT
  })
  const setColumns = (value: number) => {
    setColumnsState(value)
    localStorage.setItem(COLUMN_COUNT_KEY, String(value))
  }
  const page = (() => {
    const fromUrl = Number(urlParams.get('p'))
    return Number.isInteger(fromUrl) && fromUrl > 0 ? fromUrl - 1 : 0
  })()
  const [multiSelect, setMultiSelect] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set())
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<{ id: string; isTank: boolean } | null>(null)
  // See `RecentlyAddedCarousel`'s own `refreshKey` prop docs — bumped after "Mark as Read"/"Mark
  // as Unread" so the carousel's own non-TanStack-Query fetch effect re-runs and picks up the new
  // progress value, since `invalidateQueries` alone (which the main grid's `useSearch` already
  // responds to) has no effect on this carousel at all.
  const [carouselRefreshKey, setCarouselRefreshKey] = useState(0)
  const searchInputRef = useRef<HTMLInputElement>(null)
  const setArchiveProgress = useSetArchiveProgress()
  function handleSetProgress(archiveId: string, page: number) {
    setArchiveProgress.mutate(
      { id: archiveId, page },
      { onSuccess: () => setCarouselRefreshKey((k) => k + 1) },
    )
  }

  // First-visit context-menu tutorial toast + default-password warning — fired once per browser
  // via a `localStorage` flag for the tutorial, and every load for the default-password warning
  // (matching legacy, which has no dismiss-forever flag for it either).
  useEffect(() => {
    if (loginStatus.data?.using_default_password && !defaultPasswordToastShownThisPageLoad) {
      defaultPasswordToastShownThisPageLoad = true
      toast({
        heading: t("You're using the default password and that's super baka of you") ?? undefined,
        text:
          t(
            'Login with password "kamimamita" and change that shit on the double. ...Or just disable it! Why not check the configuration options afterwards, while you\'re at it?',
          ) ?? undefined,
        icon: 'warning',
        hideAfter: 25000,
        closeOnClick: false,
        draggable: false,
      })
    }
  }, [loginStatus.data?.using_default_password, t])

  useEffect(() => {
    const seenKey = 'seenContextMenuTutorial'
    if (localStorage.getItem(seenKey)) return
    localStorage.setItem(seenKey, '1')
    toast({
      heading: t('Tip: right-click an archive for more actions!') ?? undefined,
      icon: 'info',
      hideAfter: 8000,
    })
  }, [t])

  // Ports `migrateProgress` — a one-time sweep of stray `*-reader` localStorage progress entries
  // left over from when `localprogress` was on, pushing each one to the server (only if ahead of
  // what the server already has) and clearing the local copy. Waits for `settings`/`loginStatus`
  // to load first so it doesn't wrongly skip on a still-pending query's default `false`s.
  useEffect(() => {
    if (!settings.data || loginStatus.data === undefined) return
    if (settings.data.localprogress || (settings.data.authprogress && !loggedIn)) return
    const keys = Object.keys(localStorage)
      .filter((k) => k.endsWith('-reader'))
      .map((k) => k.slice(0, -'-reader'.length))
    if (keys.length === 0) return

    toast({
      heading: t('Migrating local reading progress to the server…') ?? undefined,
      text: `${t("This only happens once — go grab a coffee, it won't take long.")} ☕`,
      icon: 'info',
      hideAfter: 23000,
    })

    void Promise.all(
      keys.map(async (id) => {
        const localProgress = Number(localStorage.getItem(`${id}-reader`))
        const isTank = isTankoubonId(id)
        const metadataUrl = isTank ? `/api/tankoubons/${id}` : `/api/archives/${id}/metadata`
        const progressUrl = isTank ? `/api/tankoubons/${id}/progress/${localProgress}` : `/api/archives/${id}/progress/${localProgress}`
        try {
          const res = await fetch(metadataUrl)
          if (res.status === 404) {
            localStorage.removeItem(`${id}-reader`)
            return
          }
          if (!res.ok) return
          const data = await res.json()
          const serverProgress = data.progress as number | undefined
          if (Number.isFinite(localProgress) && serverProgress !== undefined && localProgress > serverProgress) {
            await fetch(progressUrl, { method: 'PUT' })
          }
        } finally {
          localStorage.removeItem(`${id}-reader`)
        }
      }),
    ).then(() => {
      toast({
        heading: `${t('Local progress migration complete!')} 🎉`,
        text: t('Every archive with local-only reading progress has been synced to the server.') ?? undefined,
        icon: 'success',
        hideAfter: 13000,
      })
    })
    // Deliberately runs once per mount, not on every `loggedIn`/`settings.data` identity change —
    // this is a one-shot sweep, not a recurring sync.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settings.data, loginStatus.data])

  // Global `/`-to-focus-search and Escape-to-close-overlay shortcuts (legacy's own
  // `handleQuickSearch`/`handleEscapeKey`, `index.js`).
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === '/' && (e.target as HTMLElement)?.tagName !== 'INPUT') {
        e.preventDefault()
        searchInputRef.current?.focus()
      }
      if (e.key === 'Escape') {
        setContextMenu(null)
        setCategoryOverflowOpen(false)
      }
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [])

  // A plain, unfiltered `useArchives()` isn't enough on its own to answer "how many total
  // archives are there" once a category/sort/page is active — `/search` (empty filter included)
  // is the single source of truth here, matching legacy's own `index.js`, which always goes
  // through the same search endpoint regardless of whether a text filter is set.
  const isBuiltinSelector = selectedCategory === NEW_ONLY || selectedCategory === UNTAGGED_ONLY
  const search = useSearch({
    filter: appliedFilter,
    category: !isBuiltinSelector && selectedCategory ? selectedCategory : undefined,
    sortby,
    order,
    start: page * PAGE_SIZE,
    newonly: selectedCategory === NEW_ONLY,
    untaggedonly: selectedCategory === UNTAGGED_ONLY,
    hidecompleted: hideCompleted,
    groupbyTanks,
  })

  const shown = search.data?.data ?? []
  const totalFiltered = search.data?.recordsFiltered ?? 0
  const pageCount = Math.max(1, Math.ceil(totalFiltered / PAGE_SIZE))
  const rangeStart = totalFiltered === 0 ? 0 : page * PAGE_SIZE + 1
  const rangeEnd = Math.min(totalFiltered, page * PAGE_SIZE + PAGE_SIZE)

  // Pinned-first, then alphabetical — matches `loadCategories`'s own sort
  // (`~/LANraragi/public/js/mod/index.js`). The first `CATEGORY_BUTTON_CAP` become buttons; the
  // rest spill into a "..." overflow dropdown.
  const sortedCategories = useMemo(() => {
    const list = categories.data ?? []
    return [...list].sort((a, b) => {
      if (a.pinned !== b.pinned) return b.pinned - a.pinned
      return a.name.localeCompare(b.name)
    })
  }, [categories.data])
  const visibleCategories = sortedCategories.slice(0, CATEGORY_BUTTON_CAP)
  const overflowCategories = sortedCategories.slice(CATEGORY_BUTTON_CAP)

  // Search-bar tag autocomplete — ports `loadTagSuggestions`'s filter/sort rule: match against
  // only the fragment after the last `,`/`-`/whitespace (so autocomplete works mid-multi-tag-
  // search), case-insensitive substring match, sorted by tag weight descending.
  const currentFragment = filterInput.match(/[^,\s-]*$/)?.[0] ?? ''
  const tagSuggestions = useMemo(() => {
    if (!currentFragment) return []
    const needle = currentFragment.toLowerCase()
    return (stats.data ?? [])
      .map((s) => ({
        label: s.namespace ? `${s.namespace}:${s.text}` : s.text,
        // What actually gets inserted into the search box on click — quoted when `s.text` has a
        // space, unlike `label` (the plain, human-readable text shown in the dropdown itself),
        // since space is now a real token delimiter in the search grammar (issue #59).
        insertValue: buildSearchToken(s.namespace ?? '', s.text),
        weight: s.weight,
      }))
      .filter((s) => s.label.toLowerCase().includes(needle))
      .sort((a, b) => b.weight - a.weight)
      .slice(0, 15)

  }, [stats.data, currentFragment])

  function toggleCategory(id: string) {
    navigateSearch({ selectedCategory: selectedCategory === id ? '' : id, page: 0 })
  }

  function toggleSelected(id: string) {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  function selectAllOnPage() {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      for (const a of shown) next.add(a.arcid)
      return next
    })
  }

  function clearSelection() {
    setSelectedIds(new Set())
  }

  // Legacy's own MSM toggle confirms before clearing a non-empty selection when turning
  // multi-select *off* (`index.js`'s `toggleMultiSelectMode`) — turning it *on* never needs
  // confirmation since there's nothing to lose yet.
  async function handleToggleMultiSelect() {
    if (multiSelect && selectedIds.size > 0) {
      if (!(await confirmDialog(t('You have an active selection. Exiting will clear it. Continue?') ?? ''))) {
        return
      }
    }
    setMultiSelect((v) => !v)
    clearSelection()
  }

  function runBatchOnSelection() {
    if (selectedIds.size === 0) return
    // Matches legacy's own hand-off exactly (`~/LANraragi/public/js/mod/index.js`'s
    // `openBatchOnSelection`/`updateSelectionCount`): stash the selection in `localStorage` under
    // the same key, open `/batch` in a new tab to read (and immediately clear) it.
    localStorage.setItem(MSM_SELECTION_KEY, JSON.stringify([...selectedIds]))
    window.open('/batch', '_blank')
  }

  // Selection containing exactly one existing Tankoubon folds the rest *into* that tank rather
  // than always creating a new one; 2+ tanks selected makes the whole operation ambiguous, so the
  // merge action is hidden for that case entirely (both matching legacy's own
  // `index.js` merge-button logic).
  const selectedTankIds = [...selectedIds].filter(isTankoubonId)
  const canMerge = selectedTankIds.length < 2 && selectedIds.size > 0

  async function mergeSelectionIntoTankoubon() {
    if (!canMerge) return
    try {
      if (selectedTankIds.length === 1) {
        const targetTank = selectedTankIds[0]
        const archiveIds = [...selectedIds].filter((id) => id !== targetTank)
        const existing = tankoubons.data?.result.find((tk) => tk.id === targetTank)
        const merged = [...(existing?.archives ?? []), ...archiveIds]
        await fetch(`/api/tankoubons/${targetTank}`, {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ archives: merged }),
        })
        clearSelection()
        navigate(routes.tankoubonEdit(targetTank))
        return
      }
      const name = await promptDialog(t('Enter a name for the new Tankoubon.') ?? '')
      if (!name?.trim()) return
      const result = await createTankoubon.mutateAsync(name.trim())
      await fetch(`/api/tankoubons/${result.tankid}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ archives: [...selectedIds] }),
      })
      clearSelection()
      navigate(routes.tankoubonEdit(result.tankid))
    } catch {
      toast({ heading: t('Error creating Tankoubon') ?? undefined, icon: 'error' })
    }
  }

  async function toggleArchiveCategory(categoryId: string, archiveId: string, currentlyIn: boolean) {
    await fetch(`/api/categories/${categoryId}/${archiveId}`, { method: currentlyIn ? 'DELETE' : 'PUT' })
    await categories.refetch()
  }

  async function updateRating(archiveId: string, isTank: boolean, rating: string | null) {
    const endpoint = isTank ? `/api/tankoubons/${archiveId}` : `/api/archives/${archiveId}/metadata`
    const current = shown.find((a) => a.arcid === archiveId)
    const tagsByNamespace = splitTagsByNamespace(current?.tags ?? '')
    if (rating === null) delete tagsByNamespace.rating
    else tagsByNamespace.rating = [rating]
    const newTags = buildTagList(tagsByNamespace).join(', ')
    if (isTank) {
      await fetch(endpoint, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ tags: newTags }),
      })
    } else {
      await sendJson('PUT', `/archives/${archiveId}/metadata?tags=${encodeURIComponent(newTags)}`)
    }
    await search.refetch()
  }

  async function deleteArchive(archiveId: string, isTank: boolean) {
    if (isTank) {
      await fetch(`/api/tankoubons/${archiveId}`, { method: 'DELETE' })
      await tankoubons.refetch()
    } else {
      await fetch(`/api/archives/${archiveId}`, { method: 'DELETE' })
    }
    await search.refetch()
  }

  function handleContextMenu(e: MouseEvent, archive: ArchiveMetadata, source: 'grid' | 'carousel' = 'grid') {
    e.preventDefault()
    setContextMenu({ archive, x: e.clientX, y: e.clientY, source })
  }

  function applyTagSearch(namespacedTag: string) {
    setFilterInputOverride(null)
    navigateSearch({ appliedFilter: namespacedTag, page: 0 })
  }

  // Hands off "which search produced this results page" to the reader (`crossArchiveNav.ts`),
  // matching legacy's own datatables->reader handoff — lets `,`/`.` step across archives inside
  // this same search without the reader re-deriving it.
  function handleOpenArchive(id: string) {
    recordSearchNavigation(
      shown.map((a) => a.arcid),
      page + 1,
      {
        filter: appliedFilter,
        category: selectedCategory,
        sortby,
        order,
        pageSize: PAGE_SIZE,
        groupbyTanks,
        hidecompleted: hideCompleted,
      },
    )
    navigate(routes.reader(id))
  }

  if (search.isError) {
    // Ports legacy's own `#json-error` panel verbatim (`~/LANraragi/templates/index.html.tt2`) —
    // same bomb icons, same exact two lines of copy, not a paraphrase — a search failure here
    // almost always means the archive index itself is unreadable, not a transient network blip.
    return (
      <div className="ido" style={{ textAlign: 'center', padding: 40 }}>
        <div id="json-error">
          <h1 style={{ color: 'red' }}>
            <i className="fas fa-bomb" aria-hidden="true"></i>{' '}
            {t("I don't know everything, but I sure as hell know this database's busted lads")}{' '}
            <i className="fas fa-bomb" aria-hidden="true"></i>
          </h1>
          <h2>{t('The database cache is corrupt, and as such LANraragi is unable to display your archive list.')}</h2>
        </div>
      </div>
    )
  }

  return (
    <div className="ido">
      <h1 className="ih">{info.data?.motd}</h1>

      <div id="toppane">
        <div className="idi">
          <div id="category-container">
            <button
              type="button"
              className={`favtag-btn${selectedCategory === NEW_ONLY ? ' toggled' : ''}`}
              title={t('Archives added within the last day') ?? undefined}
              onClick={() => toggleCategory(NEW_ONLY)}
            >
              🆕 {t('New Archives')}
            </button>
            <button
              type="button"
              className={`favtag-btn${selectedCategory === UNTAGGED_ONLY ? ' toggled' : ''}`}
              title={t('Archives with no tags at all') ?? undefined}
              onClick={() => toggleCategory(UNTAGGED_ONLY)}
            >
              🏷️ {t('Untagged Archives')}
            </button>
            {visibleCategories.map((c) => (
              <button
                key={c.id}
                type="button"
                className={`favtag-btn${selectedCategory === c.id ? ' toggled' : ''}`}
                onClick={() => toggleCategory(c.id)}
              >
                {c.pinned ? '📌 ' : ''}
                {c.name}
              </button>
            ))}
            {overflowCategories.length > 0 && (
              <span style={{ position: 'relative' }}>
                <button type="button" className="favtag-btn" onClick={() => setCategoryOverflowOpen((v) => !v)}>
                  ...
                </button>
                {categoryOverflowOpen && (
                  <select
                    className="favtag-btn"
                    style={{ position: 'absolute', top: '100%', left: 0, zIndex: Z_OVERLAY_CONTENT }}
                    value=""
                    onChange={(e) => {
                      if (e.target.value) toggleCategory(e.target.value)
                      setCategoryOverflowOpen(false)
                    }}
                  >
                    <option value="" disabled>
                      {t('More categories…')}
                    </option>
                    {overflowCategories.map((c) => (
                      <option key={c.id} value={c.id}>
                        {c.pinned ? '📌 ' : ''}
                        {c.name}
                      </option>
                    ))}
                  </select>
                )}
              </span>
            )}
          </div>
          <span style={{ position: 'relative', display: 'inline-block' }}>
            <input
              id="search-input"
              ref={searchInputRef}
              className="search stdinput"
              value={filterInput}
              autoComplete="off"
              onChange={(e) => {
                setFilterInputOverride(e.target.value)
                setAutocompleteOpen(true)
              }}
              onFocus={() => setAutocompleteOpen(true)}
              onBlur={() => setTimeout(() => setAutocompleteOpen(false), 150)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  setFilterInputOverride(null)
                  navigateSearch({ appliedFilter: filterInput, page: 0 })
                  setAutocompleteOpen(false)
                }
                if (e.key === 'Escape') setAutocompleteOpen(false)
              }}
              placeholder={t('Search Title, Artist, Series, Language or Tags') ?? undefined}
            />
            {autocompleteOpen && tagSuggestions.length > 0 && (
              // Not portaled — `minWidth: '100%'` needs the search `<input>`'s own wrapping
              // `<span style={{ position: 'relative' }}>` as its positioning ancestor to size
              // against; a default portal to `document.body` would break both that and the
              // `top`/`left` offsets.
              <PopupMenu
                portal={false}
                style={{ position: 'absolute', top: '100%', left: 0, zIndex: Z_OVERLAY_CONTENT, minWidth: '100%', maxHeight: 220, overflowY: 'auto' }}
              >
                {tagSuggestions.map((s) => (
                  <PopupMenuItem
                    key={s.label}
                    onMouseDown={(e) => {
                      // `onMouseDown` (fires before the input's own `onBlur`) rather than
                      // `onClick`, so the suggestion click actually lands instead of losing the
                      // dropdown to the blur handler first.
                      e.preventDefault()
                      const upToCursor = filterInput.replace(/[^,\s-]*$/, '')
                      const next = `${upToCursor}${s.insertValue}`
                      setFilterInputOverride(next)
                      setAutocompleteOpen(false)
                      searchInputRef.current?.focus()
                    }}
                  >
                    {s.label}
                  </PopupMenuItem>
                ))}
              </PopupMenu>
            )}
          </span>
          <input
            id="apply-search"
            className="searchbtn stdbtn"
            type="button"
            value={t('Apply Filter') ?? undefined}
            onClick={() => {
              setFilterInputOverride(null)
              navigateSearch({ appliedFilter: filterInput, page: 0 })
            }}
          />
          <input
            id="clear-search"
            className="searchbtn stdbtn"
            type="button"
            value={t('Clear Filter') ?? undefined}
            onClick={() => {
              // Legacy's own `#clear-search` only clears the text filter — it does NOT reset the
              // selected category (`index_datatables.js`: `currentSearch = ""; doSearch();`).
              setFilterInputOverride(null)
              navigateSearch({ appliedFilter: '', page: 0 })
            }}
          />
          <input
            id="msm-toggle"
            className="searchbtn stdbtn"
            type="button"
            value={t('Select Archives') ?? undefined}
            onClick={() => void handleToggleMultiSelect()}
          />
        </div>
      </div>

      <RecentlyAddedCarousel
        filter={appliedFilter}
        category={selectedCategory}
        hideCompleted={hideCompleted}
        groupbyTanks={groupbyTanks}
        cropThumbs={cropThumbs}
        onContextMenu={handleContextMenu}
        onOpen={handleOpenArchive}
        multiSelect={multiSelect}
        selectedIds={selectedIds}
        onToggleSelected={toggleSelected}
        onSelectPage={selectAllOnPage}
        onClearSelection={clearSelection}
        onRunBatch={runBatchOnSelection}
        onMerge={() => void mergeSelectionIntoTankoubon()}
        canMerge={canMerge}
        onSearchTag={applyTagSearch}
        refreshKey={carouselRefreshKey}
      />

      {/* The real 4%-side inset comes from each theme's own `.table-options` rule (e.g.
          `modern.css`'s `margin-right/left: 4%; margin-bottom: -64px`). The theme's own
          `margin-bottom: -64px` is NOT reused: that value only makes sense against legacy's own
          jQuery DataTables layout, which reserves invisible header space this app's grid doesn't
          have — inheriting it verbatim pulls the toolbar out of view behind the grid below.
          Explicit `marginBottom: 0` overrides just that one property back off. */}
      <div className="table-options" style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 16, marginBottom: 0 }}>
        <div className="thumbnail-options">
          {t('Sort by:')}{' '}
          <select
            className="favtag-btn"
            value={sortby}
            onChange={(e) => {
              navigateSearch({ sortby: e.target.value, page: 0 })
            }}
          >
            <option value="title">{t('Title')}</option>
            <option value="date_added">{t('Date')}</option>
            {/* Every real tag namespace with weight >= 2 (`useStats(2)`, matching legacy's own
                `loadTagSuggestions` weight floor) becomes a sortable field too — legacy's own
                sort-by dropdown isn't fixed to just Title/Date either. */}
            {[...new Set((stats.data ?? []).map((s) => s.namespace).filter((n): n is string => !!n && n !== 'date_added'))]
              .sort()
              .map((ns) => (
                // legacy capitalizes only the display label, not the `value` (`index.js:341`).
                <option key={ns} value={ns}>
                  {ns.charAt(0).toUpperCase() + ns.slice(1)}
                </option>
              ))}
          </select>
          {/* Real legacy markup: `class="fa fa-sort-alpha-down fa-2x table-option"`. */}
          <a
            className={`fa fa-2x fa-sort-alpha-${order === 'asc' ? 'down' : 'up'} table-option`}
            href="#"
            title={t('Sort Order') ?? undefined}
            onClick={(e) => {
              e.preventDefault()
              navigateSearch({ order: order === 'asc' ? 'desc' : 'asc' })
            }}
          ></a>
        </div>
        {viewMode === 'compact' && (
          <div className="compact-options">
            {t('Columns:')}{' '}
            {/* Legacy semantics — NOT a thumbnail-grid column count (that grid has no such
                setting). It's how many extra namespace columns (Artist, Series, ...) the compact
                table shows beyond Title. */}
            <select className="favtag-btn" value={columns} onChange={(e) => setColumns(Number(e.target.value))}>
              {Array.from({ length: 20 }, (_, i) => i + 1).map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </div>
        )}
        <div style={{ marginLeft: 'auto', display: 'flex', alignItems: 'center' }}>
          {t('Go to Page:')}{' '}
          <select className="favtag-btn" value={page} onChange={(e) => navigateSearch({ page: Number(e.target.value) })}>
            {Array.from({ length: pageCount }, (_, i) => i).map((p) => (
              <option key={p} value={p}>
                {p + 1}
              </option>
            ))}
          </select>
          <SettingsMenu
            viewMode={viewMode}
            setViewMode={setViewMode}
            cropThumbs={cropThumbs}
            setCropThumbs={setCropThumbs}
            hideCompleted={hideCompleted}
            setHideCompleted={setHideCompleted}
            groupbyTanks={groupbyTanks}
            setGroupbyTanks={setGroupbyTanks}
          />
        </div>
      </div>

      {search.isLoading ? (
        <p>{t('Loading library…')}</p>
      ) : shown.length === 0 ? (
        <div style={{ textAlign: 'center', padding: 40 }}>
          <i className="fas fa-sad-cry fa-4x" aria-hidden="true"></i>
          <h1>
            {t('No archives to show you! Try')}{' '}
            <a href={routes.upload()} onClick={(e) => { e.preventDefault(); navigate(routes.upload()) }}>
              {t('uploading some')}
            </a>
            ?
          </h1>
        </div>
      ) : viewMode === 'compact' ? (
        <CompactTable
          shown={shown}
          columns={columns}
          selectedIds={selectedIds}
          multiSelect={multiSelect}
          onSearchTag={applyTagSearch}
          onToggleSelected={toggleSelected}
          onOpen={handleOpenArchive}
          onContextMenu={handleContextMenu}
        />
      ) : (
        <div
          id="thumbs_container"
          // Legacy's real thumbnail grid: `#thumbs_container` has no layout CSS of its own —
          // plain block flow, each card (`div.id1`) is `display: inline-block` with its own
          // `margin`, wrapping the same way inline text would. A `display: flex` version
          // measurably fits fewer cards per row at the same container width, so this needs to be
          // the real thing, not a flex approximation.
          style={{ textAlign: 'center' }}
        >
          {shown.map((a) => (
            <ArchiveCard
              key={a.arcid}
              archive={a}
              multiSelect={multiSelect}
              selected={selectedIds.has(a.arcid)}
              cropThumbs={cropThumbs}
              onToggleSelect={toggleSelected}
              onContextMenu={handleContextMenu}
              onOpen={handleOpenArchive}
              onSearchTag={applyTagSearch}
            />
          ))}
        </div>
      )}

      {/* Legacy's own DataTables `info` string (`I18N.IndexPageCount`, "Showing _START_ to _END_
          of _TOTAL_ ...") — sits below the results, not as a heading above them; there is no
          legacy equivalent of a big "Archives (N)" title over the grid. */}
      {totalFiltered > 0 && (
        <p style={{ textAlign: 'center', opacity: 0.7, margin: '10px' }}>
          {t('Showing {{start}} to {{end}} of {{total}} archives.', {
            start: rangeStart,
            end: rangeEnd,
            total: totalFiltered,
          })}
        </p>
      )}

      {contextMenu && (
        <ArchiveContextMenu
          state={contextMenu}
          categories={categories.data}
          loggedIn={loggedIn}
          liveArchives={shown}
          onClose={() => setContextMenu(null)}
          onToggleCategory={(categoryId, archiveId, currentlyIn) => void toggleArchiveCategory(categoryId, archiveId, currentlyIn)}
          onDelete={(archiveId, isTank) => setDeleteTarget({ id: archiveId, isTank })}
          onOpen={handleOpenArchive}
          onRatingChange={(archiveId, isTank, rating) => void updateRating(archiveId, isTank, rating)}
          onToggleSelection={(id) => {
            if (!multiSelect) setMultiSelect(true)
            toggleSelected(id)
          }}
          isSelected={selectedIds.has(contextMenu.archive.arcid)}
          onSetProgress={handleSetProgress}
        />
      )}

      {deleteTarget && (
        <DeleteConfirmDialog
          isTank={deleteTarget.isTank}
          onCancel={() => setDeleteTarget(null)}
          onConfirm={() => {
            void deleteArchive(deleteTarget.id, deleteTarget.isTank)
            setDeleteTarget(null)
          }}
        />
      )}
    </div>
  )
}
