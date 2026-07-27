import 'swiper/css'
import 'swiper/css/navigation'

import type { MouseEvent } from 'react'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation, useNavigate } from 'react-router-dom'
import { Mousewheel, Navigation } from 'swiper/modules'
import { Swiper, SwiperSlide } from 'swiper/react'

import { sendJson } from '../api/client'
import {
  useArchiveMetadata,
  useBookmarkLink,
  useCategories,
  useCreateTankoubon,
  useLoginStatus,
  useSearch,
  useServerInfo,
  useSetArchiveProgress,
  useSettings,
  useStats,
  useTankoubons,
} from '../api/hooks'
import type { ArchiveMetadata } from '../api/types'
import { PopupMenu, PopupMenuItem, PopupMenuSeparator, useMenuPalette } from '../components/PopupMenu'
import RatingWidget from '../components/RatingWidget'
import TagTable from '../components/TagTable'
import Tooltip from '../components/Tooltip'
import { confirmDialog, promptDialog } from '../dialog'
import {
  buildNamespacedTag,
  buildTagList,
  colorCodeTags,
  formatTimestampForDisplay,
  getTagSearchURL,
  splitTagsByNamespace,
  tagValueForSearch,
} from '../lib/tagFormat'
import { routes } from '../routes'
import {
  CAROUSEL_OPEN_KEY,
  CAROUSEL_TYPE_KEY,
  COLUMN_COUNT_KEY,
  CROP_THUMBS_KEY,
  CUSTOM_COLUMN_PREFIX,
  DEFAULT_COLUMN_COUNT,
  DEFAULT_CUSTOM_COLUMNS,
  GROUP_TANKS_KEY,
  HIDE_COMPLETED_KEY,
  INDEX_ORDER_KEY,
  INDEX_SORT_KEY,
  INDEX_VIEW_MODE_KEY,
  MSM_SELECTION_KEY,
} from '../storageKeys'
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from '../theme'
import { toast } from '../toast'
import { useDocumentTitle } from '../useDocumentTitle'
import { recordSearchNavigation } from './Reader/crossArchiveNav'

// Matches `lanrurugi-api::search`'s fixed page size (`search.rs`'s `PAGE_SIZE` constant) —
// server-side pagination isn't configurable per-request, so "Go to Page" paginates through these
// fixed 100-archive chunks rather than the user's own `archives_per_page` display setting.
const PAGE_SIZE = 100

// The two hardcoded quick-filter category ids legacy's own `index.js` special-cases
// (`LANraragi::Controller::Api::Search::handle_databases`) — not real category ids, intercepted
// client-side before ever reaching `category=` and turned into `newonly=true`/`untaggedonly=true`
// instead.
const NEW_ONLY = 'NEW_ONLY'
const UNTAGGED_ONLY = 'UNTAGGED_ONLY'
// Legacy caps the visible category-button row at 10 entries before spilling the rest into a
// "..." overflow `<select>` (`index.js`'s `loadCategories`).
const CATEGORY_BUTTON_CAP = 10

type CarouselMode = 'ondeck' | 'random' | 'inbox' | 'untagged'

interface ContextMenuState {
  archive: ArchiveMetadata
  x: number
  y: number
  source: 'grid' | 'carousel'
}

function isTankoubonId(id: string): boolean {
  return id.startsWith('TANK_')
}

/** Read-crown/new/tankoubon status badges — ports `buildStatusDiv` exactly, including its
 * mutual-exclusion rule (an archive shows 🆕 XOR 👑, never both; a Tankoubon can show both plus
 * 📚) and its >85%-read threshold. */
function StatusIcons({ archive }: { archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  const isTank = isTankoubonId(archive.arcid)
  const isRead = archive.pagecount > 0 && archive.progress / archive.pagecount > 0.85
  const showNew = archive.isnew
  const showCrown = isRead && (isTank || !showNew)

  if (!showNew && !showCrown && !isTank) return null
  return (
    <div className="isnew status-icons">
      {showNew && <span title={t('New!') ?? undefined}>🆕</span>}
      {showCrown && <span title={t('Read') ?? undefined}>👑</span>}
      {isTank && <span title={t('Tankoubon') ?? undefined}>📚</span>}
    </div>
  )
}

/** Ports `buildPageCountDiv` — a Tankoubon with pages shows the 3-part `progress/pagecount/
 * archive_count` form (via its own `archive_count` field, populated server-side only for
 * synthetic Tankoubon search-result entries — see `search.rs`'s `resolve_search_entry`); a plain
 * archive shows the 2-part form; nothing renders when `pagecount` is 0. */
function PageCountBadge({ archive }: { archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  if (archive.pagecount <= 0) return null
  const isTank = isTankoubonId(archive.arcid) && archive.archive_count != null
  return (
    <div className="isnew">
      <sup title={(isTank ? t('Tankoubon Page Count') : t('Page Count')) ?? undefined}>
        {isTank
          ? `${archive.progress}/${archive.pagecount}/${archive.archive_count}`
          : `${archive.progress}/${archive.pagecount}`}
      </sup>
    </div>
  )
}

/** Bookmark star — ports `buildBookmarkIconElement`: renders nothing unless a bookmark category
 * is actually configured (`useBookmarkLink`), filled/outline depending on current membership
 * (read straight off `useCategories`' own `archives` array, matching the Reader page's own
 * `isBookmarked` derivation — no separate `localStorage.bookmarkedArchives` cache, since that
 * cache exists in legacy purely to avoid a page-load fetch we don't need with react-query's own
 * shared cache), and disabled/dimmed when logged out. */
function BookmarkIcon({ archiveId }: { archiveId: string }) {
  const { t } = useTranslation()
  const bookmarkLink = useBookmarkLink()
  const categories = useCategories()
  const loginStatus = useLoginStatus()
  const bookmarkCategoryId = bookmarkLink.data?.category_id || null
  if (!bookmarkCategoryId) return null
  const loggedIn = loginStatus.data?.logged_in ?? true
  const isBookmarked = Boolean(
    categories.data?.find((c) => c.id === bookmarkCategoryId)?.archives.includes(archiveId),
  )

  async function toggle(e: MouseEvent) {
    e.preventDefault()
    e.stopPropagation()
    if (!loggedIn) {
      toast({
        text: `<a href="${routes.login()}">${t('Login')}</a> ${t('to toggle bookmark feature.')}`,
        icon: 'warning',
      })
      return
    }
    const method = isBookmarked ? 'DELETE' : 'PUT'
    await fetch(`/api/categories/${bookmarkCategoryId}/${archiveId}`, { method })
    await categories.refetch()
  }

  return (
    <i
      className={`${isBookmarked ? 'fas' : 'far'} fa-bookmark thumbnail-bookmark-icon${loggedIn ? '' : ' disabled'}`}
      title={t('Toggle Bookmark') ?? undefined}
      style={!loggedIn ? { opacity: 0.5, cursor: 'not-allowed' } : { cursor: 'pointer' }}
      onClick={(e) => void toggle(e)}
    ></i>
  )
}

/** Tag line + hover tooltip — ports `colorCodeTags` (namespace-colored, date/time-excluded,
 * CSS-ellipsis-truncated via the `span.tags` rule already present in the copied `lrr.css`) for the
 * always-visible line, and `buildTagsDiv` (the full per-namespace tag table, via the shared
 * `TagTable` component) for the hover body — rendered through the shared `Tooltip` component
 * (portaled to `document.body`) rather than a locally absolutely-positioned `<table>`, since the
 * grid card's own ancestors clip an unportaled tooltip (this was silently never visible on the
 * homepage grid before — a real regression fixed here, not a style tweak). Click-to-search on any
 * individual tag (`.gt[search]` in legacy, intercepted by `index_datatables.js` to fire a live
 * search instead of a full navigation — reproduced here as an in-app filter-apply). */
function TagLine({
  tags,
  onSearchTag,
}: {
  tags: string
  onSearchTag: (namespacedTag: string) => void
}) {
  const timezone = useSettings().data?.timezone ?? ''
  const coded = colorCodeTags(tags, timezone)
  if (coded.length === 0) return null

  return (
    <Tooltip
      label={<TagTable tags={tags} onSearchTag={(ns, v) => onSearchTag(buildNamespacedTag(ns, v))} />}
      wrapperStyle={{ display: 'block' }}
    >
      <span className="tags tag-tooltip">
        {coded.map((tag, i) => (
          <span key={i}>
            <span
              className={`${tag.namespace}-tag`}
              style={{ cursor: 'pointer' }}
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                onSearchTag(tag.text)
              }}
            >
              {tag.text}
            </span>
            {i < coded.length - 1 && ', '}
          </span>
        ))}
      </span>
    </Tooltip>
  )
}

