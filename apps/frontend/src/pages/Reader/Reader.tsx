import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import {
  fetchRandomArchiveId,
  useArchiveMetadata,
  useArchivePages,
  useBookmarkLink,
  useCategories,
  useClearArchiveNew,
  useDeleteTankoubon,
  useGenerateThumbnails,
  useGenerateThumbnailsForArchives,
  useLoginStatus,
  usePageDimensions,
  useSettings,
  useUpdateProgress,
  useUpdateTankoubonProgress,
} from '../../api/hooks'
import Footer from '../../components/Footer'
import Tooltip from '../../components/Tooltip'
import { confirmDialog, promptDialog } from '../../dialog'
import { fetchContentLengthKb } from '../../lib/imageMeta'
import { getTagSearchURL } from '../../lib/tagFormat'
import { routes } from '../../routes'
import { FONT_SIZE_8PT, useApplyTheme } from '../../theme'
import { toast } from '../../toast'
import { useDocumentTitle } from '../../useDocumentTitle'
import { isTankoubonId } from '../Library/shared'
import ArchiveOverviewOverlay from './ArchiveOverviewOverlay'
import {
  type ArchiveNavState,
  resolveAdjacentArchive,
  setupArchiveNavigation,
} from './crossArchiveNav'
import { fileInfoText } from './fileInfoText'
import MarkerLayer from './MarkerLayer'
import { SettingsOverlay } from './SettingsOverlay'
import { clamp, computeNextPage, computeSpread } from './useReaderNavigation'
import { useReaderSettings } from './useReaderSettings'
import { useTankoubonReading } from './useTankoubonReading'

// Faithful port of legacy's reader page (`~/LANraragi/templates/reader.html.tt2` +
// `~/LANraragi/public/js/reader.js`) — real DOM structure (`#i1`-`#i7`) and CSS classnames from
// `/legacy/lrr.css`, not Tailwind.

/** Uniform icon size for every paginator (prev/next-archive, prev/next-page) nav link. `rem`
 * rather than `em` — sized off the root font-size, not whatever this row's own inherited size
 * happens to be, so it doesn't silently shift if an ancestor's font-size ever changes. */
const PAGINATOR_ICON_FONT_SIZE = '1.75rem'
/** `.pagecount`'s own font-size — scaled up alongside the paginator icons above (was inheriting
 * an unset, much smaller ~13px) so the page-number text reads as part of the same control, not a
 * visually separate, undersized label wedged between two oversized icon rows. */
const PAGINATOR_PAGECOUNT_FONT_SIZE = '1.25rem'
/** Matches `toast.tsx`'s own `AUTO_CLOSE_TIME.info` default — specified explicitly to make clear
 * this is deliberate, not incidental inheritance. */
const TOAST_DURATION_MS = 5000

type OverlayKind = 'archive' | 'settings' | 'help' | null

interface WakeLockSentinelLike {
  release(): Promise<void>
  addEventListener(type: 'release', listener: () => void): void
}

/** TanStack Query key for a reader's recommendation shortlist — prefetched near the last page
 * so the boundary panel opens with data already in cache (the LLM rerank takes seconds; the
 * prefetch hides that latency behind the reader's own page-turning). */
const RECS_QUERY_KEY = (id: string) => ['reader-recommendations', id] as const

/** Square badge chip on recommendation-card thumbnails — fixed 16×16 so the chip itself is a
 * square regardless of the emoji glyph inside (which centers via flex). Neutral overlay chrome
 * (semi-transparent dark), readable on any thumbnail in every theme. */
const badgeChipStyle: React.CSSProperties = {
  width: 16,
  height: 16,
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  fontSize: 11,
  lineHeight: 1,
  background: 'rgba(0,0,0,0.55)',
  borderRadius: 4,
}

/** Inline key-cap styling for the help panel's keyboard shortcuts — same visual language as
 * `FilenameTemplateEditor.tsx`'s own `<code>` preview block (`rgba(0,0,0,0.06)` background,
 * `borderRadius: 3`), just inline rather than block-level since these sit mid-sentence. */
function Key({ children }: { children: React.ReactNode }) {
  return (
    <code style={{ padding: '1px 5px', background: 'rgba(0,0,0,0.08)', borderRadius: 3, fontWeight: 'bold' }}>
      {children}
    </code>
  )
}

