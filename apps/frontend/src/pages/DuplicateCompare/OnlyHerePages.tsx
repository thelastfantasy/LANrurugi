import { useEffect, useRef } from "react"
import { useTranslation } from "react-i18next"

import { useArchivePages } from "@/api/hooks"
import type { ComparisonSide } from "@/api/types"
import { IconButton } from "@/components/common-ui/Form"
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme"

import { pageRanges } from "./PageAlignmentBand"
import { SideBadge } from "./SideBadge"

const SWIPE_THRESHOLD_PX = 50

/** The entry name inside a `/files` page URL. The server does not encode `path`, so the name is
 *  everything after `?path=` minus the known trailing params (a name may itself contain `&`). */
function entryName(url: string): string {
  const start = url.indexOf("?path=")
  if (start < 0) return ""
  let name = url.slice(start + "?path=".length)
  for (const suffix of ["&optimize=1", "&source=patch"]) {
    if (name.endsWith(suffix)) name = name.slice(0, -suffix.length)
  }
  return name.split("/").pop() ?? name
}

/** `/files` lists the patch-merged pages, the same index space the comparison reports. */
function useArchivePageNames(archiveId: string) {
  const files = useArchivePages(archiveId)
  const pages = files.data?.pages
  return { urls: pages?.map((p) => p.url), names: pages?.map((p) => entryName(p.url)) }
}

function thumbSrc(archiveId: string, page: number) {
  return `/api/archives/${encodeURIComponent(archiveId)}/thumbnail?page=${page + 1}`
}

/** Pages one side has and the other lacks (extra pages, a bonus chapter, a missing spread): the
 *  samples only cover aligned pairs, so these get their own tiles. */