/** Mirrors legacy's exact thumbnail card markup (`buildThumbnailDiv` in
 * `~/LANraragi/public/js/mod/common.js`) — `div.id1` > (`div.id2` status icons + title, `div.id3`
 * cover image + bookmark icon, `div.id4` page count + tags) — so the copied theme CSS
 * (`useApplyTheme`) styles it identically. Right-click opens `ArchiveContextMenu` (real functional
 * parity); multi-select mode overlays a checkbox instead of navigating on click. */
function ArchiveCard({
  archive,
  multiSelect,
  selected,
  cropThumbs,
  onToggleSelect,
  onContextMenu,
  onOpen,
  onSearchTag,
}: {
  archive: ArchiveMetadata
  multiSelect: boolean
  selected: boolean
  cropThumbs: boolean
  onToggleSelect: (id: string) => void
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  onSearchTag: (namespacedTag: string) => void
}) {
  const id = archive.arcid
  const isTank = isTankoubonId(id)
  const [thumbLoaded, setThumbLoaded] = useState(false)
  const [thumbFailed, setThumbFailed] = useState(false)

  function handleOpen(e: MouseEvent) {
    e.preventDefault()
    if (multiSelect) {
      onToggleSelect(id)
    } else {
      onOpen(id)
    }
  }

  const thumbSrc = isTank
    ? `/api/tankoubons/${id}/thumbnail?no_fallback=true`
    : `/api/archives/${id}/thumbnail?no_fallback=true`

  return (
    <div
      className={`id1${selected ? ' msm-selected' : ''}`}
      id={id}
      onContextMenu={(e) => onContextMenu(e, archive)}
    >
      <div className="id2">
        <StatusIcons archive={archive} />
        <a href={routes.reader(id)} title={archive.title} onClick={handleOpen}>
          {archive.title}
        </a>
      </div>
      <div className={cropThumbs ? 'id3' : 'id3 nocrop'} style={{ position: 'relative' }}>
        {multiSelect && (
          <input
            type="checkbox"
            checked={selected}
            onChange={() => onToggleSelect(id)}
            style={{ position: 'absolute', top: 6, left: 6, zIndex: 1, width: 20, height: 20 }}
          />
        )}
        <a href={routes.reader(id)} title={archive.title} onClick={handleOpen}>
          {!thumbLoaded && !thumbFailed && (
            <>
              <img style={{ position: 'relative' }} src="/legacy/img/wait_warmly.jpg" alt="" />
              <i className="fa fa-4x fa-cog fa-spin ttspinner" aria-hidden="true"></i>
            </>
          )}
          <img
            src={thumbFailed ? '/legacy/img/noThumb.png' : thumbSrc}
            alt={archive.title}
            style={thumbLoaded || thumbFailed ? undefined : { display: 'none' }}
            onLoad={() => setThumbLoaded(true)}
            onError={() => setThumbFailed(true)}
          />
        </a>
        {!isTank && <BookmarkIcon archiveId={id} />}
      </div>
      <div className="id4">
        <PageCountBadge archive={archive} />
        <TagLine tags={archive.tags} onSearchTag={onSearchTag} />
      </div>
    </div>
  )
}

/** Read-only card renderer for the "Recently Added"/On Deck/Random/etc. carousel — same markup as
 * `ArchiveCard` minus multi-select and context-menu source tracking differences (the carousel
 * still opens the same context menu, just tagged `source: 'carousel'` so cross-archive nav can
 * later tell the two apart, matching legacy's own `window.contextMenuSource`). */
function CarouselCard({
  archive,
  cropThumbs,
  onContextMenu,
  onOpen,
  onSearchTag,
}: {
  archive: ArchiveMetadata
  cropThumbs: boolean
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  /** Optional — `SelectedArchiveSlide`'s own multi-select-mode selection list doesn't have a
   * meaningful "search" action for its slides (clicking there removes the archive from the
   * selection instead, via `onOpen`), so it's fine to omit and fall back to a no-op. The
   * On Deck/Random/Inbox/Untagged carousel *does* wire this through to the real in-app search —
   * previously a bare no-op there too, a real, live-reported bug: clicking a tag in this
   * carousel's tooltip visibly did nothing at all (worse than the main grid's own tags, which at
   * least still navigate via a real `href` on middle/right-click even before `onSearchTag` was
   * fixed — this carousel's `<a>` click handler calls `e.preventDefault()` unconditionally, so a
   * no-op handler swallowed the click with no fallback whatsoever). */
  onSearchTag?: (namespacedTag: string) => void
}) {
  return (
    <ArchiveCard
      archive={archive}
      multiSelect={false}
      selected={false}
      cropThumbs={cropThumbs}
      onToggleSelect={() => {}}
      onContextMenu={onContextMenu}
      onOpen={onOpen}
      onSearchTag={onSearchTag ?? (() => {})}
    />
  )
}

