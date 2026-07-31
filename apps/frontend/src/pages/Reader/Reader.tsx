import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import {
  fetchRandomArchiveId,
  useArchiveMetadata,
  useArchivePages,
  useBookmarkLink,
  useCategories,
  useGenerateThumbnails,
  useLoginStatus,
  useSettings,
  useUpdateProgress,
} from '../../api/hooks'
import Footer from '../../components/Footer'
import { promptDialog } from '../../dialog'
import { getTagSearchURL } from '../../lib/tagFormat'
import { routes } from '../../routes'
import { useApplyTheme } from '../../theme'
import { toast } from '../../toast'
import { useDocumentTitle } from '../../useDocumentTitle'
import ArchiveOverviewOverlay from './ArchiveOverviewOverlay'
import {
  type ArchiveNavState,
  resolveAdjacentArchive,
  setupArchiveNavigation,
} from './crossArchiveNav'
import { fileInfoText } from './fileInfoText'
import MarkerLayer from './MarkerLayer'
import SettingsOverlay from './SettingsOverlay'
import { clamp, computeNextPage, computeSpread } from './useReaderNavigation'
import { useReaderSettings } from './useReaderSettings'

// Faithful port of legacy's reader page (`~/LANraragi/templates/reader.html.tt2` +
// `~/LANraragi/public/js/reader.js`) — real DOM structure (`#i1`-`#i7`) and CSS classnames from
// `/legacy/lrr.css`, not Tailwind.

/** Uniform icon size for every paginator (prev/next-archive, prev/next-page) nav link. */
const PAGINATOR_ICON_FONT_SIZE = '1.5em'
/** Matches `toast.tsx`'s own `AUTO_CLOSE_TIME.info` default — specified explicitly to make clear
 * this is deliberate, not incidental inheritance. */
const TOAST_DURATION_MS = 5000

type OverlayKind = 'archive' | 'settings' | 'help' | null

interface WakeLockSentinelLike {
  release(): Promise<void>
  addEventListener(type: 'release', listener: () => void): void
}