export function Reader() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { archiveId = null } = useParams<{ archiveId: string }>()
  useApplyTheme()

  // A `TANK_`-prefixed id means "read this Tankoubon as one concatenated multi-archive book"
  // (matches real legacy's own `state.id.startsWith("TANK_")` branch throughout
  // `reader_common.js`/`reader_archive_overlay.js`) rather than a single archive. Both data-
  // fetching paths below are always called (React's Rules of Hooks — a hook call can't be
  // conditional), just with the *other* one's id argument forced to `null` so it's a no-op; their
  // results are merged into the same `metadata`/`pages` variables everything downstream already
  // reads, so the rest of this file doesn't need its own `isTank` branch for every access.
  const isTank = isTankoubonId(archiveId ?? '')
  const singleMetadata = useArchiveMetadata(isTank ? null : archiveId)
  const singlePages = useArchivePages(isTank ? null : archiveId)
  const tankReading = useTankoubonReading(isTank ? archiveId : null)
  const metadata = isTank ? tankReading.metadata : singleMetadata
  const pages = isTank ? tankReading.pages : singlePages
  // Matches legacy's own real `reader.js` behavior exactly (confirmed against the Perl source
  // *and* live via `~/LANraragi/templates/reader.html.tt2`'s `<title>[% title %]</title>` initial
  // server-rendered placeholder, which legacy's own client-side `reader.js` unconditionally
  // overwrites with `document.title = content.title` once the archive's metadata finishes
  // loading — the server-rendered value only ever survives the brief pre-JS window, and was never
  // meant to be this page's real, steady-state tab title. An earlier version of this file got
  // that wrong, assuming the server-rendered `<title>` value was the final one and setting this
  // page's tab title to the plain site title with no archive name at all — a real, user-reported
  // regression.) — just the bare title, once loaded; `undefined` (falls back to the site's own
  // `htmltitle`, matching the pre-load state) while still loading.
  useDocumentTitle(metadata.data?.title)
  const settings = useSettings()
  const loginStatus = useLoginStatus()
  const categories = useCategories()
  const bookmarkLink = useBookmarkLink()
  const updateProgress = useUpdateProgress(isTank ? null : archiveId)
  const updateTankoubonProgress = useUpdateTankoubonProgress(isTank ? archiveId : null)
  const deleteTankoubon = useDeleteTankoubon()
  const generateThumbnails = useGenerateThumbnails(isTank ? '' : (archiveId ?? ''))
  const generateThumbnailsForArchives = useGenerateThumbnailsForArchives()
  const [readerSettings, updateReaderSettings] = useReaderSettings()

  // Legacy fires this unconditionally the moment the reader loads (`reader_common.js`'s init
  // sequence: `DELETE /api/archives/{id}/isnew`, skipped only for a `TANK_` id since tanks have
  // no `isnew` flag of their own) — not tied to finishing the archive or any elapsed time, just
  // "was it opened at least once." That's the `until_opened` badge mode (the default); under
  // `until_finished` or a time-window mode the reader must NOT clear the flag on load, or the
  // badge would vanish after a single open instead of after completion (or when the window
  // lapses, which is display-side only and needs no reader involvement).
  const clearArchiveNew = useClearArchiveNew()
  const clearArchiveNewRef = useRef(clearArchiveNew.mutate)
  clearArchiveNewRef.current = clearArchiveNew.mutate
  // `settings.data?.newbadgemode` (no `?? fallback` here — a `undefined` mode means the settings
  // query is still in flight, and the reader must NOT clear the badge before it knows the mode:
  // with a `?? 'until_opened'` fallback the very first render would default to the legacy mode
  // and clear the flag even under `until_finished`, before the settings ever arrived).
  const newBadgeMode = settings.data?.newbadgemode
  useEffect(() => {
    if (!archiveId || isTank || !newBadgeMode) return
    if (newBadgeMode === 'until_opened') clearArchiveNewRef.current(archiveId)
  }, [archiveId, isTank, newBadgeMode])

  const totalPages = pages.data?.pages.length ?? 0
  const loggedIn = loginStatus.data?.logged_in ?? false

  const params = new URLSearchParams(window.location.search)
  const startPage = Number(params.get('p')) || null

  const [pageOverride, setPageOverride] = useState<number | null>(startPage)
  // Whether the overlay's initial value came from `showOverlayByDefault` auto-opening it (true on
  // mount, before any click) rather than a real click on the grid button — only the latter should
  // auto-scroll/highlight the current page's thumbnail (`ArchiveOverviewOverlay`'s own `autoFocus`
  // prop below): auto-opening on every single page load *and* also yanking the scroll position to
  // hunt down the current page every time was a real, reported annoyance, even though auto-opening
  // itself is an intentional, user-requested feature (no legacy equivalent) worth keeping as-is.
  const openedByDefaultSetting = useRef(readerSettings.showOverlayByDefault)
  const [overlay, setOverlay] = useState<OverlayKind>(
    readerSettings.showOverlayByDefault ? 'archive' : null,
  )
  const [isFullscreen, setIsFullscreen] = useState(false)
  const [widespreads, setWidespreads] = useState<Record<number, boolean>>({})
  const [pageDimensions, setPageDimensions] = useState<Record<number, { width: number; height: number }>>({})
  const [pageSizesKb, setPageSizesKb] = useState<Record<number, number>>({})
  const [markerPlacementMode, setMarkerPlacementMode] = useState(false)
  const [navState, setNavState] = useState<ArchiveNavState>({ ids: [], index: -1 })
  // Resuming a slideshow across an archive boundary (legacy stashes this in `sessionStorage`
  // before navigating away — see `readAdjacentArchive` below) is a pure read of already-set-
  // before-mount state, so it belongs in the initializer, not a `useEffect` calling `setState`.
  const [autoNextActive, setAutoNextActive] = useState(
    () => sessionStorage.getItem('autoNextPage') === 'true',
  )
  const [autoNextCountdown, setAutoNextCountdown] = useState(
    () => Math.trunc(readerSettings.autoNextPageInterval) || 10,
  )
  const containerRef = useRef<HTMLDivElement>(null)
  const leftImgRef = useRef<HTMLImageElement>(null)
  const wakeLockRef = useRef<WakeLockSentinelLike | null>(null)
  const infiniteScrollRootRef = useRef<HTMLDivElement>(null)
  const infiniteScrollCurrentPageRef = useRef<number | null>(null)
  const infiniteScrollResumedRef = useRef(false)
  const infiniteScrollResumePageRef = useRef<number | null>(null)
  // A countdown before following an infinite-scroll boundary click into the adjacent archive —
  // scrolling straight into a full-page navigation with zero warning (what a bare `goTo` does in
  // standard mode) reads very differently once there's no more discrete "page" to land on
  // afterward; giving the reader a beat (and a way to cancel) before it fires matches how
  // `autoNextActive`'s own countdown already works elsewhere on this page.
  const [archiveTransition, setArchiveTransition] = useState<{
    direction: 'prev' | 'next'
    /** Fetched from `/api/reader/recommendations/{id}` while the overlay shows. `null` = still
     * loading / model not ready / fetch failed (the panel then shows nothing to pick). */
    recommendations: {
      archive_id: string
      title: string
      score: number
      isnew: boolean
      is_read: boolean
      is_tank: boolean
    }[] | null
  } | null>(null)
  const imageAreaRef = useRef<HTMLDivElement>(null)
  // The previously-rendered `#i3`'s own real height, captured right before a page turn swaps in
  // a not-yet-loaded image — used as the `.loading` gap's `min-height` instead of the CSS
  // default's flat `75vh`. That flat floor is only a good match for a page that happens to
  // render near 75% of viewport height; a wide/short landscape image in the default "container"
  // fit mode (no `maxHeight` clamp at all — see `imageStyle`'s own docs below) commonly renders
  // *shorter* than that, so falling back to `75vh` during the loading gap made the page briefly
  // grow taller than either the outgoing or the incoming page's own real height, then snap back
  // down once the new image's real dimensions were known — a visible flash pushing the nav
  // controls toward the bottom of the viewport and back on every single page turn.
  const lastSpreadHeightRef = useRef<number | null>(null)
  // The previous spread's own full `fileInfoText` output — see `displayedFileInfo`'s own docs
  // below for why this is held onto rather than always rendering `currentFileInfo` directly.
  const lastFileInfoRef = useRef<string | null>(null)

  // Mirrors legacy's own pick order exactly (reader.js's `loadImages`): an explicit `?p=` param
  // (bookmark link) always wins, then tracked progress *unless* `ignoreProgress` is on, then page
  // one. `readerSettings.ignoreProgress` was previously only ever written to, never read here —
  // toggling it off had no effect on where a reopened archive actually started.
  const currentPage = clamp(
    pageOverride ?? (readerSettings.ignoreProgress ? 1 : Math.max(metadata.data?.progress ?? 1, 1)),
    1,
    totalPages || 1,
  )

  // The fixed target infinite scroll needs to resume to on entry, captured once `currentPage`
  // reflects a real answer and kept stable after that even as `currentPage` itself keeps changing
  // as the user reads. Deliberately *not* `useState(() => currentPage)`: that initializer would
  // run on this component's very first render, which can land before `metadata` has actually
  // loaded — at that point `currentPage` is still its no-server-answer-yet fallback of `1` (see
  // `currentPage`'s own computation above), permanently freezing the resume target at the wrong
  // page. Assigning a ref directly during render, guarded so it only ever takes the *first*
  // non-loading answer, is the documented way to memoize a value that depends on data not
  // necessarily ready on mount (react.dev: "adjusting state directly during rendering").
  if (infiniteScrollResumePageRef.current === null && !metadata.isLoading && !pages.isLoading) {
    infiniteScrollResumePageRef.current = currentPage
  }

  // Real natural dimensions for every page up to *and including* the resume target itself (not
  // just the ones before it) — see the backend's own `read_page_dimensions` docs for why this is
  // deliberately bounded rather than "the whole archive," and `usePageDimensions`'s own docs for
  // why standard (non-infinite-scroll) mode never calls this at all.
  //
  // The target page's *own* dimensions have nothing to do with the resume jump's own accuracy —
  // `scrollIntoView({ block: 'start' })` aligning the target's top edge with the viewport's top
  // only ever depends on the cumulative height of everything *before* it, never its own size. What
  // the target's own reserved size actually prevents is what happens right *after* that jump
  // lands: with only the pages before it reserved (an earlier version of this only asked for
  // those), the target page's own box was still sized off `.loading-placeholder`'s flat guess at
  // the moment of the jump, then swapped to its real (usually taller) height moments later once
  // its own bytes actually finished loading — a reflow right at the current scroll position that
  // can trigger the browser's own scroll-anchoring compensation, a *real* `scroll` event
  // indistinguishable from the user's own. Verified live: resumed at page 10, the viewport-center
  // tracker got nudged to 11 within about a second of landing, which then got written back to the
  // server as progress — a silent forward drift on every single resume. Reserving the target's own
  // size too means its real bytes loading in doesn't change its box at all, so there's nothing left
  // to reflow (belt-and-suspenders alongside the grace-period guard below, which catches whatever
  // this doesn't — neighboring pages with no reserved size of their own still loading in nearby).
  const infiniteScrollResumeDimensions = usePageDimensions(
    archiveId,
    infiniteScrollResumePageRef.current ?? 0,
    readerSettings.infiniteScroll,
  )

  const spread = readerSettings.infiniteScroll
    ? { left: currentPage, right: null }
    : computeSpread(
        currentPage,
        totalPages,
        readerSettings.doublePageMode,
        readerSettings.mangaMode,
        (page) => widespreads[page],
      )

  // Mirrors legacy's `#i3.loading` toggle exactly: added before the new page's image starts
  // loading, removed once decoded. `.loading`'s CSS (`min-height: 75vh`) keeps the page from
  // visually collapsing while blank — keeping the class after load would leave that floor in
  // effect forever (dead whitespace below any shorter image). `pageDimensions` already only gets
  // an entry once `onImageLoad` has fired, so it doubles as the "finished loading" set; both
  // `spread.left` and (if present) `spread.right` must have an entry, matching legacy waiting on
  // both images in double-page mode.
  const currentSpreadLoaded =
    pageDimensions[spread.left] !== undefined &&
    (spread.right === null || pageDimensions[spread.right] !== undefined)

  // Every page *before* the infinite-scroll resume target having a known real height — from
  // `infiniteScrollResumeDimensions` (an `aspect-ratio` computed from it, applied below, well
  // before the actual bytes have loaded) rather than actually downloading and decoding each one —
  // is what the resume effect further down is waiting on before it scrolls. Firing early, while
  // those boxes are still sized off the `.loading-placeholder` CSS's flat `min-height: 40vh`
  // guess, lands nowhere near the real target: verified live that scrolling to a tracked progress
  // of page 8 out of 8 while every page's box was still a 40vh guess landed at page 6's midpoint,
  // since the guessed cumulative height up to page 8 undershot the real one.
  const infiniteScrollResumeReady =
    infiniteScrollResumePageRef.current === null ||
    infiniteScrollResumePageRef.current <= 1 ||
    infiniteScrollResumeDimensions.isSuccess

  // Legacy toggles infinite-scroll mode via `$("body").addClass("infinite-scroll")`
  // (`initInfiniteScrollView`, reader.js:674) — a body-level class, not on `#i1` — since
  // `lrr.css`'s hide rules for `#i2`/`.sn`/etc. are all scoped `body.infinite-scroll #selector`.
  useEffect(() => {
    document.body.classList.toggle('infinite-scroll', readerSettings.infiniteScroll)
    return () => document.body.classList.remove('infinite-scroll')
  }, [readerSettings.infiniteScroll])

  // Progress persistence decision tree (verified against legacy's `updateProgress`):
  // authprogress+logged_in -> server; localprogress -> localStorage; neither -> server anyway.
  // `${archiveId}-reader`'s localStorage key already works unchanged for tank mode — `archiveId`
  // is whatever's in the URL, tank or archive id alike, same as legacy's own `${state.id}-reader`.
  // Only the *server* mutation needs to pick the right endpoint: a Tankoubon's own progress is a
  // single global page number stored directly on its own record (`useUpdateTankoubonProgress` ->
  // `PUT /tankoubons/{id}/progress/{page}`), not resolved back to any one member archive's own
  // progress field.
  useEffect(() => {
    if (!archiveId || totalPages === 0) return
    if (settings.data?.localprogress && !(settings.data?.authprogress && loggedIn)) {
      localStorage.setItem(`${archiveId}-reader`, String(currentPage))
    } else if (isTank) {
      updateTankoubonProgress.mutate(currentPage)
    } else {
      updateProgress.mutate(currentPage)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [archiveId, currentPage, totalPages])

  // `until_finished` badge mode: clear the "new" flag once the reader actually reaches the last
  // page — the same condition the display-side filter (`progress >= pagecount`,
  // `archives::effective_isnew`) uses, so the badge disappears the moment the archive is
  // complete rather than lingering as a stale flag. (Declared after `totalPages`/`currentPage`
  // for that reason.)
  useEffect(() => {
    if (!archiveId || isTank || totalPages === 0 || !newBadgeMode) return
    if (newBadgeMode === 'until_finished' && currentPage >= totalPages) {
      clearArchiveNewRef.current(archiveId)
    }
  }, [archiveId, isTank, totalPages, currentPage, newBadgeMode])

  // Prefetches the next `readerSettings.preloadCount` pages beyond the currently-shown spread. A
  // bare `new Image()` with its `src` set (not appended to the DOM) is enough to make the browser
  // fetch and HTTP-cache the bytes, so a later real `<img>` for that URL serves from cache
  // instantly. Concurrent prefetches across pages are bounded server-side by
  // `AppState::page_singleflight`.
  useEffect(() => {
    if (!pages.data || readerSettings.preloadCount <= 0) return
    const urls: string[] = []
    for (let offset = 1; offset <= readerSettings.preloadCount; offset++) {
      const page = currentPage + offset
      if (page > totalPages) break
      const url = pages.data.pages[page - 1]
      if (url) urls.push(url)
    }
    // Keeping references prevents the browser from cancelling an in-flight prefetch request when
    // the `Image()` object would otherwise be garbage-collected before the request finishes.
    const preloaded = urls.map((url) => {
      const img = new Image()
      img.src = url
      return img
    })
    return () => {
      preloaded.length = 0
    }
  }, [pages.data, currentPage, totalPages, readerSettings.preloadCount])

  // Sets up cross-archive `,`/`.` navigation once per archive open (legacy's
  // `setupArchiveNavigation`, called from `initializeAll`) — resolves whether this reader session
  // arrived from a same-origin index search, and if so prefetches the adjacent results page.
  useEffect(() => {
    if (!archiveId) return
    let cancelled = false
    void setupArchiveNavigation(archiveId).then((nav) => {
      if (!cancelled) setNavState(nav)
    })
    return () => {
      cancelled = true
    }
  }, [archiveId])

  function goTo(target: Parameters<typeof computeNextPage>[0]) {
    const isSpread = spread.right !== null
    const next = computeNextPage(
      target,
      currentPage,
      totalPages || 1,
      readerSettings.mangaMode,
      readerSettings.doublePageMode,
      isSpread,
    )
    // At an archive boundary, step into the adjacent archive instead of clamping in place —
    // mirrors legacy's `changePage` calling `readPreviousArchive`/`readNextArchive` when the
    // destination would fall outside [1, totalPages].
    if (next === currentPage) {
      const goingForward = readerSettings.mangaMode ? target === 'prev' : target === 'next'
      if ((target === 'next' || target === 'prev') && (currentPage === 1 || currentPage === totalPages)) {
        startArchiveTransition(goingForward ? 'next' : 'prev')
        return
      }
    }
    // Captured now, synchronously, before the state update below triggers the re-render that
    // swaps in the new (not-yet-loaded) image — see `lastSpreadHeightRef`'s own docs for why.
    if (imageAreaRef.current) {
      lastSpreadHeightRef.current = imageAreaRef.current.getBoundingClientRect().height
    }
    setPageOverride(next)
    // Legacy's own `goToPage` (reader.js) ends every non-infinite-scroll page change with a
    // plain, instant `window.scrollTo(0, 0)` — verified against the real source, not a smooth
    // scroll. A page tall enough to have been scrolled (a long strip page, a zoomed-in fit mode,
    // a short viewport) would otherwise leave a turn landing wherever the previous page's scroll
    // position happened to be, instead of the new page's own title/nav bar at `#i2` starting
    // visible from the top. Infinite-scroll mode is excluded, matching legacy's own `if
    // (infiniteScroll) { ... } else { ...; window.scrollTo(0, 0); }` branch — there every page
    // shares one continuously-scrolling document, and `selectPage`'s own `scrollIntoView` (not
    // this) is what "jump to page N" means there.
    if (!readerSettings.infiniteScroll) {
      window.scrollTo(0, 0)
    }
  }

  // `goTo` deliberately skips scrolling in infinite-scroll mode (see its own docs) — a click on a
  // page image used to call this directly with its own copy of this exact logic, which meant the
  // keyboard equivalent (arrow keys / A-D, going through `goTo` like everything else) inherited
  // that same no-op-scroll behavior: state updated, nothing visibly moved. Shared here so both
  // input paths agree, instead of the keyboard path silently staying broken while only the click
  // handler got fixed. `fromPage` lets each caller supply its own idea of "where navigation starts
  // from" — the image click passes the specific image that was clicked (not necessarily whatever
  // the scroll-position tracker currently thinks is "current"), the keyboard handler passes
  // `currentPage` (that tracker's own live answer, the only "where am I" it has).
  function goToInfiniteScrollPage(fromPage: number, target: 'prev' | 'next') {
    let offset = target === 'next' ? 1 : -1
    if (readerSettings.mangaMode) offset = -offset
    const nextPage = fromPage + offset
    if (nextPage < 1 || nextPage > totalPages) {
      startArchiveTransition(offset > 0 ? 'next' : 'prev')
      return
    }
    document.querySelector(`[data-page="${nextPage}"]`)?.scrollIntoView({ behavior: 'smooth', block: 'start' })
  }

  // Shows the boundary overlay (recommendations only — no auto-jump; the user picks a card or
  // cancels) and starts fetching the recommendations for the current archive in the background.
  // Lock the page scroll while the boundary overlay is open (standard lightbox behavior) —
  // otherwise wheel events over the overlay chain down to the reader page beneath it. The
  // overlay's own container scrolls internally (max-height + overflow-y).
  useEffect(() => {
    if (!archiveTransition) return
    const prevOverflow = document.body.style.overflow
    document.body.style.overflow = 'hidden'
    return () => {
      document.body.style.overflow = prevOverflow
    }
  }, [archiveTransition])

  const queryClient = useQueryClient()
  // Prefetch the recommendation shortlist while the reader is still a couple pages from the
  // boundary — the LLM rerank takes seconds, and this hides that latency behind normal
  // page-turning so the boundary panel opens with cached data (stale for a minute).
  useEffect(() => {
    if (!archiveId || isTank || totalPages === 0) return
    if (currentPage < totalPages - 2) return
    void queryClient.prefetchQuery({
      queryKey: RECS_QUERY_KEY(archiveId),
      staleTime: 60_000,
      queryFn: () =>
        fetch(`/api/reader/recommendations/${encodeURIComponent(archiveId)}?limit=10`).then(
          (r) => (r.ok ? r.json() : null),
        ),
    })
  }, [archiveId, currentPage, totalPages, queryClient, isTank])

  function startArchiveTransition(direction: 'prev' | 'next') {
    // Cached (prefetched) shortlist opens instantly; otherwise show the skeleton while the
    // fetch runs.
    const cached = archiveId
      ? queryClient.getQueryData<{
          recommendations?: {
            archive_id: string
            title: string
            score: number
            isnew: boolean
            is_read: boolean
            is_tank: boolean
          }[]
        }>(RECS_QUERY_KEY(archiveId))
      : undefined
    setArchiveTransition({
      direction,
      recommendations: cached?.recommendations ?? null,
    })
    if (!archiveId || isTank || cached) return
    void queryClient
      .fetchQuery({
        queryKey: RECS_QUERY_KEY(archiveId),
        staleTime: 60_000,
        queryFn: () =>
          fetch(`/api/reader/recommendations/${encodeURIComponent(archiveId)}?limit=10`).then(
            (r) => (r.ok ? r.json() : null),
          ),
      })
      .then((data) => {
        setArchiveTransition((prev) =>
          prev
            ? {
                ...prev,
                recommendations: data?.recommendations ?? [],
              }
            : prev,
        )
      })
      .catch(() => {
        setArchiveTransition((prev) => (prev ? { ...prev, recommendations: [] } : prev))
      })
  }

  async function readAdjacentArchive(direction: 'prev' | 'next') {
    if (document.fullscreenElement) {
      console.warn('Archive navigation not supported in fullscreen mode.')
      return
    }
    const adjacentId = resolveAdjacentArchive(navState, direction)
    if (!adjacentId) {
      toast({
        text:
          direction === 'prev'
            ? (t('This is the first archive') ?? undefined)
            : (t('This is the last archive') ?? undefined),
      })
      return
    }
    if (autoNextActive) sessionStorage.setItem('autoNextPage', 'true')
    window.location.assign(`/reader/${adjacentId}`)
  }

  function selectPage(page: number) {
    setPageOverride(clamp(page, 1, totalPages || 1))
    setOverlay(null)
    if (readerSettings.infiniteScroll) {
      document.querySelector(`[data-page="${page}"]`)?.scrollIntoView({ block: 'start' })
    } else {
      // Same as `goTo`'s own scroll-to-top — legacy's `goToPage` is the single shared landing
      // point every page-jump path (Next/Prev, the overview thumbnail grid, the page-number
      // input) funnels through, verified against the real source.
      window.scrollTo(0, 0)
    }
  }

  function onImageLoad(page: number, e: React.SyntheticEvent<HTMLImageElement>) {
    const img = e.currentTarget
    const isWide = img.naturalWidth > img.naturalHeight
    setWidespreads((prev) => (prev[page] === isWide ? prev : { ...prev, [page]: isWide }))
    setPageDimensions((prev) => ({
      ...prev,
      [page]: { width: img.naturalWidth, height: img.naturalHeight },
    }))
    if (pageSizesKb[page] === undefined) {
      void fetchContentLengthKb(img.src).then((kb) => {
        if (kb !== null) setPageSizesKb((prev) => ({ ...prev, [page]: kb }))
      })
    }
  }

  function toggleFullScreen() {
    if (!document.fullscreenElement) {
      containerRef.current?.requestFullscreen?.().catch(() => undefined)
    } else {
      document.exitFullscreen?.().catch(() => undefined)
    }
  }

  useEffect(() => {
    function onFullscreenChange() {
      setIsFullscreen(document.fullscreenElement !== null)
    }
    document.addEventListener('fullscreenchange', onFullscreenChange)
    return () => document.removeEventListener('fullscreenchange', onFullscreenChange)
  }, [])

  async function goRandom() {
    const id = await fetchRandomArchiveId()
    if (id) navigate(routes.reader(id))
  }

  // Reuses the exact same confirmation copy `ArchiveOverviewOverlay`'s own tank-delete button
  // uses (an existing key already in every locale file) — offered from the empty-Tankoubon state
  // below since an abandoned, never-populated (or emptied-out) tank has no archives to browse
  // into that overlay from at all; this is the only other place a user could otherwise clean one
  // up from besides TankoubonEdit's own delete button.
  async function handleDeleteEmptyTankoubon() {
    if (
      !archiveId ||
      !(await confirmDialog(
        t(
          'Are you sure you want to delete this tankoubon? The archives will remain in your library but will no longer be grouped.',
        ) ?? '',
      ))
    ) {
      return
    }
    await deleteTankoubon.mutateAsync(archiveId)
    navigate(routes.library())
  }

  function cleanCache() {
    // Matches legacy's own tank-mode `generateThumbnails` (`reader_common.js`): loops every
    // member archive's own thumbnail-regen endpoint, not a single call — there's no one "the
    // archive" to regenerate thumbnails for while reading a concatenated Tankoubon.
    if (isTank) {
      generateThumbnailsForArchives.mutate(tankReading.chapters.map((c) => c.arcId))
    } else {
      generateThumbnails.mutate()
    }
    window.location.reload()
  }

  // Screen Wake Lock — kept alive only while a slideshow is actively running, matching legacy's
  // `requestWakeLock`/`releaseWakeLock` (reader.js:2293) exactly: no point dimming/sleeping the
  // screen mid-slideshow, and no reason to hold the lock any other time.
  async function acquireWakeLock() {
    if (wakeLockRef.current) return
    const nav = navigator as Navigator & { wakeLock?: { request(type: 'screen'): Promise<WakeLockSentinelLike> } }
    if (!nav.wakeLock) return
    try {
      const sentinel = await nav.wakeLock.request('screen')
      sentinel.addEventListener('release', () => {
        wakeLockRef.current = null
      })
      wakeLockRef.current = sentinel
    } catch {
      // Wake lock is a nice-to-have; a denial (e.g. backgrounded tab) shouldn't break the slideshow.
    }
  }

  function releaseWakeLock() {
    wakeLockRef.current?.release().catch(() => undefined)
    wakeLockRef.current = null
  }

  function stopAutoNextPage() {
    setAutoNextActive(false)
    releaseWakeLock()
  }

  function startAutoNextPage() {
    if (readerSettings.autoNextPageInterval <= 0) {
      toast({
        heading: t('Starting auto next page failed!') ?? undefined,
        text: t('Please set the auto next page interval to a positive number.') ?? undefined,
        icon: 'error',
        hideAfter: TOAST_DURATION_MS,
      })
      return
    }
    setAutoNextCountdown(Math.trunc(readerSettings.autoNextPageInterval))
    setAutoNextActive(true)
    void acquireWakeLock()
  }

  function toggleAutoNextPage() {
    if (autoNextActive) stopAutoNextPage()
    else startAutoNextPage()
  }

  // The countdown/advance loop itself — a single interval tied to `autoNextActive`, matching
  // legacy's `startAutoNextPage`'s own `setInterval` (reader.js:1590), just expressed as a React
  // effect instead of manually re-arming a fresh `setInterval` after every tick.
  useEffect(() => {
    if (!autoNextActive) return
    const id = window.setInterval(() => {
      setAutoNextCountdown((prev) => {
        if (prev > 1) return prev - 1
        const atLastPage = readerSettings.mangaMode ? currentPage === 1 : currentPage === totalPages
        if (atLastPage) {
          if (navState.ids.length > 0) {
            void readAdjacentArchive(readerSettings.mangaMode ? 'prev' : 'next')
          }
          setAutoNextActive(false)
          releaseWakeLock()
        } else {
          goTo(readerSettings.mangaMode ? 'prev' : 'next')
        }
        return Math.trunc(readerSettings.autoNextPageInterval)
      })
    }, 1000)
    return () => window.clearInterval(id)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [autoNextActive, currentPage, totalPages, readerSettings.mangaMode, readerSettings.autoNextPageInterval])

  // `autoNextActive`'s initializer above already read the resume flag; this effect only handles
  // the side effects that go with it (clearing the flag so a manual stop+reload doesn't re-arm,
  // and acquiring the wake lock) once pages are actually available to advance through.
  useEffect(() => {
    if (!autoNextActive || totalPages === 0) return
    sessionStorage.removeItem('autoNextPage')
    void acquireWakeLock()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [archiveId, totalPages])

  useEffect(() => releaseWakeLock, [])

  const bookmarkCategoryId = bookmarkLink.data?.category_id || null
  const isBookmarked = Boolean(
    bookmarkCategoryId &&
      categories.data?.find((c) => c.id === bookmarkCategoryId)?.archives.includes(archiveId ?? ''),
  )

  async function toggleBookmark() {
    if (!bookmarkCategoryId) {
      console.error('No bookmark category ID found!')
      return
    }
    if (!loggedIn) {
      const template = t("<a href='\\${url}'>Login</a> to toggle bookmark feature.") ?? ''
      toast({
        text: template.replace('${url}', '/login'),
        icon: 'warning',
        hideAfter: TOAST_DURATION_MS,
      })
      return
    }
    if (!archiveId) return
    const method = isBookmarked ? 'DELETE' : 'PUT'
    await fetch(`/api/categories/${bookmarkCategoryId}/${archiveId}`, { method })
    await categories.refetch()
  }

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if ((e.target as HTMLElement)?.tagName === 'INPUT') return

      if (e.key === ',') {
        void readAdjacentArchive('prev')
        return
      }
      if (e.key === '.') {
        void readAdjacentArchive('next')
        return
      }

      switch (e.key) {
        case 'Backspace':
          navigate(routes.library())
          return
        case 'Escape':
          // Legacy's own keydown handler (`reader_stamps.js`) checks `state.markerMode` before
          // anything else and, if armed, only ever cancels *that* — it doesn't also happen to
          // close some other overlay in the same keystroke. Matches that priority: an Escape while
          // placing a stamp cancels the placement and stops there, same as pressing it with no
          // overlay open at all does nothing further.
          if (markerPlacementMode) {
            setMarkerPlacementMode(false)
            return
          }
          setOverlay(null)
          return
        case ' ':
          window.scrollBy({ top: window.innerHeight * 0.8, behavior: 'smooth' })
          return
        case 'ArrowLeft':
        case 'a':
          if (readerSettings.infiniteScroll) {
            if (e.shiftKey) selectPage(1)
            else goToInfiniteScrollPage(currentPage, 'prev')
          } else {
            goTo(e.shiftKey ? 'first' : 'prev')
          }
          return
        case 'ArrowRight':
        case 'd':
          if (readerSettings.infiniteScroll) {
            if (e.shiftKey) selectPage(totalPages)
            else goToInfiniteScrollPage(currentPage, 'next')
          } else {
            goTo(e.shiftKey ? 'last' : 'next')
          }
          return
        case 'b':
          void toggleBookmark()
          return
        case 'f':
          toggleFullScreen()
          return
        case 'g': {
          void (async () => {
            const value = await promptDialog(t('Go to page:') ?? '')
            const page = value ? parseInt(value, 10) : NaN
            if (!Number.isNaN(page)) selectPage(page)
          })()
          return
        }
        case 'h':
          setOverlay((prev) => (prev === 'help' ? null : 'help'))
          return
        case 'm':
          updateReaderSettings({ mangaMode: !readerSettings.mangaMode })
          return
        case 'n':
          toggleAutoNextPage()
          return
        case 'o':
          setOverlay((prev) => (prev === 'settings' ? null : 'settings'))
          return
        case 'p':
          updateReaderSettings({ doublePageMode: !readerSettings.doublePageMode })
          return
        case 'q':
          setOverlay((prev) => (prev === 'archive' ? null : 'archive'))
          return
        case 'r':
          void goRandom()
          return
        case 's':
          // Matches legacy's own `addStamp()` guard (`if (!LRR.isUserLogged()) return;`) — stamps
          // are a per-user API resource, so arming placement mode while logged out would only ever
          // end in the `addStamp` mutation itself failing after the user already went to the
          // trouble of picking a spot and typing a name.
          if (!readerSettings.infiniteScroll && loggedIn) setMarkerPlacementMode(true)
          return
        default:
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    currentPage,
    totalPages,
    readerSettings.mangaMode,
    readerSettings.doublePageMode,
    readerSettings.infiniteScroll,
    navState,
    autoNextActive,
    isBookmarked,
    bookmarkCategoryId,
    loggedIn,
    markerPlacementMode,
  ])

  // Infinite scroll: tracks which mounted page is nearest the viewport center and treats that as
  // "current" for progress purposes — legacy's own hand-rolled hit-test in `changePage`
  // (`reader_common.js`: walk every `.reader-image`, find whichever one's bounding rect straddles
  // `window.innerHeight / 2`), not an `IntersectionObserver`. An earlier version of this effect
  // used `IntersectionObserver` with `threshold: 0.5` instead, driven by whichever entries the
  // browser happened to report as newly-intersecting in a given callback batch — verified live via
  // the network panel that this produced non-monotonic progress writes (`3 -> 8 -> 6` while
  // scrolling steadily downward), almost certainly several `lazy`-loaded images near the initial
  // viewport all crossing 50% together as they decode in, not in scroll order. A direct viewport-
  // center hit test on `scroll` (rAF-throttled) has no such batching ambiguity: at any instant
  // there's exactly one page whose rect can straddle the midpoint.
  useEffect(() => {
    if (!readerSettings.infiniteScroll || totalPages === 0) {
      // Reset so the next time infinite scroll is (re-)entered — a fresh page load, or just
      // toggling the setting back on mid-session — resumes at whatever `currentPage` is *then*,
      // not silently skipping the resume-scroll because it already ran once, arbitrarily long ago,
      // the first time this component ever saw the mode turned on.
      infiniteScrollResumedRef.current = false
      infiniteScrollResumePageRef.current = null
      return
    }
    const root = infiniteScrollRootRef.current
    if (!root) return

    // A fresh load always starts scrolled to the very top (page one's own image) — the browser
    // has no notion of "resume where tracked progress left off" here on its own, unlike standard
    // mode where `currentPage` alone decides which single image renders. Legacy's own
    // `enterInfiniteScrollView` has the same problem and solves it the same way: one explicit,
    // instant `scrollIntoView` to the resume page, done once — but only once every page before it
    // has a real `aspect-ratio` reserved from `infiniteScrollResumeDimensions`
    // (`infiniteScrollResumeReady`), not still sized off `.loading-placeholder`'s flat `40vh`
    // guess; firing early lands wherever that guess's cumulative error happens to put it, not the
    // real target (verified live: jumping to a tracked progress of page 8/8 landed at page 6's
    // midpoint instead, back when this waited on real decoded image loads instead). The scroll-
    // position tracker below intentionally doesn't attach until *after* this fires, for the same
    // reason it needs to fire accurately in the first place: its own first read would otherwise
    // see the still-at-the-top scroll position and immediately stomp `currentPage` back down to 1
    // before the resume jump ever got a chance to happen.
    const alreadyResumed = infiniteScrollResumedRef.current
    if (!alreadyResumed) {
      if (!infiniteScrollResumeReady) return
      infiniteScrollResumedRef.current = true
      infiniteScrollCurrentPageRef.current = currentPage
      root.querySelector<HTMLElement>(`[data-page="${currentPage}"]`)?.scrollIntoView({ block: 'start' })
    }
    // A `scroll` event fires identically whether a person actually dragged/wheeled/keyed the
    // page, or the browser's own scroll-anchoring silently compensated for some page's box
    // changing size (any lazy-loaded image swapping its `.loading-placeholder` guess for its real
    // height, at any point during the whole reading session, not just around the initial resume).
    // A first attempt compared `document.documentElement.scrollHeight` against its own last-known
    // value on every `scroll` event to catch this indirectly — abandoned after verifying live it
    // has a real timing gap: the anchor compensation's own `scrollY` adjustment doesn't necessarily
    // land in the same animation frame as the height change that caused it, so a check can land in
    // between — sampling a `scrollHeight` that already looks stable (nothing grew *again* since
    // the last poll) with a `scrollY` that's *only just* been nudged by a compensation whose
    // triggering height change this polling loop had already "used up" reacting to on an earlier,
    // separate poll. A `ResizeObserver` directly on every page's own box sidesteps that gap
    // entirely: the browser guarantees its callback fires whenever an observed box's size actually
    // changes, so arming a short guard window from *that* (not from polling a derived height) can't
    // miss a resize the way sampling scrollHeight on our own schedule can.
    const REFLOW_GUARD_MS = 400
    let reflowGuardUntil = performance.now() + REFLOW_GUARD_MS

    let rafId: number | null = null
    function updateCurrentPageFromScroll() {
      rafId = null
      if (performance.now() < reflowGuardUntil) return
      if (!root) return
      const viewportMid = window.innerHeight / 2
      const images = root.querySelectorAll<HTMLElement>('[data-page]')
      for (const img of images) {
        const rect = img.getBoundingClientRect()
        if (rect.top > viewportMid || rect.bottom < viewportMid) continue
        const page = Number(img.dataset.page)
        if (!Number.isNaN(page) && infiniteScrollCurrentPageRef.current !== page) {
          infiniteScrollCurrentPageRef.current = page
          setPageOverride(page)
        }
        break
      }
    }
    function onScroll() {
      if (rafId !== null) return
      rafId = requestAnimationFrame(updateCurrentPageFromScroll)
    }
    window.addEventListener('scroll', onScroll, { passive: true })

    const resizeObserver = new ResizeObserver(() => {
      reflowGuardUntil = performance.now() + REFLOW_GUARD_MS
    })
    root.querySelectorAll<HTMLElement>('[data-page]').forEach((img) => resizeObserver.observe(img))

    return () => {
      window.removeEventListener('scroll', onScroll)
      if (rafId !== null) cancelAnimationFrame(rafId)
      resizeObserver.disconnect()
    }
    // `currentPage` deliberately excluded: only read once, guarded by `infiniteScrollResumedRef`,
    // for the initial-resume scroll above — adding it here would re-run this whole effect (tear
    // down and re-attach the scroll listener) on every page the user scrolls past, since this
    // effect's own tracking is what drives `currentPage` changes in the first place.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [readerSettings.infiniteScroll, totalPages, pages.data, infiniteScrollResumeReady])

  if (metadata.isLoading || pages.isLoading) {
    return (
      <div className="loading">
        <div className="loading-overlay">
          <p className="loading-spinner">
            <i className="fas fa-fan fa-spin"></i>
          </p>
        </div>
      </div>
    )
  }

  if (metadata.isError || pages.isError || !pages.data || !metadata.data) {
    return (
      <div className="ido">
        <p>
          {t('Failed to load archives: {{error}}', {
            error: String(metadata.error ?? pages.error),
          })}
        </p>
        <input
          type="button"
          className="stdbtn"
          value={t('Return to Library') ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>
    )
  }

  // A Tankoubon with zero member archives has nothing to concatenate into a book — `totalPages`
  // is genuinely `0`, not a loading/error state. Rendering the normal page-view JSX below for that
  // case used to fall through to a broken "1 / 0" reader with no image (a real, observed bug — see
  // `TANK_...`'s own incident this session where a Tankoubon's archive list emptied out and its
  // reader page silently showed this), rather than a real explanation. `isTank` gates this (a
  // plain archive's own `totalPages` is only ever `0` mid-load, already handled by the loading
  // check above) — an intentionally-empty Tankoubon is a real, reachable state (freshly created
  // and not yet populated, or emptied via archive removal), not an error.
  if (isTank && totalPages === 0) {
    return (
      <div className="ido" style={{ textAlign: 'center', padding: 40 }}>
        <i className="fas fa-8x fa-box-open" aria-hidden="true"></i>
        <h2 style={{ marginTop: 16 }}>{t('This Tankoubon has no archives yet.')}</h2>
        <div style={{ marginTop: 16, display: 'flex', gap: 8, justifyContent: 'center' }}>
          <input
            type="button"
            className="stdbtn"
            value={t('Edit Tankoubon') ?? undefined}
            onClick={() => archiveId && navigate(routes.tankoubonEdit(archiveId))}
          />
          <input
            type="button"
            className="stdbtn"
            value={t('Delete Tankoubon') ?? undefined}
            onClick={() => void handleDeleteEmptyTankoubon()}
          />
          <input
            type="button"
            className="stdbtn"
            value={t('Return to Library') ?? undefined}
            onClick={() => navigate(routes.library())}
          />
        </div>
      </div>
    )
  }

  const leftUrl = pages.data.pages[spread.left - 1]
  const rightUrl = spread.right !== null ? pages.data.pages[spread.right - 1] : null

  // `MarkerLayer` (stamps) is a per-real-archive resource — in tank mode, the current global page
  // has to be resolved back to which member archive (and that archive's own local page number)
  // it actually belongs to first, matching legacy's own `getArchiveForPage` used the same way in
  // `reader_stamps.js`. `null` (nothing rendered) if that resolution fails — a member archive that
  // vanished from the repo after the Tankoubon's own `full_data` was fetched (see
  // `TankoubonFullResponse`'s own docs), a real if rare edge case.
  const markerTarget = isTank
    ? tankReading.getArchiveForPage(spread.left)
    : archiveId
      ? { arcId: archiveId, localPage: spread.left }
      : null

  // Mirrors legacy's `applyContainerWidth` (reader.js:1502) exactly: fit mode drives two
  // *different* styles on two *different* elements — `.reader-image` (each `<img>`) and `.sni`
  // (the outermost `#i1` container, not some intermediate wrapper) — not a single style applied
  // to one shared box, which is why "Width" and "Container" modes previously looked identical.
  const isSpreadShowing = spread.right !== null
  const imageStyle: React.CSSProperties = {}
  const outerStyle: React.CSSProperties = {}
  // Legacy applies none of this while in fullscreen (`applyContainerWidth`'s own
  // `if (fscreen.inFullscreen()) return`) — the browser's native fullscreen presentation should
  // decide sizing there, not these fit-mode rules.
  if (!isFullscreen) {
    if (readerSettings.fitMode === 'fit-height') {
      const heightVh = readerSettings.hideHeader || readerSettings.infiniteScroll ? 98 : 90
      imageStyle.maxHeight = `${heightVh}vh`
      outerStyle.width = 'fit-content'
    } else if (readerSettings.fitMode === 'fit-width') {
      imageStyle.width = '100%'
      outerStyle.maxWidth = '98%'
    } else if (readerSettings.containerWidth) {
      outerStyle.maxWidth = readerSettings.containerWidth
      imageStyle.width = '100%'
    } else if (isSpreadShowing) {
      outerStyle.maxWidth = '90%'
    } else {
      outerStyle.maxWidth = '1200px'
    }
  }

  // Legacy's real `addStamp()` (`~/LANraragi/public/js/mod/reader_stamps.js`) bumps only the
  // *left* image's own `z-index` above `.focus-overlay`'s while placing a stamp — not the
  // double-page-mode right image (`#img_doublepage` keeps `imageStyle` as-is, unmodified), matching
  // `MarkerLayer`'s own `imageRef` only ever pointing at this one. `zIndex: 22` beats the
  // overlay's own 21 (`.focus-overlay` in `/legacy/lrr.css`) so the image stays fully visible and
  // clickable above the dimmed backdrop instead of getting dimmed along with everything else.
  const placementImageStyle: React.CSSProperties = markerPlacementMode
    ? { ...imageStyle, zIndex: 22, cursor: 'cell' }
    : imageStyle

  const bookmarkLinkConfigured = Boolean(bookmarkCategoryId)

  // Shared between the `?` icon's hover-tooltip preview and the same icon's click/`H`-key full
  // panel (`overlay === 'help'` below) — one piece of content, two presentations, rather than
  // duplicating (and inevitably drifting) the same shortcut list twice.
  const helpContent = (
    <div style={{ fontSize: FONT_SIZE_8PT }}>
      <p style={{ margin: '0 0 4px' }}>{t('You can navigate between pages using:')}</p>
      <ul style={{ margin: '0 0 8px', paddingLeft: 18 }}>
        <li>{t('The arrow icons')}</li>
        <li>
          {t('The')} <Key>A</Key>/<Key>D</Key> {t('keys')}
        </li>
        <li>{t('Your keyboard arrows (and the spacebar)')}</li>
        <li>{t('Touching the left/right side of the image.')}</li>
      </ul>
      <p style={{ margin: '0 0 4px' }}>
        {t('When reading an archive from search results, you can also navigate between archives using:')}
      </p>
      <ul style={{ margin: '0 0 8px', paddingLeft: 18 }}>
        <li>
          <Key>,</Key> {t('and')} <Key>.</Key> {t('keys')}
        </li>
        <li>{t('Reading past the first/last page')}</li>
      </ul>
      <p style={{ margin: '0 0 4px' }}>{t('Other keyboard shortcuts:')}</p>
      <ul style={{ margin: '0 0 8px', paddingLeft: 18 }}>
        <li>{t('M: toggle manga mode (right-to-left reading)')}</li>
        <li>{t('O: show advanced reader options.')}</li>
        <li>{t('P: toggle double page mode')}</li>
        <li>{t('Q: bring up the thumbnail index and archive options.')}</li>
        <li>{t('R: open a random archive.')}</li>
        <li>{t('F: toggle fullscreen mode')}</li>
        <li>{t('B: toggle bookmark')}</li>
        <li>{t('N: toggle auto next page')}</li>
        <li>{t('shift+Left/Right: go to first page/last page')}</li>
        <li>{t('G: go to page number')}</li>
        <li>{t('S: set a Stamp')}</li>
      </ul>
      <p style={{ margin: 0 }}>{t('To return to the archive index, touch the arrow pointing down or use Backspace.')}</p>
    </div>
  )

  const pagesel = (
    <>
      {/* Each `<a>` gets an explicit `marginRight` matching what legacy gets "for free": its own
          template has these as separate lines of hand-indented HTML
          (`~/LANraragi/templates/reader.html.tt2`), and adjacent `inline-block` elements collapse
          the whitespace *between* them into a visible gap (~3px at this font-size) — something
          JSX's compiled output never produces, since React never inserts whitespace text nodes
          between sibling elements. Without this, the icons render flush against each other. */}
      <div className="absolute-options absolute-left">
        <a
          className="fas fa-cog fa-2x"
          href="#"
          title={t('Reader Options') ?? undefined}
          style={{ marginRight: 3 }}
          onClick={(e) => {
            e.preventDefault()
            setOverlay((prev) => (prev === 'settings' ? null : 'settings'))
          }}
        />
        {/* Hover for a quick preview (`Tooltip`'s own `anchor="element"` default); click, or `H`,
            still opens the full `#reader-help` panel below — hovering to check one shortcut
            shouldn't require a full modal open/close round trip, but the complete list (including
            the ones easy to forget) stays reachable the same way it always was. */}
        <Tooltip label={helpContent} maxWidth={420}>
          <a
            className="fas fa-question-circle fa-2x"
            href="#"
            title={t('Help') ?? undefined}
            style={{ marginRight: 3 }}
            onClick={(e) => {
              e.preventDefault()
              setOverlay((prev) => (prev === 'help' ? null : 'help'))
            }}
          />
        </Tooltip>
        {bookmarkLinkConfigured && (
          <a
            className={`${isBookmarked ? 'fas' : 'far'} fa-bookmark fa-2x toggle-bookmark${loggedIn ? '' : ' disabled'}`}
            href="#"
            title={t('Toggle Bookmark') ?? undefined}
            style={loggedIn ? { marginRight: 3 } : { marginRight: 3, opacity: 0.5, cursor: 'not-allowed' }}
            onClick={(e) => {
              e.preventDefault()
              void toggleBookmark()
            }}
          />
        )}
      </div>
      <div className="absolute-options absolute-right">
        <a
          className={`fas ${readerSettings.mangaMode ? 'fa-arrow-left' : 'fa-arrow-right'} fa-2x reading-direction`}
          href="#"
          title={t('Reading Direction') ?? undefined}
          style={{ marginRight: 3 }}
          onClick={(e) => {
            e.preventDefault()
            updateReaderSettings({ mangaMode: !readerSettings.mangaMode })
          }}
        />
        <a
          className="fas fa-stopwatch fa-2x toggle-auto-next-page"
          href="#"
          title={t('Auto Next Page') ?? undefined}
          style={{ marginRight: 3 }}
          onClick={(e) => {
            e.preventDefault()
            toggleAutoNextPage()
          }}
        >
          {autoNextActive ? autoNextCountdown : ''}
        </a>
        <a
          className="fas fa-th fa-2x"
          href="#"
          title={t('Archive Overview') ?? undefined}
          style={{ marginRight: 3 }}
          onClick={(e) => {
            e.preventDefault()
            openedByDefaultSetting.current = false
            setOverlay((prev) => (prev === 'archive' ? null : 'archive'))
          }}
        />
        <a
          className={`fas ${isFullscreen ? 'fa-compress' : 'fa-expand'} fa-2x`}
          href="#"
          title={t('FullScreen') ?? undefined}
          style={{ marginRight: 3 }}
          onClick={(e) => {
            e.preventDefault()
            toggleFullScreen()
          }}
        />
      </div>
    </>
  )

  // `body.infinite-scroll .sn { display: none }` (legacy's `lrr.css`) can't win against this
  // element's own inline `style.display` below (inline style always beats an external stylesheet
  // selector short of `!important`) — verified live: the row stayed visible with the class
  // correctly applied to `<body>`. Not rendering it at all in that mode, rather than trying to
  // out-fight the inline style with a conditional `display` value, since infinite-scroll mode
  // doesn't have a notion of "current page" a click on these icons could jump to anyway (see
  // `goTo`'s own no-scroll early-exit for that mode).
  const arrows = readerSettings.infiniteScroll ? null : (
    // `marginTop`/`marginBottom` override each theme's own real `div.sn { margin: 1px auto }`
    // (verified live via `getComputedStyle` — e.g. `g.css`), which leaves only ~1px between this
    // row and the archive title/file-info text immediately above or below it (this same `arrows`
    // element renders twice — once above the image in `#i2`, once below it in `#i4` — so both
    // spots need the fix).
    // `display: flex` + `alignItems: 'center'` — the icons and `.pagecount` text used to rely on
    // each inline element's own `vertical-align: baseline` (the CSS default) to line up, which
    // looked fine at the old, uniformly small font-size but visibly mismatched once the icons and
    // page-number text were enlarged to two different sizes (each size's own baseline sits at a
    // different height). Flex's cross-axis centering doesn't care about font-size/baseline at
    // all — it's already `display: block` at 100% of its parent's width (verified live:
    // `getBoundingClientRect` both 1200px), so switching to flex doesn't disturb the existing
    // horizontal centering; `justifyContent: 'center'` replaces the `margin: auto` a flex
    // container doesn't honor the same way.
    <div
      className="sn paginator"
      style={{
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        gap: 12,
        marginTop: 10,
        marginBottom: 10,
      }}
    >
      {/* `gap` on the flex container above is the single source of spacing between every icon
          and `.pagecount` — legacy's own `div.sn div { margin: 2px 25px 0 }` theme rule only ever
          matched `.pagecount` (a real `div`), never these `<a>` tags, so the two pairs of adjacent
          arrows (first/prev, next/last) rendered flush against each other with zero gap
          (verified live via `getBoundingClientRect` — 0px) — `gap` fixes all of them uniformly
          instead of hand-tuning per-element margins that would fight `.pagecount`'s own margin
          reset below. `title` on each: legacy itself never set one here (checked
          `reader.html.tt2` — bare `<a class="page-link">`, no title attribute at all), a real gap
          beyond a port, not a missed one. */}
      <a
        className="fas fa-backward-step page-link archive-nav-link"
        title={t('Previous Archive') ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE, display: navState.ids.length > 0 ? undefined : 'none' }}
        onClick={() => void readAdjacentArchive('prev')}
      />
      <a
        className="fas fa-angle-double-left page-link"
        title={t('First Page') ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('first')}
      />
      <a
        className="fas fa-angle-left page-link"
        title={t('Previous Page') ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('prev')}
      />
      {/* `lineHeight: 1` — matches the icons' own real rendered line-height (`font-size`'s own
          value, since Font Awesome glyphs are `line-height: 1` by convention); without it,
          `line-height: normal`'s default leading throws off vertical centering at this larger
          size. `margin: 0, padding: 0` overrides legacy theme CSS's real `.pagecount` rule
          (`margin: 2px 25px 0; padding: 0 0 8px` — sized for the old, much smaller font, verified
          live via `getComputedStyle`), whose bottom-heavy padding pushed the box itself visibly
          off-center (3px gap above vs. 1px below, measured live) — flex's own `alignItems:
          'center'`/`justifyContent: 'center'` already place this correctly, so the leftover
          legacy spacing only fights it now. */}
      <div className="pagecount" style={{ fontSize: PAGINATOR_PAGECOUNT_FONT_SIZE, lineHeight: 1, margin: 0, padding: 0 }}>
        <span className="current-page">{currentPage}</span> / <span className="max-page">{totalPages}</span>
      </div>
      <a
        className="fas fa-angle-right page-link"
        title={t('Next Page') ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('next')}
      />
      <a
        className="fas fa-angle-double-right page-link"
        title={t('Last Page') ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('last')}
      />
      <a
        className="fas fa-forward-step page-link archive-nav-link"
        title={t('Next Archive') ?? undefined}
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE, display: navState.ids.length > 0 ? undefined : 'none' }}
        onClick={() => void readAdjacentArchive('next')}
      />
    </div>
  )

  // `fileInfoText` returns just the bare filename (dimensions/size omitted) whenever the new
  // spread's `pageDimensions`/`pageSizesKb` aren't known yet — true for every freshly-turned-to
  // page until its `onLoad` (and the follow-up `HEAD` request for byte size) resolve. Rendering
  // that shorter string immediately made this line visibly shrink then grow back on every single
  // page turn; holding the *previous* spread's full text during that gap (via a ref, so this
  // doesn't itself trigger an extra render) keeps the line's content — and thus its wrapped
  // height — stable until the new page's real info is ready to replace it outright, matching how
  // `lastSpreadHeightRef` above smooths the image area's own height across the same gap.
  const currentFileInfo = pages.data
    ? fileInfoText(pages.data.pages, spread, pageDimensions, pageSizesKb, window.location.origin)
    : ''
  const isFileInfoReady = spread.right === null
    ? pageDimensions[spread.left] !== undefined && pageSizesKb[spread.left] !== undefined
    : pageDimensions[spread.left] !== undefined &&
      pageDimensions[spread.right] !== undefined &&
      pageSizesKb[spread.left] !== undefined &&
      pageSizesKb[spread.right] !== undefined
  if (isFileInfoReady) lastFileInfoRef.current = currentFileInfo
  const displayedFileInfo = isFileInfoReady ? currentFileInfo : (lastFileInfoRef.current ?? currentFileInfo)
  const fileinfo = (
    <div className="file-info" title={displayedFileInfo}>
      {displayedFileInfo}
    </div>
  )

  // Matches legacy's own real `reader.js` exactly: `content.tags.match(/artist:([^,]+)(?:,|$)/i)`
  // — case-insensitive, the value runs up to the next comma or end of string. When present, the
  // heading reads "{title} by {artist}" with the artist name as a real link to that artist's own
  // search results (`getTagSearchURL`, the same helper `TagTable.tsx`'s tag chips already use);
  // otherwise just the bare title, same as before. The literal `" by "` is not run through `t()`
  // — verified against legacy's own real source (`reader.js` and every `locales/template/*.po`):
  // this exact string was never internationalized there either, always a plain hardcoded English
  // JS string concatenation regardless of the active UI language.
  const artistMatch = metadata.data.tags.match(/artist:([^,]+)(?:,|$)/i)
  const archiveHeading = artistMatch ? (
    <>
      {metadata.data.title} by <a href={getTagSearchURL('artist', artistMatch[1])}>{artistMatch[1]}</a>
    </>
  ) : (
    metadata.data.title
  )

  return (
    <>
    <div id="i1" className="sni" ref={containerRef} style={outerStyle}>
      {!readerSettings.hideHeader && (
        <div id="i2">
          <h1 id="archive-title">{archiveHeading}</h1>
          {pagesel}
          {arrows}
          {fileinfo}
        </div>
      )}

      <div
        id="i3"
        ref={imageAreaRef}
        className={!readerSettings.infiniteScroll && !currentSpreadLoaded ? 'loading' : undefined}
        // Overrides `.loading`'s CSS `min-height: 75vh` with the previous spread's own real
        // height (captured in `goTo`, right before this page turn) whenever one's actually
        // known — a floor that already matches what's about to render is far less likely to
        // over- or under-shoot the incoming page's real height than a flat, content-blind 75vh
        // guess. Falls through to the CSS default (by simply not setting `minHeight` at all,
        // rather than overriding it with something equally arbitrary) for the very first page
        // load, when there's no previous spread to measure yet.
        style={
          !currentSpreadLoaded && lastSpreadHeightRef.current !== null
            ? { minHeight: lastSpreadHeightRef.current }
            : undefined
        }
      >
        {readerSettings.infiniteScroll ? (
          <div id="display" ref={infiniteScrollRootRef}>
            {pages.data.pages.map((url, i) => {
              const hasRealHeight = pageDimensions[i + 1] !== undefined
              // Every page before the resume target gets its real `aspect-ratio` from
              // `infiniteScrollResumeDimensions` (a lightweight, dimensions-only backend read —
              // see its own docs) *before* its actual bytes have loaded, so the browser can
              // compute this box's real rendered height right away from `imageStyle`'s own
              // existing width/height constraint (e.g. `width: '100%'`) the exact same way it
              // would once the image data itself arrives — no need to force this fetch any
              // earlier than native `loading="lazy"` already would on its own. Pages beyond that
              // range fall back to `.loading-placeholder`'s flat guess, same as before; they were
              // never part of what the resume jump needs to land accurately.
              const resumeDim = infiniteScrollResumeDimensions.data?.dimensions[i]
              const style: React.CSSProperties =
                !hasRealHeight && resumeDim
                  ? { ...imageStyle, aspectRatio: `${resumeDim.width} / ${resumeDim.height}` }
                  : imageStyle
              return (
                <img
                  key={url}
                  data-page={i + 1}
                  className={hasRealHeight ? 'reader-image' : 'reader-image loading-placeholder'}
                  src={url}
                  alt={`${t('Page')} ${i + 1}`}
                  loading="lazy"
                  draggable={false}
                  style={style}
                  onLoad={(e) => onImageLoad(i + 1, e)}
                  onClick={(e) => {
                    const isLeftHalf = e.clientX < window.innerWidth / 2
                    goToInfiniteScrollPage(i + 1, isLeftHalf ? 'prev' : 'next')
                  }}
                />
              )
            })}
          </div>
        ) : (
          <div id="display">
            <a
              id="imgLink"
              href={leftUrl}
              onClick={(e) => {
                const x = e.clientX
                const isLeftHalf = x < window.innerWidth / 2
                e.preventDefault()
                goTo(isLeftHalf ? 'prev' : 'next')
              }}
              style={{ position: 'relative', display: 'inline-flex' }}
            >
              <img
                id="img"
                ref={leftImgRef}
                className="reader-image"
                src={leftUrl}
                alt={`${t('Page')} ${spread.left}`}
                fetchPriority="high"
                onLoad={(e) => onImageLoad(spread.left, e)}
                draggable={false}
                style={placementImageStyle}
              />
              {rightUrl && (
                <img
                  id="img_doublepage"
                  className="reader-image"
                  src={rightUrl}
                  alt={`${t('Page')} ${spread.right}`}
                  fetchPriority="high"
                  onLoad={(e) => onImageLoad(spread.right ?? 0, e)}
                  draggable={false}
                  style={imageStyle}
                />
              )}
            </a>
            {markerTarget && (
              <MarkerLayer
                archiveId={markerTarget.arcId}
                page={markerTarget.localPage}
                imageRef={leftImgRef}
                visible={readerSettings.markersVisible}
                placementMode={markerPlacementMode}
                onPlaced={() => setMarkerPlacementMode(false)}
              />
            )}
          </div>
        )}
      </div>

      <div id="i4">
        {fileinfo}
        {pagesel}
        {arrows}
      </div>

      <div id="i5">
        <div className="sb">
          <a
            id="return-to-index"
            style={{ cursor: 'pointer' }}
            title={t('Done reading? Go back to Archive Index') ?? undefined}
            onClick={() => navigate(routes.library())}
          >
            <i className="fas fa-angle-down fa-3x"></i>
          </a>
        </div>
      </div>

      <div id="i7" className="if">
        <i className="fas fa-caret-right fa-lg"></i>
        <a href={leftUrl} target="_blank" rel="noreferrer">
          {t('View full-size image')}
        </a>
        <i className="fas fa-caret-right fa-lg"></i>
        <a style={{ cursor: 'pointer' }} onClick={() => void goRandom()}>
          {t('Switch to another random archive')}
        </a>
        {loggedIn && (
          <>
            <i className="fas fa-caret-right fa-lg"></i>
            <a style={{ cursor: 'pointer' }} onClick={cleanCache}>
              {t('Clean Archive Cache')}
            </a>
          </>
        )}
      </div>

      {overlay === 'archive' && (
        <ArchiveOverviewOverlay
          archive={metadata.data}
          categories={categories.data}
          loggedIn={loggedIn}
          currentPage={currentPage}
          onClose={() => setOverlay(null)}
          onSelectPage={selectPage}
          autoFocus={!openedByDefaultSetting.current}
          resolvePage={isTank ? tankReading.getArchiveForPage : undefined}
          tankChapters={isTank ? tankReading.chapters : undefined}
          tankPages={isTank ? pages.data.pages : undefined}
        />
      )}

      {overlay === 'settings' && (
        <SettingsOverlay
          settings={readerSettings}
          update={updateReaderSettings}
          onClose={() => setOverlay(null)}
        />
      )}

      {overlay === 'help' && (
        <>
          {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
              opaque black, so content behind the shade stays faintly visible. */}
          <div id="overlay-shade" style={{ display: 'block', opacity: 0.6 }} onClick={() => setOverlay(null)} />
          <div id="reader-help" className="id1 base-overlay small-overlay">
            <div className="navigation-help-toast">{helpContent}</div>
          </div>
        </>
      )}

      {/* Boundary overlay: recommendations only — no auto-jump (the user picks a card or
          cancels). Legacy had nothing here (it immediately called
          `readNextArchive`/`readPreviousArchive`, which toasted "last archive" without search
          context); the panel replaces that toast with actual next-read suggestions. The
          container is deliberately an *invisible* modal — no panel box, just the shade plus
          the content floating above it (the boxy `base-overlay` panel felt obstructive).
          Clicking the shade cancels, same as every other overlay on this page. */}
      {archiveTransition && (
        <>
          {/* Lightbox-style shade: noticeably darker than the reader's own 0.6 overlays so the
              recommendation cards pop, matching the classic lightbox look. */}
          <div
            id="overlay-shade"
            style={{ display: 'block', opacity: 0.85, overscrollBehavior: 'contain' }}
            onClick={() => setArchiveTransition(null)}
          />
          /* Full-width fixed container (left:0/right:0 + translateY only) — a `left: 50% +
             translate(-50%)` shrink-to-fit container's width is content/available-derived and
             came out narrower than the 760px flex row, wrapping the 5-per-row cards into more
             rows; the full-width container lets the inner flex row's own maxWidth rule
             correctly center a 5-across two-row grid on any desktop viewport. */
          <div
            className="rec-overlay"
            onClick={() => setArchiveTransition(null)}
            style={{ position: 'fixed', top: '50%', left: 0, right: 0, transform: 'translateY(-50%)', textAlign: 'center', zIndex: 9001, background: 'transparent', maxHeight: '95vh', overflowY: 'auto', overscrollBehavior: 'contain', paddingBottom: 16 }}
          >

            {metadata.data?.title && (
              <p style={{ fontSize: 13, color: 'rgba(255,255,255,0.8)', marginBottom: 4 }}>
                {t('Currently reading')}: {metadata.data.title}
              </p>
            )}
            <p style={{ fontSize: 16, fontWeight: 'bold', color: '#fff' }}>
              {t('You might also like')}
            </p>
            {archiveTransition.recommendations === null && (
              /* Skeleton while the (un-prefetched) LLM rerank is in flight — grey card shapes
                 matching the real cards' dimensions. */
              <div className="rec-row" aria-busy="true">
                {Array.from({ length: 10 }).map((_, i) => (
                  <div key={i} className="rec-card rec-skeleton" style={{ background: 'rgba(255,255,255,0.1)', border: 'none' }}>
                    <div
                      style={{
                        width: '100%',
                        aspectRatio: '3 / 4',
                        borderRadius: 4,
                        background: 'rgba(255,255,255,0.08)',
                      }}
                    />
                    <div style={{ height: 11, marginTop: 6, width: '80%', background: 'rgba(255,255,255,0.08)', borderRadius: 2 }} />
                  </div>
                ))}
              </div>
            )}
            {archiveTransition.recommendations !== null && archiveTransition.recommendations.length > 0 && (
              /* 10 cards on desktop, 5 per row (two rows) — the fixed card width makes the
                 flex-wrap land at 5 across within this container's max width; narrow viewports
                 wrap to fewer per row naturally. */
              <div className="rec-row">
                {archiveTransition.recommendations.slice(0, 10).map((rec) => (
                  <div key={rec.archive_id} className="rec-card">
                    <a
                      href={`/reader/${rec.archive_id}`}
                      title={rec.title}
                      style={{ display: 'block', textDecoration: 'none' }}
                      onClick={() => setArchiveTransition(null)}
                    >
                      <div style={{ position: 'relative' }}>
                        <img
                          src={
                            rec.archive_id.startsWith('TANK_')
                              ? `/api/tankoubons/${rec.archive_id}/thumbnail?no_fallback=true`
                              : `/api/archives/${rec.archive_id}/thumbnail?no_fallback=true`
                          }
                          alt={rec.title}
                          loading="lazy"
                        />
                        {/* Status badges, same emoji set as the Library grid cards (🆕 new /
                            👑 read / 📚 tankoubon). Semi-transparent dark chips stay readable on
                            any thumbnail; neutral overlay chrome, not theme-colored. */}
                        {(rec.isnew || rec.is_read || rec.is_tank) && (
                          <span
                            style={{
                              position: 'absolute',
                              top: 4,
                              left: 4,
                              fontSize: 10,
                              lineHeight: 1,
                              display: 'flex',
                              gap: 3,
                            }}
                          >
                            {/* Square 16px chips (emoji vertically/horizontally centered) — a
                                padding-based chip would be a flat rectangle, since the emoji
                                glyph's own box is taller than its advance width. */}
                            {rec.is_tank && (
                              <span title={t('Tankoubon') ?? undefined} style={badgeChipStyle}>
                                📚
                              </span>
                            )}
                            {rec.isnew && (
                              <span title={t('New!') ?? undefined} style={badgeChipStyle}>
                                🆕
                              </span>
                            )}
                            {rec.is_read && (
                              <span title={t('Read') ?? undefined} style={badgeChipStyle}>
                                👑
                              </span>
                            )}
                          </span>
                        )}
                      </div>
                      <div className="rec-title">
                        <span>{rec.title}</span>
                      </div>
                    </a>
                  </div>
                ))}
              </div>
            )}
            <div style={{ marginTop: 18 }}>
              <input
                type="button"
                className="stdbtn"
                value={t('Return to Library') ?? undefined}
                onClick={() => navigate(routes.library())}
              />
            </div>
          </div>
          {/* Close button lives OUTSIDE the scrollable container — a fixed viewport-top-right
              element, since the container's overflow would clip an absolutely-positioned child
              that sits outside its box (top: -24 was getting cut off). White semi-transparent
              circle, lightbox convention — neutral overlay chrome, not theme-colored. */}
          <button
            type="button"
            aria-label={t('Close') ?? undefined}
            onClick={() => setArchiveTransition(null)}
            style={{
              position: 'fixed',
              top: 16,
              right: 16,
              width: 36,
              height: 36,
              borderRadius: '50%',
              border: 'none',
              cursor: 'pointer',
              padding: 0,
              // Flex centering (not line-height) so the glyph sits on the button's true center
              display: 'inline-flex',
              alignItems: 'center',
              justifyContent: 'center',
              background: 'rgba(255,255,255,0.2)',
              color: '#fff',
              zIndex: 9002,
            }}
            onMouseEnter={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.45)')}
            onMouseLeave={(e) => (e.currentTarget.style.background = 'rgba(255,255,255,0.2)')}
          >
            <i className="fas fa-times" aria-hidden="true"></i>
          </button>
        </>
      )}
    </div>
    <Footer />
    </>
  )
}