const CAROUSEL_ICON: Record<CarouselMode, string> = {
  ondeck: 'fa-book-reader',
  random: 'fa-random',
  inbox: 'fa-envelope-open-text',
  untagged: 'fa-edit',
}

/** "Recently Added" carousel (`~/LANraragi/templates/index.html.tt2`'s `.index-carousel` +
 * `index.js`'s `toggleCarousel`/`updateCarousel`) — collapsible (state persisted to
 * `localStorage.carouselOpen`), a Swiper slider (`navigation` module only — legacy's own
 * `virtual`-slides perf mode isn't reproduced, this app's libraries/result sets are far smaller
 * than legacy's largest real deployments), a mode switcher persisted to
 * `localStorage.carouselType`, and a refresh button. Endpoint/params per mode mirror
 * `updateCarousel` exactly, including which of the *other* three flags each mode always forces on
 * regardless of the index settings menu. */
/** Resolves a single selected archive/Tankoubon ID to its card-displayable metadata (a bare
 * hook-in-a-loop isn't possible for a variable-length `selectedIds` set, hence its own
 * component) — used by the multi-select mode's selection-list body. Renders nothing while its
 * own fetch is in flight rather than a per-slide spinner (matches the small, session-scale
 * selection sizes this is meant for). */
function SelectedArchiveSlide({
  id,
  cropThumbs,
  onContextMenu,
  onRemove,
}: {
  id: string
  cropThumbs: boolean
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onRemove: (id: string) => void
}) {
  const metadata = useArchiveMetadata(id)
  if (!metadata.data) return null
  return (
    <SwiperSlide style={{ width: 228 }}>
      <CarouselCard
        archive={metadata.data}
        cropThumbs={cropThumbs}
        onContextMenu={onContextMenu}
        onOpen={() => onRemove(id)}
      />
    </SwiperSlide>
  )
}

