import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useArchivePages } from "@/api/hooks"
import type { ArchivePage } from "@/api/types"
import { Tooltip } from "@/components/Display"
import { useHorizontalScroll } from "@/hooks"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { fetchContentLengthKb } from "@/lib/utils/imageMeta"
import { displayTocName, TOC_IDENTIFIER_TABLE_OF_CONTENTS, tocChapterIdentifier } from "@/lib/utils/tocValidation"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"

import {
  ChapterActionMenu,
  chapterForPage,
  chapterSwatchColor,
  isDarkColor,
  openChapterActionMenuCount,
  QuickAddTocOptions,
  tocChapterSpans,
} from "./shared"

/** One frame in `PageLightbox`'s own bottom filmstrip gallery — a small thumbnail (reuses the
 * same cheap `/thumbnail?page=N` endpoint the page grid itself already uses, not the full-size
 * page image; a gallery of a few hundred full-size images loading at once would be far heavier
 * for no visual benefit at this size) that becomes the large preview on hover, without touching
 * the reader's own reading progress or the overview grid's own scroll position (both entirely
 * unaffected — this lightbox is its own, separate, throwaway "preview" state layered on top). */
function LightboxFilmstripFrame({
  archiveId,
  page,
  isPreview,
  accentColor,
  borderColor,
  chapter,
  onHover,
  onClick,
}: {
  archiveId: string
  page: number
  isPreview: boolean
  accentColor: string
  borderColor: string
  /** The chapter this page belongs to, if any — rendered as a small colored swatch + truncated
   * title strip beneath the thumbnail itself (see `chapterForPage`/`chapterSwatchColor`), so a
   * chapter's extent is visible at a glance while scrubbing the filmstrip instead of only showing
   * up once hovered into the large preview above. `label` is pre-formatted by the caller (plain
   * chapter name on the chapter's own start frame, "name (ordinal)" — e.g. "第 1 章 (5)" — on
   * later frames, an incrementing per-frame count rather than one fixed total repeated on every
   * frame, so scrubbing through a chapter shows *how far into it* each specific frame is). */
  chapter?: { label: string; swatch: string }
  onHover: () => void
  onClick: () => void
}) {
  return (
    <div data-filmstrip-page={page} style={{ flex: "0 0 auto", width: 90, display: "flex", flexDirection: "column", gap: 2 }}>
      <div
        onMouseEnter={onHover}
        onClick={onClick}
        style={{
          position: "relative",
          width: 90,
          height: 120,
          cursor: "pointer",
          outline: isPreview ? `3px solid ${accentColor}` : `1px solid ${borderColor}`,
          outlineOffset: -1,
        }}
      >
        <img
          src={`/api/archives/${archiveId}/thumbnail?page=${page}`}
          alt={`${page}`}
          loading="lazy"
          style={{ width: "100%", height: "100%", objectFit: "cover" }}
        />
      </div>
      {chapter && (
        <div style={{ display: "flex", alignItems: "center", gap: 3, minWidth: 0 }} title={chapter.label}>
          <span style={{ flex: "0 0 auto", width: 8, height: 8, borderRadius: "50%", background: chapter.swatch }} />
          <span style={{ flex: "1 1 auto", minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: 10 }}>
            {chapter.label}
          </span>
        </div>
      )}
    </div>
  )
}

/** Continuous-scroll hot zone at one edge of the filmstrip — hovering it scrolls the filmstrip
 * toward that edge for as long as the pointer stays over it (not a single fixed-distance nudge
 * per click/hover), matching a real "hover the edge, it just keeps going" gallery scrubber. Visible
 * chevron buttons (rather than an invisible hover margin) so the affordance is discoverable at a
 * glance, matching the Library carousel's own real `.carousel-prev`/`.carousel-next` chevrons. */