export function OnlyHerePages({
  side,
  archiveId,
  pages,
  patchPages,
  flashPage,
  registerTile,
  onOpen,
  onPatch,
}: {
  side: ComparisonSide
  archiveId: string
  pages: number[]
  /** Pages of this side that already come from its own patch. */
  patchPages: Set<number>
  flashPage: number | null
  registerTile: (page: number, el: HTMLButtonElement | null) => void
  onOpen: (index: number) => void
  /** Opens the assignment view copying these pages into the other side's patch. */
  onPatch: () => void
}) {
  const { t } = useTranslation()
  const { names } = useArchivePageNames(archiveId)
  if (pages.length === 0) return null
  return (
    <section style={{ marginTop: 24 }}>
      <h4 style={{ display: "flex", alignItems: "center", gap: 8, margin: "0 0 8px", fontSize: FONT_SIZE_SM, color: "inherit" }}>
        <SideBadge side={side} />
        <span style={{ flex: 1, minWidth: 0 }}>
          {t("duplicates.onlyHereHeading", { side: side.toUpperCase(), count: pages.length, pages: pageRanges(pages) })}
        </span>
        <button type="button" className="stdbtn" style={{ margin: 0, minHeight: 32, touchAction: "manipulation" }} onClick={onPatch}>
          <i className="fa fa-puzzle-piece" aria-hidden="true" style={{ marginRight: 4 }} />
          {t("duplicates.patchInto", { target: side === "a" ? "B" : "A" })}
        </button>
      </h4>
      <div style={{ display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(120px, 1fr))", gap: 10 }}>
        {pages.map((page, i) => (
          <button
            key={page}
            ref={(el) => registerTile(page, el)}
            type="button"
            onClick={() => onOpen(i)}
            style={{
              padding: 0,
              border: 0,
              background: "rgba(255,255,255,0.06)",
              borderRadius: 4,
              overflow: "hidden",
              cursor: "zoom-in",
              color: "inherit",
              outline: flashPage === page ? "2px solid #e67e22" : "2px solid transparent",
              outlineOffset: 2,
              transition: "outline-color 0.3s ease",
              scrollMarginTop: 8,
              touchAction: "manipulation",
            }}
          >
            <img
              src={thumbSrc(archiveId, page)}
              alt=""
              loading="lazy"
              style={{ width: "100%", aspectRatio: "5 / 7", objectFit: "contain", display: "block", background: "#000" }}
            />
            <div style={{ fontSize: FONT_SIZE_XS, padding: "4px 6px", lineHeight: 1.4 }}>
              <div style={{ opacity: 0.9 }}>
                {t("duplicates.pageN", { page: page + 1 })}
                {patchPages.has(page) && (
                  <i className="fa fa-puzzle-piece" aria-hidden="true" title={t("duplicates.fromPatch")} style={{ marginLeft: 4 }} />
                )}
              </div>
              {names?.[page] && (
                <div
                  title={names[page]}
                  style={{ opacity: 0.65, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                >
                  {names[page]}
                </div>
              )}
            </div>
          </button>
        ))}
      </div>
    </section>
  )
}

/** Full-page viewer for the only-on-one-side pages: ←/→ or a swipe steps through them. */
export function PageLightbox({
  side,
  archiveId,
  pages,
  index,
  onIndex,
  onClose,
}: {
  side: ComparisonSide
  archiveId: string
  pages: number[]
  index: number
  onIndex: (index: number) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { urls, names } = useArchivePageNames(archiveId)
  const touchStartX = useRef<number | null>(null)
  const page = pages[index]
  const src = urls?.[page] ?? thumbSrc(archiveId, page)
  const hasPrev = index > 0
  const hasNext = index < pages.length - 1

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "ArrowLeft" && index > 0) onIndex(index - 1)
      if (e.key === "ArrowRight" && index < pages.length - 1) onIndex(index + 1)
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [index, pages.length, onIndex])

  const navButton = (dir: -1 | 1) => (
    <IconButton
      icon={dir < 0 ? "fa fa-chevron-left" : "fa fa-chevron-right"}
      size={44}
      disabled={dir < 0 ? !hasPrev : !hasNext}
      onClick={() => onIndex(index + dir)}
      title={t(dir < 0 ? "duplicates.previousPage" : "duplicates.nextPage")}
      style={{ borderRadius: "50%", flexShrink: 0, touchAction: "manipulation" }}
    />
  )

  return (
    <div
      role="dialog"
      aria-modal="true"
      style={{
        position: "fixed",
        inset: 0,
        height: "100dvh",
        zIndex: 9650,
        background: "#000",
        color: "#f2f2f2",
        display: "flex",
        flexDirection: "column",
        padding: "max(8px, env(safe-area-inset-top)) max(8px, env(safe-area-inset-right)) max(8px, env(safe-area-inset-bottom)) max(8px, env(safe-area-inset-left))",
        boxSizing: "border-box",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: FONT_SIZE_SM }}>
        <SideBadge side={side} />
        <span style={{ flex: 1, minWidth: 0 }}>
          {t("duplicates.pageN", { page: page + 1 })} · {index + 1}/{pages.length}
          {names?.[page] && <span style={{ opacity: 0.7, wordBreak: "break-all" }}> · {names[page]}</span>}
        </span>
        <a
          href={`/reader/${archiveId}?p=${page + 1}`}
          target="_blank"
          rel="noreferrer"
          style={{ color: "inherit", textDecoration: "underline" }}
        >
          {t("duplicates.openInReader")}
        </a>
        <IconButton
          icon="fa fa-times"
          size={36}
          onClick={onClose}
          title={t("common.close")}
          className="modal-close-btn"
          style={{ borderRadius: "50%", touchAction: "manipulation" }}
        />
      </div>
      <div
        style={{ flex: 1, minHeight: 0, display: "flex", alignItems: "center", gap: 8, marginTop: 8 }}
        onTouchStart={(e) => {
          touchStartX.current = e.touches[0].clientX
        }}
        onTouchEnd={(e) => {
          const start = touchStartX.current
          touchStartX.current = null
          if (start === null) return
          const dx = e.changedTouches[0].clientX - start
          if (dx > SWIPE_THRESHOLD_PX && hasPrev) onIndex(index - 1)
          if (dx < -SWIPE_THRESHOLD_PX && hasNext) onIndex(index + 1)
        }}
      >
        {navButton(-1)}
        <img
          key={src}
          src={src}
          alt=""
          draggable={false}
          style={{ flex: 1, minWidth: 0, height: "100%", objectFit: "contain", userSelect: "none" }}
        />
        {navButton(1)}
      </div>
    </div>
  )
}