function RecentlyAddedCarousel({
  filter,
  category,
  hideCompleted,
  groupbyTanks,
  cropThumbs,
  onContextMenu,
  onOpen,
  multiSelect,
  selectedIds,
  onToggleSelected,
  onSelectPage,
  onClearSelection,
  onRunBatch,
  onMerge,
  canMerge,
  onSearchTag,
  refreshKey,
}: {
  filter: string
  category: string
  hideCompleted: boolean
  groupbyTanks: boolean
  cropThumbs: boolean
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata, source: 'carousel') => void
  onOpen: (id: string) => void
  multiSelect: boolean
  selectedIds: Set<string>
  onToggleSelected: (id: string) => void
  onSelectPage: () => void
  onClearSelection: () => void
  onRunBatch: () => void
  onMerge: () => void
  canMerge: boolean
  onSearchTag: (namespacedTag: string) => void
  /** Bumped by the parent whenever something outside this component's own control (the
   * "Mark as Read"/"Mark as Unread" context-menu action, currently the only such case) changes
   * archive progress data this carousel's own fetch effect has no other way to learn about — this
   * carousel doesn't use TanStack Query (a plain `useEffect`+`fetch`, unlike the main grid's own
   * `useSearch`), so a parent-side `invalidateQueries` call has no effect on it at all. */
  refreshKey: number
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(() => localStorage.getItem(CAROUSEL_OPEN_KEY) === '1')
  const [mode, setMode] = useState<CarouselMode>(
    () => (localStorage.getItem(CAROUSEL_TYPE_KEY) as CarouselMode | null) ?? 'ondeck',
  )
  const [menuOpen, setMenuOpen] = useState(false)
  const [items, setItems] = useState<ArchiveMetadata[]>([])
  const [loading, setLoading] = useState(false)
  const [nonce, setNonce] = useState(0)

  useEffect(() => {
    localStorage.setItem(CAROUSEL_OPEN_KEY, open ? '1' : '0')
  }, [open])

  // The mode-switch "..." dropdown is its own local overlay (not the page-level
  // `contextMenu`/`categoryOverflowOpen` state Escape already clears in the parent), so it needs
  // its own listener to close on Escape too.
  useEffect(() => {
    if (!menuOpen) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') setMenuOpen(false)
    }
    document.addEventListener('keydown', onKeyDown)
    return () => document.removeEventListener('keydown', onKeyDown)
  }, [menuOpen])
  useEffect(() => {
    localStorage.setItem(CAROUSEL_TYPE_KEY, mode)
  }, [mode])

  // Legacy's own `enterSelectionCarouselMode` (`index.js`) force-expands the carousel — entering
  // MSM with it collapsed would otherwise hide the very selection list the mode exists to show.
  // A derived value (not an effect calling `setOpen`) since this is plain synchronous rendering
  // logic, not a sync-with-an-external-system concern — `open` itself still holds only the user's
  // own manually-toggled preference (that's what's persisted to `localStorage` above), unaffected
  // by MSM's temporary forced-expand.
  const isOpen = open || multiSelect

  useEffect(() => {
    // No mode-based fetch while in selection mode — legacy doesn't refresh carousel data during
    // MSM either, the carousel is repurposed to display the selection itself instead.
    if (!isOpen || multiSelect) return
    const params = new URLSearchParams()
    if (filter) params.set('filter', filter)
    const isBuiltinSelector = category === NEW_ONLY || category === UNTAGGED_ONLY
    if (category && !isBuiltinSelector) params.set('category', category)
    if (!groupbyTanks) params.set('groupby_tanks', 'false')
    if (hideCompleted) params.set('hidecompleted', 'true')
    if (category === NEW_ONLY) params.set('newonly', 'true')
    if (category === UNTAGGED_ONLY) params.set('untaggedonly', 'true')

    let endpoint: string
    switch (mode) {
      case 'random':
        params.set('count', '15')
        endpoint = `/api/search/random?${params.toString()}`
        break
      case 'inbox':
        params.set('newonly', 'true')
        params.set('sortby', 'date_added')
        params.set('order', 'desc')
        params.set('start', '-1')
        endpoint = `/api/search?${params.toString()}`
        break
      case 'untagged':
        params.set('untaggedonly', 'true')
        params.set('sortby', 'date_added')
        params.set('order', 'desc')
        params.set('start', '-1')
        endpoint = `/api/search?${params.toString()}`
        break
      default:
        params.set('sortby', 'lastread')
        params.set('hidecompleted', 'true')
        endpoint = `/api/search?${params.toString()}`
        break
    }

    let cancelled = false
    // `setLoading(true)` is deferred a tick rather than called synchronously in the effect body —
    // this is a real network request kicking off (an external-system interaction, not a plain
    // state sync), and react-hooks' `set-state-in-effect` rule flags direct synchronous setState
    // calls in an effect body as a cascading-render risk regardless.
    queueMicrotask(() => {
      if (!cancelled) setLoading(true)
    })
    fetch(endpoint)
      .then((r) => r.json())
      .then((data) => {
        if (cancelled) return
        setItems(data.data ?? [])
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }
     
  }, [isOpen, mode, filter, category, hideCompleted, groupbyTanks, nonce, multiSelect, refreshKey])

  const modeLabel: Record<CarouselMode, string> = {
    ondeck: t('On Deck'),
    random: t('Random'),
    inbox: t('New Archives'),
    untagged: t('Untagged Archives'),
  }

  return (
    <ul className="collapsible index-carousel with-right-caret">
      {/* Real legacy class list is exactly `collapsible index-carousel with-right-caret` with no
          inline style — `index-carousel`'s CSS (`lrr.css`) supplies the panel's own margins/inset,
          and `.option-flyout>.collapsible-title` is a direct-child selector, so
          `.collapsible-title`/`.collapsible-right` must stay direct children of `.option-flyout`
          (no wrapper div) or that styling silently drops. */}
      <li
        className="option-flyout"
        style={{ display: 'flex', flexWrap: 'wrap', justifyContent: 'space-between' }}
      >
        {/* Matches legacy's real two-sibling split — `caret-right`'s CSS `::after` glyph paints
            at the end of whichever element carries the class, so keeping the refresh/more-options
            buttons out of `.collapsible-title` entirely puts the caret right after "On Deck"
            instead of at the far right past both buttons. */}
        <div
          className={`collapsible-title caret-right${isOpen ? ' active' : ''}`}
          onClick={() => setOpen((o) => !o)}
          style={{ display: 'flex', alignItems: 'center', flex: '1 1 0', overflow: 'hidden' }}
        >
          {/* Legacy's `enterSelectionCarouselMode`/`exitSelectionCarouselMode` (`index.js`) swap
              this same header's icon/text in place rather than showing a second header — MSM is a
              *mode the carousel itself enters*, not a separate panel underneath it. */}
          <i className={multiSelect ? 'fas fa-check-square' : `fa ${CAROUSEL_ICON[mode]}`} aria-hidden="true"></i>
          <div style={{ marginLeft: 8 }}>{multiSelect ? t('Selection') : modeLabel[mode]}</div>
        </div>
        {isOpen && multiSelect && (
          <div className="collapsible-right" onClick={(e) => e.stopPropagation()}>
            {/* Legacy's real 4-button MSM toolbar lives in this exact `.collapsible-right`
                slot, replacing the refresh/more-options icons. `updateSelectionCount` hides
                batch-ops/merge/clear at zero selected — only select-page stays visible. */}
            {selectedIds.size > 0 && (
              <span>{t('{{n}} selected', { n: selectedIds.size })}</span>
            )}
            {/* No `marginBottom` offset on these four, unlike the refresh/more-options icons
                below (`margin-bottom: 0px` vs. `-4px` in legacy's real computed style). */}
            {selectedIds.size > 0 && (
              <a
                href="#"
                className="fa fa-2x fa-hammer"
                style={{ marginLeft: 12 }}
                title={t('Run Batch Operations on selection') ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  onRunBatch()
                }}
              ></a>
            )}
            {canMerge && (
              <a
                href="#"
                className="fa fa-2x fa-compress-alt"
                style={{ marginLeft: 12 }}
                title={t('Merge Archives into Tankoubon') ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  onMerge()
                }}
              ></a>
            )}
            {selectedIds.size > 0 && (
              <a
                href="#"
                className="fa fa-2x fa-eject"
                style={{ marginLeft: 12 }}
                title={t('Clear selection') ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  onClearSelection()
                }}
              ></a>
            )}
            <a
              href="#"
              className="fa fa-2x fa-check-double"
              style={{ marginLeft: 12 }}
              title={t('Select All in Page') ?? undefined}
              onClick={(e) => {
                e.preventDefault()
                onSelectPage()
              }}
            ></a>
          </div>
        )}
        {isOpen && !multiSelect && (
          <div className="collapsible-right" onClick={(e) => e.stopPropagation()}>
            <a
              href="#"
              className={`fa fa-2x fa-sync${loading ? ' fa-spin' : ''}`}
              style={{ marginBottom: -4 }}
              title={t('Refresh') ?? undefined}
              onClick={(e) => {
                e.preventDefault()
                setNonce((n) => n + 1)
              }}
            ></a>
            <span style={{ position: 'relative' }}>
              <a
                href="#"
                className="fa fa-2x fa-ellipsis-h"
                style={{ marginBottom: -4, marginLeft: 12 }}
                title={t('Carousel Mode') ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  setMenuOpen((m) => !m)
                }}
              ></a>
              {menuOpen && (
                // Not portaled — positioned via `top: '100%'`/`right: 0` against this menu's own
                // trigger `<span style={{ position: 'relative' }}>`, which a default portal to
                // `document.body` would detach it from.
                <PopupMenu
                  portal={false}
                  style={{ position: 'absolute', top: '100%', right: 0, zIndex: Z_OVERLAY_CONTENT }}
                >
                  {(['ondeck', 'random', 'inbox', 'untagged'] as CarouselMode[]).map((m) => (
                    <PopupMenuItem
                      key={m}
                      style={{ fontWeight: m === mode ? 'bold' : undefined }}
                      onClick={() => {
                        setMode(m)
                        setMenuOpen(false)
                      }}
                    >
                      <i className={`fa ${CAROUSEL_ICON[m]}`} aria-hidden="true"></i> {modeLabel[m]}
                    </PopupMenuItem>
                  ))}
                </PopupMenu>
              )}
            </span>
          </div>
        )}
        {isOpen && multiSelect && (
          // Legacy's carousel body is repurposed into the selection list itself during MSM,
          // reusing the same empty-state icon/copy the normal "no results" state has.
          //
          // `boxSizing: 'border-box'` matters here: `.option-flyout>.collapsible-body`'s real CSS
          // gives it `padding: 10px !important`, which under content-box sizing would add on top
          // of `width: 100%` instead of being included in it, overflowing the `<li>`'s right edge.
          <div className="collapsible-body" style={{ width: '100%', boxSizing: 'border-box' }}>
            {selectedIds.size === 0 ? (
              /* Real legacy `#carousel-empty`: a fixed `height: 344px` flex column, centered both
                 axes, not a content-flow `<p>` with a `<br>`. */
              <div style={{ height: 344, display: 'flex', justifyContent: 'center', alignItems: 'center', flexDirection: 'column' }}>
                <i className="fa fa-glasses fa-4x" aria-hidden="true"></i>
                <span style={{ marginTop: 12 }}>
                  {t('Click Archives to add them to the selection. Your selection carries over across searches.')}
                </span>
              </div>
            ) : (
              <Swiper
                modules={[Navigation, Mousewheel]}
                navigation={{ nextEl: '.carousel-next', prevEl: '.carousel-prev' }}
                mousewheel
                spaceBetween={8}
                slidesPerView="auto"
                // No side padding — legacy's `.index-carousel-container` has `padding: 0`; the
                // prev/next arrows overlay the slide track's own edge rather than a reserved gutter.
                style={{ padding: '8px 0' }}
              >
                {[...selectedIds].map((id) => (
                  <SelectedArchiveSlide
                    key={id}
                    id={id}
                    cropThumbs={cropThumbs}
                    onContextMenu={(e, archive) => onContextMenu(e, archive, 'carousel')}
                    onRemove={onToggleSelected}
                  />
                ))}
                {/* `top: 136` (not `50%`) matches legacy's real `.carousel-prev`/`.carousel-next`
                    rule (`lrr.css`: `position: absolute; top: 136px; left/right: 0; z-index: 20`)
                    — a fixed pixel offset, not vertically-centered. */}
                <a href="#" className="fa fa-3x fa-chevron-left carousel-prev" style={{ position: 'absolute', left: 0, top: 136, cursor: 'pointer', zIndex: 20 }}></a>
                <a href="#" className="fa fa-3x fa-chevron-right carousel-next" style={{ position: 'absolute', right: 0, top: 136, cursor: 'pointer', zIndex: 20 }}></a>
              </Swiper>
            )}
          </div>
        )}
        {isOpen && !multiSelect && (
          // Same `boxSizing: 'border-box'` reasoning as the MSM branch above.
          <div className="collapsible-body" style={{ width: '100%', boxSizing: 'border-box' }}>
            {loading && items.length === 0 ? (
              <div style={{ height: 344, display: 'flex', justifyContent: 'center', alignItems: 'center' }}>
                <i className="fa fa-stroopwafel fa-spin fa-4x" aria-hidden="true"></i>
              </div>
            ) : items.length === 0 ? (
              <div style={{ height: 344, display: 'flex', justifyContent: 'center', alignItems: 'center', flexDirection: 'column' }}>
                <i className="fa fa-glasses fa-4x" aria-hidden="true"></i>
                <span style={{ marginTop: 12 }}>{t('No results here.')}</span>
              </div>
            ) : (
              <Swiper
                modules={[Navigation, Mousewheel]}
                navigation={{ nextEl: '.carousel-next', prevEl: '.carousel-prev' }}
                mousewheel
                spaceBetween={8}
                slidesPerView="auto"
                style={{ padding: '8px 0' }}
              >
                {items.map((a) => (
                  // 228px matches `div.id1`'s real fixed width — a narrower slide box makes each
                  // card visually spill into its neighbor.
                  <SwiperSlide key={a.arcid} style={{ width: 228 }}>
                    <CarouselCard
                      archive={a}
                      cropThumbs={cropThumbs}
                      onContextMenu={(e, archive) => onContextMenu(e, archive, 'carousel')}
                      onOpen={onOpen}
                      onSearchTag={onSearchTag}
                    />
                  </SwiperSlide>
                ))}
                <a href="#" className="fa fa-3x fa-chevron-left carousel-prev" style={{ position: 'absolute', left: 0, top: 136, cursor: 'pointer', zIndex: 20 }}></a>
                <a href="#" className="fa fa-3x fa-chevron-right carousel-next" style={{ position: 'absolute', right: 0, top: 136, cursor: 'pointer', zIndex: 20 }}></a>
              </Swiper>
            )}
          </div>
        )}
      </li>
    </ul>
  )
}