function LightboxFilmstripEdge({ direction, onScroll }: { direction: "left" | "right"; onScroll: (delta: number) => void }) {
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)
  function start() {
    stop()
    intervalRef.current = setInterval(() => onScroll(direction === "left" ? -16 : 16), 16)
  }
  function stop() {
    if (intervalRef.current !== null) clearInterval(intervalRef.current)
    intervalRef.current = null
  }
  useEffect(() => stop, [])
  return (
    <div
      onMouseEnter={start}
      onMouseLeave={stop}
      onTouchStart={(e) => { e.preventDefault(); start() }}
      onTouchEnd={stop}
      onTouchCancel={stop}
      style={{
        position: "absolute",
        top: 0,
        bottom: 0,
        [direction]: 0,
        width: 48,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
        zIndex: 1,
        // Deliberately theme-independent (unlike the rest of the lightbox, which follows
        // `useMenuPalette()`) — this is a dark photo-viewer-style scrim overlaid *on top of* the
        // filmstrip thumbnails themselves, not a themed content surface, so a fixed dark
        // gradient + white icon (reliable contrast against any thumbnail's own colors) makes more
        // sense here than chasing the active theme's own (possibly light) palette.
        background: `linear-gradient(to ${direction}, transparent, rgba(0,0,0,0.6))`,
      }}
    >
      <i className={`fa fa-3x fa-chevron-${direction}`} style={{ color: "white" }} aria-hidden="true"></i>
    </div>
  )
}

/** Full-page-image preview, opened via the magnifying-glass icon on a page-grid cell — the small
 * grid thumbnails (~100px tall) are too small to actually judge a page's content by eye, which
 * makes deciding exactly where a chapter starts/ends (the whole point of the quick-add-chapter
 * feature) guesswork at that size. Layered *on top of* the Archive Overview modal (which stays
 * open underneath, unlike a normal "close the old one first" modal stack) rather than replacing
 * it, so closing the lightbox returns to exactly where the grid was.
 *
 * The bottom filmstrip's own hover-to-preview is intentionally decoupled from both the reader's
 * real reading progress (`useUpdateProgress` is never called here) and the overview grid's own
 * scroll position underneath (this component owns its own `previewPage` state, entirely separate
 * from `currentPage`/`highlightedPage`) — this is a scratch "look around" tool, not a navigation
 * action, and shouldn't have any lasting side effect just from hovering around in it. */
