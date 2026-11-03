import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useArchivePages } from "@/api/hooks"
import type { ArchivePage } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
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

/** One frame in the bottom filmstrip gallery — a small thumbnail (`/thumbnail?page=N`) that
 * becomes the large preview on hover, without touching reading progress or grid scroll position. */
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
  /** Chapter this page belongs to, if any — swatch + label beneath the thumbnail. `label` is
   * pre-formatted by the caller: plain name on the chapter's start frame, "name (ordinal)" after. */
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

/** Continuous-scroll hot zone at one edge of the filmstrip — hovering it scrolls that direction
 * for as long as the pointer stays over it, matching the Library carousel's chevron pattern. */
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
        background: `linear-gradient(to ${direction}, transparent, rgba(0,0,0,0.6))`,
      }}
    >
      <i className={`fa fa-3x fa-chevron-${direction}`} style={{ color: "white" }} aria-hidden="true"></i>
    </div>
  )
}

/** Full-page-image preview, opened via the magnifying-glass icon on a page-grid cell. Layers on
 * top of the Archive Overview modal (stays open underneath) rather than replacing it. */
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
  /** Tankoubon-mode only: the already-concatenated multi-archive page list, used instead of
   * fetching `archiveId`'s own pages (which is then the Tankoubon's id, not a real archive's). */
  pagesOverride?: ArchivePage[]
  /** Tankoubon-mode companion to `pagesOverride` — resolves a global page number to the real
   * member archive + local page number. `undefined` in the plain single-archive case. */
  resolvePage?: (globalPage: number) => { arcId: string; localPage: number } | null
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const singlePages = useArchivePages(pagesOverride ? null : archiveId)
  const pages = pagesOverride ? { data: { pages: pagesOverride } } : singlePages
  const totalPages = pages.data?.pages.length ?? 0
  const [previewPage, setPreviewPage] = useState(initialPage)
  const [editTocMenuAt, setEditTocMenuAt] = useState<DOMRect | null>(null)
  const [removeTocMenuAt, setRemoveTocMenuAt] = useState<DOMRect | null>(null)
  const [measured, setMeasured] = useState<{ page: number; width: number; height: number; sizeKb: number | null } | null>(null)
  const filmstripRef = useRef<HTMLDivElement>(null)
  const lenisRef = useHorizontalScroll(filmstripRef)
  const scrollFilmstripOnNextPage = useRef(false)
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
      frame.scrollIntoView({ block: "nearest", inline: "nearest" })
    }
    const timer = setTimeout(() => {
      suppressHoverRef.current = false
    }, 100)
    return () => clearTimeout(timer)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewPage])

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

  const latest = useRef({ previewPage, totalPages, loggedIn, onQuickAddToc, onClose })
  useLayoutEffect(() => {
    latest.current = { previewPage, totalPages, loggedIn, onQuickAddToc, onClose }
  })

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const { totalPages, loggedIn, onQuickAddToc, onClose } = latest.current
      if (e.key === "Escape") {
        if (openChapterActionMenuCount > 0) return
        e.stopImmediatePropagation()
        onClose()
        return
      }
      if (e.key === "ArrowLeft" || e.key === "ArrowRight") {
        e.stopImmediatePropagation()
        e.preventDefault()
        scrollFilmstripOnNextPage.current = true
        setPreviewPage((p) => Math.min(totalPages, Math.max(1, p + (e.key === "ArrowLeft" ? -1 : 1))))
        return
      }
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
      <div
        style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_ABOVE_LEGACY_MODAL, background: palette.bg, opacity: 0.9 }}
        onClick={(e) => {
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
                      label: chapter.isStart
                        ? displayTocName(chapter.name, t)
                        : (t("reader.count", { name: displayTocName(chapter.name, t), count: chapter.ordinal }) ?? displayTocName(chapter.name, t)),
                      swatch: chapterSwatchColor(chapter.name, onDarkBg),
                    }
                  }
                  onHover={() => {
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