/** Ports legacy's own right-click menu (`~/LANraragi/public/js/mod/index_contextmenu.js`) — same
 * action set and same login-gating (Edit/Delete/Rating/Category only shown when `useLoginStatus`
 * reports logged in). Built entirely from `PopupMenu`/`PopupMenuItem`/`PopupMenuSeparator`
 * (`components/PopupMenu.tsx`) — a from-scratch React component styled with Tailwind + this app's
 * own `MENU_PALETTE` colour table, matching each of legacy's 5 real themes without depending on
 * any menu-plugin's markup or CSS. Closes on any outside click or right-click. */
function ArchiveContextMenu({
  state,
  categories,
  loggedIn,
  liveArchives,
  onClose,
  onToggleCategory,
  onDelete,
  onOpen,
  onRatingChange,
  onToggleSelection,
  isSelected,
  onSetProgress,
}: {
  state: ContextMenuState
  categories: { id: string; name: string; search: string | null; archives: string[] }[] | undefined
  loggedIn: boolean
  /** The live, refetch-synced search results (`shown` in the parent) — `state.archive` itself is
   * a one-time snapshot taken at right-click time and never updated, which used to be harmless
   * (every action that changed the archive also closed the menu immediately) but broke once the
   * rating row became a persistent, stay-open-after-click control: clicking a star correctly
   * updated the archive in Redis, but the menu kept rendering the stale pre-click tags until
   * closed and reopened. Looked up by ID so the rating row (and anything else keying off
   * `archive.tags`) reflects the real just-saved value without needing its own separate refetch. */
  liveArchives: ArchiveMetadata[]
  onClose: () => void
  onToggleCategory: (categoryId: string, archiveId: string, currentlyIn: boolean) => void
  onDelete: (archiveId: string, isTank: boolean) => void
  onOpen: (id: string) => void
  onRatingChange: (archiveId: string, isTank: boolean, rating: string | null) => void
  onToggleSelection: (id: string) => void
  isSelected: boolean
  /** "Mark as Read"/"Mark as Unread" — the actual mutation lives in the parent `Library`
   * component, not here, because `onClose()` (called first, on the same click) unmounts this
   * whole menu immediately; a `useMutation` instance owned by *this* component would have its
   * observer torn down before the mutation's async response ever arrives, silently dropping
   * whatever `onSuccess` callback was passed to that particular `mutate()` call (TanStack Query's
   * own `hasListeners()` guard on delivering per-call `mutate(vars, { onSuccess })` callbacks — a
   * real, live-confirmed bug: the write to Redis genuinely succeeded and the main grid's `invalidate
   * Queries`-driven refetch picked it up fine since that mutation is defined in the *parent*, but
   * a second effect meant to also refresh the On Deck carousel never fired at all, because it was
   * wired through this component's own now-torn-down mutation instance instead). */
  onSetProgress: (archiveId: string, page: number) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { x, y } = state
  const archive = liveArchives.find((a) => a.arcid === state.archive.arcid) ?? state.archive
  const isTank = isTankoubonId(archive.arcid)
  const staticCategories = (categories ?? []).filter((c) => !c.search)
  const [categoryMenuOpen, setCategoryMenuOpen] = useState(false)
  const palette = useMenuPalette()

  // Submenu opens on hover (matching legacy's `jquery-contextMenu`, and standard desktop
  // context-menu behavior generally) rather than click. A short close delay absorbs the mouse
  // briefly leaving the trigger row while crossing the gap into the submenu itself.
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  function openSubmenu(which: 'category') {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current)
    setCategoryMenuOpen(which === 'category')
  }
  function scheduleCloseSubmenus() {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current)
    closeTimerRef.current = setTimeout(() => {
      setCategoryMenuOpen(false)
    }, 200)
  }

  function copyLink() {
    const url = `${window.location.origin}${routes.reader(archive.arcid)}`
    navigator.clipboard
      .writeText(url)
      .then(() => toast({ heading: t('Link copied to clipboard!') ?? undefined, icon: 'info', hideAfter: 3000 }))
      .catch(() => toast({ heading: t('Failed to copy link.') ?? undefined, icon: 'error' }))
  }

  return (
    <>
      {/* Full-viewport transparent overlay — the standard "click outside to dismiss" pattern for
          a positioned popup, cheaper than a document-level listener + ref check. */}
      <div
        style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={onClose}
        onContextMenu={(e) => {
          e.preventDefault()
          onClose()
        }}
      />
      <PopupMenu style={{ position: 'fixed', top: y, left: x, zIndex: Z_OVERLAY_CONTENT }}>
        {loggedIn && (
          <>
            {/* A compact icon-only row at the very top of the menu (Firefox's own right-click
                menu puts Back/Forward/Reload/Bookmark the same way) rather than a full-width
                "Add Rating" row that opens a whole separate hover submenu — the star widget's own
                click targets are already precise enough that a submenu was pure overhead. Not a
                `PopupMenuItem` (no hover-highlight-the-whole-row/click-closes-menu behavior makes
                sense for a row of independent controls). */}
            <li
              className="flex items-center justify-center gap-1 px-2 pt-1"
              style={{ paddingBottom: '.45em', borderBottom: `1px solid ${palette.separator}`, marginBottom: '.35em' }}
            >
              <RatingWidget
                archiveId={archive.arcid}
                tags={archive.tags}
                size={16}
                onChange={(nextTags) => {
                  const tagsByNamespace = splitTagsByNamespace(nextTags)
                  const rating = tagsByNamespace.rating?.[0] ?? null
                  onRatingChange(archive.arcid, isTank, rating)
                }}
              />
            </li>
          </>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose()
            onOpen(archive.arcid)
          }}
        >
          <i className="fa fa-book-open" style={{ width: 18 }}></i> {t('Read')}
        </PopupMenuItem>
        {/* Not offered on a Tankoubon — it's an aggregate container with no single `progress`/
            `pagecount` of its own (each member archive tracks its own separately), so "mark this
            one thing as read" doesn't have a single well-defined target the way it does for a
            plain archive. Toggles on `progress > 0` (any progress at all counts as "not unread"),
            not the 85%-complete threshold `hidecompleted`/On Deck use elsewhere — those answer a
            different question ("is this basically finished, worth hiding from an in-progress
            list") than this menu item's own binary read/unread state. */}
        {!isTank && archive.pagecount > 0 && (
          <PopupMenuItem
            onClick={() => {
              onClose()
              onSetProgress(archive.arcid, archive.progress > 0 ? 0 : archive.pagecount)
            }}
          >
            <i className={`fa ${archive.progress > 0 ? 'fa-eye-slash' : 'fa-eye'}`} style={{ width: 18 }}></i>{' '}
            {archive.progress > 0 ? t('Mark as Unread') : t('Mark as Read')}
          </PopupMenuItem>
        )}
        {!isTank && (
          <PopupMenuItem
            onClick={() => {
              onClose()
              window.location.assign(`/api/archives/${archive.arcid}/download`)
            }}
          >
            <i className="fa fa-download" style={{ width: 18 }}></i> {t('Download')}
          </PopupMenuItem>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose()
            copyLink()
          }}
        >
          <i className="fa fa-link" style={{ width: 18 }}></i> {t('Copy Link')}
        </PopupMenuItem>
        <PopupMenuItem
          onClick={() => {
            onClose()
            onToggleSelection(archive.arcid)
          }}
        >
          <i className="fa fa-check-square" style={{ width: 18 }}></i>{' '}
          {isSelected ? t('Remove from Selection') : t('Add to Selection')}
        </PopupMenuItem>
        {loggedIn && (
          <>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose()
                navigate(routes.edit(archive.arcid))
              }}
            >
              <i className="fa fa-pen" style={{ width: 18 }}></i> {t('Edit Metadata')}
            </PopupMenuItem>
            <PopupMenuItem style={{ position: 'relative' }} onMouseEnter={() => openSubmenu('category')} onMouseLeave={scheduleCloseSubmenus}>
              <i className="fa fa-search-plus" style={{ width: 18 }}></i> {t('Add to Category')}
              {categoryMenuOpen && (
                <PopupMenu
                  portal={false}
                  style={{ position: 'absolute', left: '100%', top: 0, maxHeight: 220, overflowY: 'auto' }}
                  onMouseEnter={() => openSubmenu('category')}
                  onMouseLeave={scheduleCloseSubmenus}
                >
                  {staticCategories.length === 0 && (
                    <PopupMenuItem disabled>{t('No categories found.')}</PopupMenuItem>
                  )}
                  {staticCategories.map((c) => {
                    const currentlyIn = c.archives.includes(archive.arcid)
                    return (
                      <PopupMenuItem key={c.id} onClick={() => onToggleCategory(c.id, archive.arcid, currentlyIn)}>
                        <input type="checkbox" readOnly checked={currentlyIn} style={{ verticalAlign: 'middle' }} /> {c.name}
                      </PopupMenuItem>
                    )
                  })}
                </PopupMenu>
              )}
            </PopupMenuItem>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose()
                onDelete(archive.arcid, isTank)
              }}
            >
              <i className="fa fa-trash" style={{ width: 18 }}></i> {t('Delete')}
            </PopupMenuItem>
          </>
        )}
      </PopupMenu>
    </>
  )
}