export default function Reader() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { archiveId = null } = useParams<{ archiveId: string }>()
  useApplyTheme()

  const metadata = useArchiveMetadata(archiveId)
  const pages = useArchivePages(archiveId)
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
  const updateProgress = useUpdateProgress(archiveId)
  const generateThumbnails = useGenerateThumbnails(archiveId ?? '')
  const [readerSettings, updateReaderSettings] = useReaderSettings()

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
  const infiniteScrollObserverPage = useRef<number | null>(null)
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

  const currentPage = clamp(
    pageOverride ?? Math.max(metadata.data?.progress ?? 1, 1),
    1,
    totalPages || 1,
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

  // Legacy toggles infinite-scroll mode via `$("body").addClass("infinite-scroll")`
  // (`initInfiniteScrollView`, reader.js:674) — a body-level class, not on `#i1` — since
  // `lrr.css`'s hide rules for `#i2`/`.sn`/etc. are all scoped `body.infinite-scroll #selector`.
  useEffect(() => {
    document.body.classList.toggle('infinite-scroll', readerSettings.infiniteScroll)
    return () => document.body.classList.remove('infinite-scroll')
  }, [readerSettings.infiniteScroll])

  // Progress persistence decision tree (verified against legacy's `updateProgress`):
  // authprogress+logged_in -> server; localprogress -> localStorage; neither -> server anyway.
  useEffect(() => {
    if (!archiveId || totalPages === 0) return
    if (settings.data?.localprogress && !(settings.data?.authprogress && loggedIn)) {
      localStorage.setItem(`${archiveId}-reader`, String(currentPage))
    } else {
      updateProgress.mutate(currentPage)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [archiveId, currentPage, totalPages])

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
        void readAdjacentArchive(goingForward ? 'next' : 'prev')
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
      fetch(img.src, { method: 'HEAD' })
        .then((res) => {
          const bytes = Number(res.headers.get('Content-Length'))
          if (!Number.isNaN(bytes)) {
            setPageSizesKb((prev) => ({ ...prev, [page]: Math.floor(bytes / 1024) }))
          }
        })
        .catch(() => undefined)
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

  function cleanCache() {
    generateThumbnails.mutate()
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
          setOverlay(null)
          return
        case ' ':
          window.scrollBy({ top: window.innerHeight * 0.8, behavior: 'smooth' })
          return
        case 'ArrowLeft':
        case 'a':
          goTo(e.shiftKey ? 'first' : 'prev')
          return
        case 'ArrowRight':
        case 'd':
          goTo(e.shiftKey ? 'last' : 'next')
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
          if (!readerSettings.infiniteScroll) setMarkerPlacementMode(true)
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
  ])

  // Infinite scroll: tracks which mounted page is nearest the viewport center and treats that as
  // "current" for progress purposes — legacy's own `IntersectionObserver`-per-image approach
  // (reader.js:684), reimplemented as one observer watching every page `<img>` at once.
  useEffect(() => {
    if (!readerSettings.infiniteScroll || totalPages === 0) return
    const root = infiniteScrollRootRef.current
    if (!root) return

    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue
          const page = Number((entry.target as HTMLElement).dataset.page)
          if (!Number.isNaN(page) && infiniteScrollObserverPage.current !== page) {
            infiniteScrollObserverPage.current = page
            setPageOverride(page)
          }
        }
      },
      { threshold: 0.5 },
    )
    const images = root.querySelectorAll<HTMLElement>('[data-page]')
    images.forEach((img) => observer.observe(img))
    return () => observer.disconnect()
  }, [readerSettings.infiniteScroll, totalPages, pages.data])

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

  const leftUrl = pages.data.pages[spread.left - 1]
  const rightUrl = spread.right !== null ? pages.data.pages[spread.right - 1] : null

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

  const bookmarkLinkConfigured = Boolean(bookmarkCategoryId)

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

  const arrows = (
    <div className="sn paginator">
      <a
        className="fas fa-backward-step page-link archive-nav-link"
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE, display: navState.ids.length > 0 ? undefined : 'none' }}
        onClick={() => void readAdjacentArchive('prev')}
      />
      <a
        className="fas fa-angle-double-left page-link"
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('first')}
      />
      <a
        className="fas fa-angle-left page-link"
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('prev')}
      />
      <div className="pagecount">
        <span className="current-page">{currentPage}</span> / <span className="max-page">{totalPages}</span>
      </div>
      <a
        className="fas fa-angle-right page-link"
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('next')}
      />
      <a
        className="fas fa-angle-double-right page-link"
        style={{ fontSize: PAGINATOR_ICON_FONT_SIZE }}
        onClick={() => goTo('last')}
      />
      <a
        className="fas fa-forward-step page-link archive-nav-link"
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
            {pages.data.pages.map((url, i) => (
              <img
                key={url}
                data-page={i + 1}
                className="reader-image"
                src={url}
                alt={`${t('Page')} ${i + 1}`}
                loading="lazy"
                draggable={false}
                style={imageStyle}
                onClick={(e) => {
                  const isLeftHalf = e.clientX < window.innerWidth / 2
                  goTo(isLeftHalf ? 'prev' : 'next')
                }}
              />
            ))}
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
                style={imageStyle}
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
            {archiveId && (
              <MarkerLayer
                archiveId={archiveId}
                page={spread.left}
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
            <div className="navigation-help-toast">
              {t('You can navigate between pages using:')}
              <ul>
                <li>{t('The arrow icons')}</li>
                <li>{t('The a/d keys')}</li>
                <li>{t('Your keyboard arrows (and the spacebar)')}</li>
                <li>{t('Touching the left/right side of the image.')}</li>
              </ul>
              {t('When reading an archive from search results, you can also navigate between archives using:')}
              <ul>
                <li>{t('The , and . keys')}</li>
                <li>{t('Reading past the first/last page')}</li>
              </ul>
              <br />
              {t('Other keyboard shortcuts:')}
              <ul>
                <li>{t('M: toggle manga mode (right-to-left reading)')}</li>
                <li>{t('O: show advanced reader options.')}</li>
                <li>{t('P: toggle double page mode')}</li>
                <li>{t('Q: bring up the thumbnail index and archive options.')}</li>
                <li>{t('R: open a random archive.')}</li>
                <li>{t('F: toggle fullscreen mode')}</li>
                <li>{t('B: toggle bookmark')}</li>
                <li>{t('N: toggle auto next page')}</li>
                <li>{t('G: go to page number')}</li>
                <li>{t('S: set a Stamp')}</li>
              </ul>
              <br />
              {t('To return to the archive index, touch the arrow pointing down or use Backspace.')}
            </div>
          </div>
        </>
      )}
    </div>
    <Footer />
    </>
  )
}