export function PageLightbox({
  archiveId,
  initialPage,
  toc,
  loggedIn,
  onQuickAddToc,
  onEditToc,
  onRemoveToc,
  onClose,
  pagesOverride,
  resolvePage,
}: {
  archiveId: string
  initialPage: number
  toc: { page: number; name: string }[]
  loggedIn: boolean
  onQuickAddToc: (page: number, title: string) => void
  onEditToc: (entry: { page: number; name: string }) => void
  onRemoveToc: (entry: { page: number; name: string }) => void
  onClose: () => void
  /** Tankoubon-mode only: the already-concatenated multi-archive page list
   * (`useTankoubonReading`'s own `pages.data.pages`) — used instead of fetching `archiveId`'s own
   * pages internally, since `archiveId` is then the Tankoubon's own id, not a real archive's. */
  pagesOverride?: ArchivePage[]
  /** Tankoubon-mode only companion to `pagesOverride` — resolves one of its own *global* page
   * numbers back to the real member archive (and that archive's own local page number) whose
   * thumbnail the filmstrip should actually show for it. `undefined` in the plain single-archive
   * case, where every frame already just uses `archiveId`/the raw page number directly. */
  resolvePage?: (globalPage: number) => { arcId: string; localPage: number } | null
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const singlePages = useArchivePages(pagesOverride ? null : archiveId)
  const pages = pagesOverride ? { data: { pages: pagesOverride } } : singlePages
  const totalPages = pages.data?.pages.length ?? 0
  const [previewPage, setPreviewPage] = useState(initialPage)
  // Same `ChapterActionMenu` the overview's own chapter-selector row uses (see that JSX further
  // up this file) — duplicated here as a second, independent trigger next to the lightbox's own
  // chapter `<select>`, so editing/deleting a chapter doesn't require closing the lightbox first.
  const [editTocMenuAt, setEditTocMenuAt] = useState<DOMRect | null>(null)
  const [removeTocMenuAt, setRemoveTocMenuAt] = useState<DOMRect | null>(null)
  // Keyed by the page it was measured for, rather than reset via a separate effect whenever
  // `previewPage` changes — `img.onLoad`'s own measurement is naturally already scoped to
  // whichever page's `<img>` just loaded, so comparing `measured.page === previewPage` below is
  // enough to tell a stale previous-page measurement from a current one, with no extra effect
  // needed just to null out state on every page change.
  const [measured, setMeasured] = useState<{ page: number; width: number; height: number; sizeKb: number | null } | null>(null)
  const filmstripRef = useRef<HTMLDivElement>(null)
  const lenisRef = useHorizontalScroll(filmstripRef)
  // Set right before an arrow-key-driven `setPreviewPage` call, read (and cleared) by the
  // scroll-into-view effect below — hover-driven page changes deliberately do NOT scroll the
  // filmstrip (the user's own scroll position while scrubbing shouldn't be fought), but keyboard
  // navigation should keep the active frame visible since there's no other way to see where you
  // landed once it scrolls out of the visible strip.
  const scrollFilmstripOnNextPage = useRef(false)
  // Set for the duration of a keyboard-triggered `scrollIntoView` below (and briefly after) —
  // see the filmstrip frames' own `onHover` docs for why: scrolling the strip out from under a
  // stationary mouse cursor fires a real `mouseenter` on whichever frame the pointer ends up
  // over, which would otherwise immediately override the arrow key's own page change. Starts
  // `true` (not `false`) for the same reason, generalized to the moment the lightbox itself opens
  // — the cursor is very likely already sitting somewhere over the lightbox (right where the
  // magnifying-glass icon that opened it was clicked), and if that happens to land over a
  // filmstrip frame once the mount-time scroll-to-`initialPage` effect below runs, the resulting
  // `mouseenter` would immediately clobber `initialPage` with whatever page the mouse happens to
  // be resting over — cleared 1s after mount by the effect right below this ref.
  const suppressHoverRef = useRef(true)

  const pageUrl = pages.data?.pages[previewPage - 1]?.url
  const dimensions = measured?.page === previewPage ? measured : null

  useEffect(() => {
    const timer = setTimeout(() => {
      suppressHoverRef.current = false
    }, 1000)
    return () => clearTimeout(timer)
  }, [])

  useEffect(() => {
    if (!scrollFilmstripOnNextPage.current) return
    scrollFilmstripOnNextPage.current = false
    suppressHoverRef.current = true
    const lenis = lenisRef.current
    const strip = filmstripRef.current
    const frame = strip?.querySelector<HTMLElement>(`[data-filmstrip-page="${previewPage}"]`)
    if (lenis && strip && frame) {
      lenis.scrollTo(frame.offsetLeft - strip.clientWidth / 2 + frame.clientWidth / 2)
    } else if (strip && frame) {
      // Fallback before Lenis is ready
      frame.scrollIntoView({ block: "nearest", inline: "nearest" })
    }
    const timer = setTimeout(() => {
      suppressHoverRef.current = false
    }, 100)
    return () => clearTimeout(timer)
  // lenisRef is a ref (stable), read only at call time — not an effect dependency.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewPage])

  // Scrolls the filmstrip to the page the lightbox was opened on. Waits on both `pages.data` and
  // the Lenis instance being ready (the hook's own useEffect fires after mount). Centered manually
  // via `offsetLeft`/`clientWidth` — `scrollIntoView({inline:'center'})` was confirmed live to not
  // reliably work inside this nested `position:fixed` modal's `overflow-x:auto` container.
  const centeredRef = useRef(false)
  useEffect(() => {
    if (centeredRef.current) return
    const lenis = lenisRef.current
    if (!lenis || !pages.data) return
    const strip = filmstripRef.current
    const frame = strip?.querySelector<HTMLElement>(`[data-filmstrip-page="${initialPage}"]`)
    if (!strip || !frame) return
    lenis.scrollTo(frame.offsetLeft - strip.clientWidth / 2 + frame.clientWidth / 2, { immediate: true })
    centeredRef.current = true
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pages.data, lenisRef.current])

  // Latest-value refs for the keydown listener below — kept in a ref rather than read directly
  // from the closure so the listener itself can be registered exactly once (see that effect's own
  // docs on why: re-registering on every `previewPage` change was the real cause of a live-
  // confirmed bug where a fast arrow-key repeat occasionally jumped to an unexpected earlier
  // page — the effect tearing down and re-adding the `window` listener on every keystroke raced
  // against the browser's own OS-level key-repeat firing the next `keydown` before React's next
  // commit had finished swapping the old closure for the new one, so a stale `previewPage` value
  // from the *previous* closure could still be captured by that in-flight event).
  const latest = useRef({ previewPage, totalPages, loggedIn, onQuickAddToc, onClose })
  useLayoutEffect(() => {
    latest.current = { previewPage, totalPages, loggedIn, onQuickAddToc, onClose }
  })

  // Registered on the `capture` phase specifically — `Reader.tsx`'s own global `window.keydown`
  // listener (bubble phase, no capture) also treats `Escape`/arrow keys as its own reader
  // navigation, which would move the actual reading position or close the Archive Overview modal
  // underneath this lightbox if it ran first. Capture-phase listeners always run before
  // bubble-phase ones regardless of registration order, so this reliably intercepts the key first.
  // Registered exactly once (empty deps) — see `latest` ref above for why re-registering per
  // keystroke was itself the bug, not just unnecessary churn.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const { totalPages, loggedIn, onQuickAddToc, onClose } = latest.current
      if (e.key === "Escape") {
        // Skip entirely if a `ChapterActionMenu` (edit/delete chapter) is currently open above
        // this lightbox — see `openChapterActionMenuCount`'s own docs for why a plain counter
        // check, not `stopImmediatePropagation`, is what actually solves this: this listener is
        // registered (at lightbox-mount time) *before* any menu's own listener could possibly
        // exist (menus only mount later, on click), and capture-phase listeners on the same
        // `window` target fire in registration order — so without this check, this handler
        // always ran first and closed the whole lightbox on Escape even while a menu was open
        // above it, no matter what that menu's own listener tried to do afterward (a real,
        // live-confirmed bug).
        if (openChapterActionMenuCount > 0) return
        // `stopImmediatePropagation`, not just `stopPropagation` — both this listener and
        // `Reader.tsx`'s own are registered on the *same* `window` object (just different phases,
        // capture vs. bubble); per the DOM spec, plain `stopPropagation` only blocks propagation
        // to *other* targets in the tree, not other listeners already registered on this same
        // target, so it alone did not actually stop `Reader.tsx`'s bubble-phase listener from
        // also firing (confirmed live: Escape closed both the lightbox *and* the Archive Overview
        // modal underneath it even with plain `stopPropagation` in place).
        e.stopImmediatePropagation()
        onClose()
        return
      }
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.stopImmediatePropagation()
        e.preventDefault()
        scrollFilmstripOnNextPage.current = true
        // Functional update, not `previewPage + delta` off a value read from `latest.current` —
        // React guarantees a functional updater always sees the most recently *queued* state, so
        // it can't go stale even under fast repeated calls, unlike reading a plain closed-over/
        // ref-cached value (a real, live-confirmed bug: rapid ArrowRight presses could jump
        // backward mid-chapter when a stale `previewPage` snapshot got used for the next update
        // before the ref had synced past it).
        setPreviewPage((p) => Math.min(totalPages, Math.max(1, p + (e.key === "ArrowLeft" ? -1 : 1))))
        return
      }
      // Point 6: 0-9 (top row or numpad) sets a chapter at the current preview page — 0 = Table of
      // Contents, 1-9 = that chapter number. Only fires while logged in (same guard as the
      // flattened `QuickAddTocOptions` row below, which this is a keyboard shortcut for).
      if (loggedIn && /^(Digit|Numpad)[0-9]$/.test(e.code)) {
        const n = e.code.slice(-1)
        e.stopImmediatePropagation()
        e.preventDefault()
        const title = n === "0" ? TOC_IDENTIFIER_TABLE_OF_CONTENTS : tocChapterIdentifier(Number(n))
        onQuickAddToc(latest.current.previewPage, title)
      }
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [])

  const chapterSpans = tocChapterSpans(toc, totalPages)
  const currentChapter = chapterForPage(chapterSpans, previewPage)
  const filmstripChapterByPage = new Map(
    (pages.data?.pages ?? []).map((_, i) => {
      const page = i + 1
      const chapter = chapterForPage(chapterSpans, page)
      return [page, chapter] as const
    }),
  )
  const onDarkBg = isDarkColor(palette.bg)

  function scrollFilmstrip(delta: number) {
    const lenis = lenisRef.current
    if (lenis) lenis.scrollTo(lenis.targetScroll + delta)
  }

  return (
    <>
      {/* `Z_OVERLAY_ABOVE_LEGACY_MODAL`, not `Z_OVERLAY_BACKDROP`/`Z_OVERLAY_CONTENT` — this
          lightbox layers on top of the still-open Archive Overview modal underneath it
          (`#archivePagesOverlay`, legacy's own `.base-overlay` class carrying a hardcoded
          `z-index: 9000`), which the app's own generic overlay tiers don't clear (a real,
          live-confirmed bug: the lightbox rendered fully invisible behind that modal despite
          mounting later, since z-index — not DOM/mount order — decides paint order here).
          Background is the active theme's own `palette.bg` at reduced opacity (not a fixed
          `rgba(0,0,0,...)` scrim) — still reads as "dim what's behind", but the dimming color
          itself now follows the theme instead of always going pure black regardless of it. */}
      <div
        style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_ABOVE_LEGACY_MODAL, background: palette.bg, opacity: 0.9 }}
        onClick={(e) => {
          // `stopPropagation` — same real, live-confirmed bug class as `QuickAddTocPopover`'s own
          // backdrop earlier in this file: without it, this click bubbles up to `#overlay-shade`
          // (the Archive Overview modal's own click-to-close backdrop, still mounted underneath
          // and covering the same full viewport) and closes *that* too, not just this lightbox.
          e.stopPropagation()
          onClose()
        }}
      />
      <div
        style={{
          position: "fixed",
          inset: "3vh 3vw",
          zIndex: Z_OVERLAY_ABOVE_LEGACY_MODAL + 1,
          display: "flex",
          flexDirection: "column",
          background: palette.bg,
          color: palette.text,
          border: `1px solid ${palette.border}`,
          borderRadius: 8,
          overflow: "hidden",
        }}
        onClick={(e) => e.stopPropagation()}
      >

        {/* Large preview + info bar + flattened quick-add-chapter row. */}
        <div style={{ flex: "1 1 auto", display: "flex", flexDirection: "column", minHeight: 0, padding: 16 }}>
          <div style={{ flex: "1 1 auto", minHeight: 0, display: "flex", alignItems: "center", justifyContent: "center" }}>
            {pageUrl && (
              <img
                key={pageUrl}
                src={pageUrl}
                alt={t("common.pageN", { n: previewPage }) ?? undefined}
                style={{ maxWidth: "100%", maxHeight: "100%", objectFit: "contain" }}
                onLoad={(e) => {
                  const img = e.currentTarget
                  const page = previewPage
                  setMeasured({ page, width: img.naturalWidth, height: img.naturalHeight, sizeKb: null })
                  void fetchContentLengthKb(img.src).then((kb) => {
                    if (kb !== null) setMeasured((prev) => (prev?.page === page ? { ...prev, sizeKb: kb } : prev))
                  })
                }}
              />
            )}
          </div>

          <div style={{ flex: "0 0 auto", textAlign: "center", padding: "8px 0", fontSize: 13 }}>
            {t("common.pageN", { n: previewPage })}
            {" :: "}
            {pageUrl ? (new URL(pageUrl, window.location.origin).searchParams.get("path") ?? "") : ""}
            {dimensions && ` :: ${dimensions.width} x ${dimensions.height}`}
            {dimensions?.sizeKb !== null && dimensions?.sizeKb !== undefined && ` :: ${dimensions.sizeKb} KB`}
            {currentChapter && (
              <>
                {" :: "}
                {/* Bold/accent color only on the chapter's own start page — later pages belonging
                    to the same chapter show it in plain text plus a "(N)" total-page-count
                    suffix, so scrubbing through a long chapter still shows which one you're in
                    without visually implying *this* page is where it was set. */}
                {currentChapter.isStart ? (
                  <span style={{ fontWeight: "bold", color: palette.hoverText }}>{displayTocName(currentChapter.name, t)}</span>
                ) : (
                  <span>{t("reader.count", { name: displayTocName(currentChapter.name, t), count: currentChapter.count })}</span>
                )}
              </>
            )}
          </div>

          {loggedIn && (
            <div style={{ flex: "0 0 auto", display: "flex", justifyContent: "center", alignItems: "center", gap: 12, flexWrap: "wrap", padding: "4px 0" }}>
              {/* Same icon+text pairing as `QuickAddTocPopover`'s own `mainLabel` header (the
                  popover version of this same preset row) — reusing that already-translated
                  string here too instead of introducing a second, differently-worded label for
                  what is otherwise the identical feature just rendered inline. */}
              <span style={{ fontWeight: "bold", opacity: 0.85 }}>
                <i className="fa fa-bolt" style={{ width: 18 }} aria-hidden="true"></i> {t("reader.quickAddChapter")}
              </span>
              <QuickAddTocOptions asMenuItems={false} onPick={(title) => onQuickAddToc(previewPage, title)} />
              <Tooltip label={t("reader.press0ForTableOf") ?? ""}>
                <i className="fa fa-keyboard" aria-hidden="true" style={{ cursor: "help", color: palette.text, opacity: 0.7 }}></i>
              </Tooltip>
              {toc.length > 0 && (
                <>
                  <a
                    className="fas fa-pencil-alt"
                    href="#"
                    style={{ padding: 4, fontSize: 14, color: palette.text }}
                    title={t("common.editChapterName") ?? undefined}
                    onClick={(e) => {
                      e.preventDefault()
                      setEditTocMenuAt(e.currentTarget.getBoundingClientRect())
                    }}
                  />
                  {editTocMenuAt && (
                    <ChapterActionMenu
                      mode="edit"
                      anchor={editTocMenuAt}
                      chapters={toc}
                      zIndexBase={Z_OVERLAY_ABOVE_LEGACY_MODAL + 2}
                      onPick={(entry) => {
                        onEditToc(entry)
                        setEditTocMenuAt(null)
                      }}
                      onClose={() => setEditTocMenuAt(null)}
                    />
                  )}
                  <a
                    className="fas fa-trash-alt"
                    href="#"
                    style={{ padding: 4, fontSize: 14, color: palette.text }}
                    title={t("common.deleteChapter") ?? undefined}
                    onClick={(e) => {
                      e.preventDefault()
                      setRemoveTocMenuAt(e.currentTarget.getBoundingClientRect())
                    }}
                  />
                  {removeTocMenuAt && (
                    <ChapterActionMenu
                      mode="delete"
                      anchor={removeTocMenuAt}
                      chapters={toc}
                      zIndexBase={Z_OVERLAY_ABOVE_LEGACY_MODAL + 2}
                      onPick={(entry) => {
                        onRemoveToc(entry)
                        setRemoveTocMenuAt(null)
                      }}
                      onClose={() => setRemoveTocMenuAt(null)}
                    />
                  )}
                </>
              )}
            </div>
          )}
        </div>

        {/* Bottom filmstrip gallery. */}
        {/* Height accounts for the 8px top/bottom padding + 120px frame + the chapter-label row
            each `LightboxFilmstripFrame` now renders beneath its thumbnail (point 2) — without
            the extra room, that label row pushed total content past the old fixed 136px and
            triggered an unwanted vertical scrollbar (confirmed live via a real screenshot). */}
        <div style={{ position: "relative", flex: "0 0 auto", height: 158, background: palette.bg, borderTop: `1px solid ${palette.separator}` }}>
          <LightboxFilmstripEdge direction="left" onScroll={scrollFilmstrip} />
          <LightboxFilmstripEdge direction="right" onScroll={scrollFilmstrip} />
          <div
            ref={filmstripRef}
            style={{ display: "flex", gap: 4, height: "100%", overflowX: "auto", padding: "8px 48px", scrollBehavior: "auto" }}
          >
            {(pages.data?.pages ?? []).map((_, i) => {
              const page = i + 1
              const chapter = filmstripChapterByPage.get(page)
              const resolved = resolvePage ? resolvePage(page) : { arcId: archiveId, localPage: page }
              if (!resolved) return null
              return (
                <LightboxFilmstripFrame
                  key={i}
                  archiveId={resolved.arcId}
                  page={resolved.localPage}
                  isPreview={page === previewPage}
                  accentColor={palette.hoverText}
                  borderColor={palette.border === "transparent" ? palette.text : palette.border}
                  chapter={
                    chapter && {
                      // Swatch color is keyed on the raw stored `chapter.name` (the identifier,
                      // when it's a preset) rather than the display text — display text is
                      // locale-dependent, and the swatch should stay the same color regardless of
                      // which language the UI happens to be showing at the moment.
                      label: chapter.isStart
                        ? displayTocName(chapter.name, t)
                        : (t("reader.count", { name: displayTocName(chapter.name, t), count: chapter.ordinal }) ?? displayTocName(chapter.name, t)),
                      swatch: chapterSwatchColor(chapter.name, onDarkBg),
                    }
                  }
                  onHover={() => {
                    // Suppressed right after a keyboard-driven scroll — `scrollIntoView` moving
                    // the filmstrip out from under a *stationary* mouse cursor still fires a real
                    // `mouseenter` on whatever frame ends up under the pointer, which otherwise
                    // clobbered the arrow key's own `setPreviewPage` call an instant after it ran
                    // (a real, live-confirmed bug: pressing → at the last visible frame scrolled
                    // the strip, and the resulting `mouseenter` silently jumped the preview back
                    // to an earlier page under the now-stationary cursor).
                    if (suppressHoverRef.current) return
                    setPreviewPage(page)
                  }}
                  onClick={() => setPreviewPage(page)}
                />
              )
            })}
          </div>
        </div>
      </div>
    </>
  )
}