/** Styled delete-confirmation popup (legacy's `LRR.showPopUp` — a SweetAlert2 dialog), replacing
 * a plain `window.confirm`, with text that differs for a Tankoubon vs a plain archive (legacy's
 * own `ConfirmTankoubonDeletion`/`ConfirmArchiveDeletion` distinction). */
function DeleteConfirmDialog({
  isTank,
  onConfirm,
  onCancel,
}: {
  isTank: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  return (
    <>
      <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: 'rgba(0,0,0,0.4)' }} onClick={onCancel} />
      <div
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: Z_OVERLAY_CONTENT,
          width: 360,
          padding: 20,
          textAlign: 'center',
          background: '#fff',
          border: '1px solid #bebebe',
          borderRadius: '.2em',
          boxShadow: '0 2px 5px rgba(0,0,0,.5)',
        }}
      >
        <i className="fa fa-exclamation-triangle fa-2x" style={{ color: '#d33' }} aria-hidden="true"></i>
        <p>
          {isTank
            ? t('This will delete this Tankoubon grouping (archives inside it are not deleted).')
            : t('This will delete both metadata and matching files from your system! Please use with caution.')}
        </p>
        <div style={{ display: 'flex', justifyContent: 'center', gap: 12, marginTop: 12 }}>
          <input type="button" className="stdbtn" value={t('Cancel') ?? undefined} onClick={onCancel} />
          <input
            type="button"
            className="stdbtn"
            style={{ background: '#d33', color: 'white' }}
            value={t('Yes, delete it') ?? undefined}
            onClick={onConfirm}
          />
        </div>
      </div>
    </>
  )
}

