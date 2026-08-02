import 'swiper/css'
import 'swiper/css/navigation'

import type { MouseEvent } from 'react'
import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Mousewheel, Navigation } from 'swiper/modules'
import { Swiper, SwiperSlide } from 'swiper/react'

import { useArchiveMetadata } from '../../api/hooks'
import type { ArchiveMetadata } from '../../api/types'
import { PopupMenu, PopupMenuItem } from '../../components/PopupMenu'
import { CAROUSEL_OPEN_KEY, CAROUSEL_TYPE_KEY } from '../../storageKeys'
import { Z_OVERLAY_CONTENT } from '../../theme'
import { ArchiveCard } from './ArchiveCard'
import { CAROUSEL_ICON, type CarouselMode, NEW_ONLY, UNTAGGED_ONLY } from './shared'

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

export function RecentlyAddedCarousel({
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