/** One compact-table custom column's chosen namespace, read/write straight to its own
 * `localStorage` key (`customColumn${index}`) — ports `generateTableHeaders`'s per-header default
 * (`artist`/`series` for columns 1/2, `Header N` beyond that) and `handleColumnNum`'s rename flow
 * (legacy uses an inline pencil-icon-triggered prompt; reproduced here the same way rather than a
 * persistent input, since this is an infrequent per-column configuration action, not a per-row
 * one). */
function useCustomColumnNamespace(index: number): [string, (v: string) => void] {
  const key = `${CUSTOM_COLUMN_PREFIX}${index}`
  const [namespace, setNamespaceState] = useState(
    () => localStorage.getItem(key) ?? DEFAULT_CUSTOM_COLUMNS[index - 1] ?? `Header ${index}`,
  )
  const setNamespace = (v: string) => {
    setNamespaceState(v)
    localStorage.setItem(key, v)
  }
  return [namespace, setNamespace]
}

function CustomColumnHeader({ index }: { index: number }) {
  const { t } = useTranslation()
  const [namespace, setNamespace] = useCustomColumnNamespace(index)
  return (
    <th>
      {namespace.charAt(0).toUpperCase() + namespace.slice(1)}{' '}
      <i
        className="fas fa-pencil-alt edit-header-btn"
        title={t('Edit this column') ?? undefined}
        style={{ cursor: 'pointer' }}
        onClick={() => {
          void (async () => {
            const next = await promptDialog(t('Tag namespace') ?? '', namespace)
            if (next?.trim()) setNamespace(next.trim())
          })()
        }}
      ></i>
    </th>
  )
}

/** Ports `renderColumn` exactly: extracts every value under this column's chosen namespace out of
 * the archive's full `tags` string (regex, not `splitTagsByNamespace`, to match legacy's own
 * substring-match behavior byte-for-byte), formats dates via `convertTimestamp`, capitalizes
 * every other value's words (skipping `source`, since that's a URL), and links each to a search
 * for that exact tag. */
function CustomColumnCell({
  index,
  tags,
  onSearchTag,
}: {
  index: number
  tags: string
  onSearchTag: (namespacedTag: string) => void
}) {
  const [namespace] = useCustomColumnNamespace(index)
  // Server timezone for `date_added`/`timestamp` custom-column display + search URL — same
  // pattern as `TagTable`/`ArchiveOverviewOverlay`'s own `TagsTable`.
  const timezone = useSettings().data?.timezone ?? ''
  const matches = [...tags.matchAll(new RegExp(`${namespace}:([^,]+)`, 'g'))].map((m) => m[1].trim())
  const isDate = namespace === 'date_added' || namespace === 'timestamp'
  return (
    <td style={{ textAlign: 'left' }}>
      {matches.map((raw, i) => {
        const text = isDate ? formatTimestampForDisplay(raw, timezone) : namespace === 'source' ? raw : raw.replace(/\b./g, (c) => c.toUpperCase())
        return (
          <span key={i}>
            <a
              href={getTagSearchURL(namespace, raw, timezone)}
              style={{ cursor: 'pointer' }}
              onClick={(e) => {
                e.preventDefault()
                // `tagValueForSearch`, not the raw stored value — same real bug class as
                // `TagTable.tsx`'s own fix: a `date_added`/`timestamp` value's search semantics
                // are the `yyyy-mm-dd` day-range syntax, not its bare Unix-seconds form (which
                // never matches, `date_added` isn't tag-indexed). This in-app click path bypassed
                // the same conversion the `href` above already applies.
                onSearchTag(buildNamespacedTag(namespace, tagValueForSearch(namespace, raw, timezone)))
              }}
            >
              {text}
            </a>
            {i < matches.length - 1 && ', '}
          </span>
        )
      })}
    </td>
  )
}

/** Settings gear menu (legacy's `#settings-menu` contextMenu, `index.js:117-199`) — bundles
 * Display Mode (thumbnail grid vs compact table), Crop Thumbnails, Hide Completed, and Group
 * Tankoubons into one dropdown, each persisted to the same `localStorage` keys legacy itself
 * uses. Positioned next to "Go to Page", matching legacy's own placement. */
function SettingsMenu({
  viewMode,
  setViewMode,
  cropThumbs,
  setCropThumbs,
  hideCompleted,
  setHideCompleted,
  groupbyTanks,
  setGroupbyTanks,
}: {
  viewMode: 'thumbnail' | 'compact'
  setViewMode: (v: 'thumbnail' | 'compact') => void
  cropThumbs: boolean
  setCropThumbs: (v: boolean) => void
  hideCompleted: boolean
  setHideCompleted: (v: boolean) => void
  groupbyTanks: boolean
  setGroupbyTanks: (v: boolean) => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  // Which side the menu opens toward — decided fresh each time it opens, not a fixed direction,
  // from the gear icon's own position: opens
  // toward the side with more room, so it never gets clipped by the viewport edge regardless of
  // where "Go to Page"/the gear ends up sitting (this toolbar is at the far right of the page, so
  // a hardcoded direction is wrong in one direction or the other depending on viewport width).
  const [openTowardLeft, setOpenTowardLeft] = useState(false)
  const ref = useRef<HTMLSpanElement>(null)
  // The menu itself is portaled to `document.body` (see PopupMenu's doc comment), so it's no
  // longer a DOM descendant of `ref` — checking only `ref.current.contains(...)` below would
  // treat every click *inside* the open menu as an "outside" click and close it before the
  // item's own onClick fires. Also checking this second ref against the portaled `<ul>` fixes it.
  const menuRef = useRef<HTMLUListElement>(null)

  useEffect(() => {
    if (!open) return
    function onClick(e: globalThis.MouseEvent) {
      const target = e.target as Node
      if (ref.current?.contains(target)) return
      if (menuRef.current?.contains(target)) return
      setOpen(false)
    }
    document.addEventListener('click', onClick)
    return () => document.removeEventListener('click', onClick)
  }, [open])

  return (
    <span ref={ref} style={{ position: 'relative', marginLeft: 6 }}>
      <a
        href="#"
        className="fa fa-cog fa-2x table-option"
        title={t('Index Settings') ?? undefined}
        onClick={(e) => {
          e.preventDefault()
          if (!open && ref.current) {
            const rect = ref.current.getBoundingClientRect()
            const spaceRight = window.innerWidth - rect.right
            // Menu itself is ~220px (`PopupMenu`'s own min-width) — opens left if the right side
            // genuinely doesn't have room for it, not just "less than the left side".
            setOpenTowardLeft(spaceRight < 220)
          }
          setOpen((v) => !v)
        }}
      ></a>
      {open && (
        <PopupMenu
          ref={menuRef}
          // `position: absolute` here is measured against this menu's own trigger `<span
          // ref={ref} style={{ position: 'relative' }}>` (`top: '100%'`/`left`/`right: 0`) — not
          // portaled, so that ancestor is still the one it's positioned against instead of
          // `document.body`. The toolbar this lives in doesn't clip overflow, so there's no
          // clipping problem `portal` would be solving here anyway.
          portal={false}
          style={{
            position: 'absolute',
            top: '100%',
            ...(openTowardLeft ? { right: 0 } : { left: 0 }),
            zIndex: Z_OVERLAY_CONTENT,
          }}
          // The gear icon's own real title (`Index Settings`) — this menu's actual name, as
          // distinct from `Display Mode` right below, which is a sub-heading for just the
          // Thumbnail/Compact radio pair, not the whole menu.
          mainLabel={{ icon: 'fa-cog', text: t('Index Settings') ?? 'Index Settings' }}
        >
          <PopupMenuItem disabled>
            <i className="fas fa-table" style={{ width: 18 }}></i> {t('Display Mode')}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setViewMode('thumbnail')}>
            <input type="radio" readOnly checked={viewMode === 'thumbnail'} /> {t('Thumbnail')}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setViewMode('compact')}>
            <input type="radio" readOnly checked={viewMode === 'compact'} /> {t('Compact')}
          </PopupMenuItem>
          <PopupMenuSeparator />
          <PopupMenuItem onClick={() => setCropThumbs(!cropThumbs)}>
            <input type="checkbox" readOnly checked={cropThumbs} /> {t('Crop thumbnails')}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setHideCompleted(!hideCompleted)}>
            <input type="checkbox" readOnly checked={hideCompleted} /> {t('Hide completed Archives')}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setGroupbyTanks(!groupbyTanks)}>
            <input type="checkbox" readOnly checked={groupbyTanks} /> {t('Group Tankoubons')}
          </PopupMenuItem>
        </PopupMenu>
      )}
    </span>
  )
}

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
      .map((s) => ({ label: s.namespace ? `${s.namespace}:${s.text}` : s.text, weight: s.weight }))
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
                      const next = `${upToCursor}${s.label}`
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
        // Column order/content mirrors legacy's real compact-table columns exactly
        // (`index_datatables.js`'s `columns` array): Title, then `columns` editable custom
        // namespace columns (`customColumn1..N`, default Artist/Series), then a single full Tags
        // column (all namespaces, unfiltered) — legacy has no dedicated Pages/Date Added columns
        // here at all (those were an invented approximation, not what legacy actually shows).
        <table className="itg" style={{ width: '100%' }}>
          <thead>
            <tr>
              <th>{t('Title')}</th>
              {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
                <CustomColumnHeader key={i} index={i} />
              ))}
              <th>{t('Tags')}</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((a) => (
              <tr
                key={a.arcid}
                className={selectedIds.has(a.arcid) ? 'msm-selected' : undefined}
                onContextMenu={(e) => handleContextMenu(e, a)}
              >
                <td style={{ textAlign: 'left' }}>
                  {multiSelect && (
                    <input
                      type="checkbox"
                      checked={selectedIds.has(a.arcid)}
                      onChange={() => toggleSelected(a.arcid)}
                      style={{ marginRight: 6 }}
                    />
                  )}
                  <BookmarkIcon archiveId={a.arcid} />{' '}
                  <a
                    href={routes.reader(a.arcid)}
                    onClick={(e) => {
                      e.preventDefault()
                      if (multiSelect) toggleSelected(a.arcid)
                      else handleOpenArchive(a.arcid)
                    }}
                  >
                    {a.isnew && '🆕 '}
                    {a.title}
                  </a>
                </td>
                {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
                  <CustomColumnCell key={i} index={i} tags={a.tags} onSearchTag={applyTagSearch} />
                ))}
                <td style={{ textAlign: 'left' }}>
                  <TagLine tags={a.tags} onSearchTag={applyTagSearch} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
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
