import type { MouseEvent } from "react"
import { useEffect, useLayoutEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { displayTocName, isReservedTocIdentifier, TOC_CHAPTER_COUNT, TOC_IDENTIFIER_TABLE_OF_CONTENTS, tocChapterIdentifier } from "@/lib/utils/tocValidation"
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

// Mounted-`ChapterActionMenu` count — lets `PageLightbox`'s Escape handler skip closing itself
// when a menu is open above it.
export let openChapterActionMenuCount = 0

export function incrementOpenChapterActionMenuCount(delta: 1 | -1) {
  openChapterActionMenuCount += delta
}

/** Each ToC entry's page span; `count` includes the start page itself. */
export function tocChapterSpans(toc: { page: number; name: string }[], totalPages: number): { page: number; name: string; count: number }[] {
  const sorted = [...toc].sort((a, b) => a.page - b.page)
  return sorted.map((entry, i) => ({
    ...entry,
    count: (sorted[i + 1]?.page ?? totalPages + 1) - entry.page,
  }))
}

/** Which chapter a page belongs to, plus `isStart` and this page's 1-based `ordinal` in it. */
export function chapterForPage(
  spans: { page: number; name: string; count: number }[],
  page: number,
): { page: number; name: string; count: number; isStart: boolean; ordinal: number } | undefined {
  const span = [...spans].filter((c) => c.page <= page).sort((a, b) => b.page - a.page)[0]
  return span ? { ...span, isStart: span.page === page, ordinal: page - span.page + 1 } : undefined
}

// Deterministic string hash — same chapter name, same swatch color.
export function hashString(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0
  return h
}

/** Deterministic swatch color; lightness picked for contrast against the theme's background. */
export function chapterSwatchColor(name: string, onDark: boolean): string {
  const hue = hashString(name) % 360
  return onDark ? `hsl(${hue}, 55%, 62%)` : `hsl(${hue}, 55%, 40%)`
}

/** Rough luminance check — only needs to pick the right side per theme background. */
export function isDarkColor(css: string): boolean {
  const m = css.match(/\d+/g)
  if (!m || m.length < 3) return false
  const [r, g, b] = m.map(Number)
  return (0.299 * r + 0.587 * g + 0.114 * b) / 255 < 0.5
}

/** Anchors the popup's top-right corner to `anchor`'s bottom-right, clamped inside `#archivePagesOverlay`. */
export function anchorPopupToOverviewModal(anchor: DOMRect, width: number, height: number): { top: number; left: number } {
  const margin = 8
  const bounds = document.getElementById("archivePagesOverlay")?.getBoundingClientRect()
  const minLeft = (bounds?.left ?? 0) + margin
  const maxLeft = (bounds?.right ?? window.innerWidth) - width - margin
  const minTop = (bounds?.top ?? 0) + margin
  const maxTop = (bounds?.bottom ?? window.innerHeight) - height - margin
  return {
    left: Math.max(minLeft, Math.min(anchor.right - width, maxLeft)),
    top: Math.max(minTop, Math.min(anchor.bottom, maxTop)),
  }
}

/** Two-pass measure-then-reposition: first render off-screen, then recompute from the measured
 * real size before paint. */
export function useAnchoredMenuPosition(anchor: DOMRect) {
  const [menuEl, setMenuEl] = useState<HTMLUListElement | null>(null)
  const [pos, setPos] = useState<{ top: number; left: number }>({ top: -9999, left: -9999 })

  useLayoutEffect(() => {
    if (!menuEl) return
    const rect = menuEl.getBoundingClientRect()
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setPos(anchorPopupToOverviewModal(anchor, rect.width, rect.height))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [menuEl, anchor.top, anchor.left, anchor.right, anchor.bottom])
  return { setMenuEl, pos }
}

/** Preset rows (封面/封底/目录/彩页 + 第N章 select), shared by the popover and the lightbox. */
export function QuickAddTocOptions({ onPick, asMenuItems }: { onPick: (title: string) => void; asMenuItems: boolean }) {
  const { t } = useTranslation()
  const presets: { icon: string; title: string; value: string }[] = [
    { icon: "fa-file-image", title: t("reader.cover") ?? "Cover", value: t("reader.cover") ?? "Cover" },
    { icon: "fa-file-image", title: t("reader.backCover") ?? "Back Cover", value: t("reader.backCover") ?? "Back Cover" },
    { icon: "fa-list", title: t("reader.tableOfContents") ?? "Table of Contents", value: TOC_IDENTIFIER_TABLE_OF_CONTENTS },
    { icon: "fa-palette", title: t("reader.colorPages") ?? "Color Pages", value: t("reader.colorPages") ?? "Color Pages" },
    { icon: "fa-gift", title: t("reader.omake") ?? "Omake", value: t("reader.omake") ?? "Omake" },
    { icon: "fa-pen-nib", title: t("reader.afterword") ?? "Afterword", value: t("reader.afterword") ?? "Afterword" },
    { icon: "fa-image", title: t("reader.illustration") ?? "Illustration", value: t("reader.illustration") ?? "Illustration" },
  ]
  const Row = asMenuItems ? PopupMenuItem : "div"
  const chapterSelect = (
    <select
      className="favtag-btn"
      defaultValue=""
      style={{ marginLeft: 4 }}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => {
        if (!e.target.value) return
        onPick(tocChapterIdentifier(Number(e.target.value)))
        e.target.value = ""
      }}
    >
      <option value="" disabled>
        {t("reader.chapter")}
      </option>
      {Array.from({ length: TOC_CHAPTER_COUNT }, (_, i) => i + 1).map((n) => (
        <option key={n} value={n}>
          {t("reader.chapterN", { n })}
        </option>
      ))}
    </select>
  )
  return (
    <>
      {presets.map(({ icon, title, value }) => (
        <Row
          key={title}
          onClick={(e: MouseEvent) => {
            e.stopPropagation()
            onPick(value)
          }}
          {...(asMenuItems ? {} : { style: { cursor: "pointer", padding: "4px 8px", display: "flex", alignItems: "center" } })}
        >
          <i className={`fa ${icon}`} style={{ width: 18 }}></i> {title}
        </Row>
      ))}
      <Row {...(asMenuItems ? { style: { cursor: "default" } } : { style: { padding: "4px 8px", display: "flex", alignItems: "center" } })}>
        <i className="fa fa-book-medical" style={{ width: 18 }}></i>
        {chapterSelect}
      </Row>
    </>
  )
}

/** Right-click menu on the "add chapter" icon; every option submits immediately on pick. */
export function QuickAddTocPopover({
  anchor,
  onPick,
  onClose,
}: {
  anchor: DOMRect
  onPick: (title: string) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { setMenuEl, pos } = useAnchoredMenuPosition(anchor)
  return (
    <>
      <div
        style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
        }}
      />
      <PopupMenu
        ref={setMenuEl}
        style={{ position: "fixed", top: pos.top, left: pos.left, zIndex: Z_OVERLAY_CONTENT }}
        mainLabel={{ icon: "fa-bolt", text: t("reader.quickAddChapter") ?? "Quick Add Chapter" }}
      >
        <QuickAddTocOptions
          asMenuItems
          onPick={(title) => {
            onPick(title)
            onClose()
          }}
        />
      </PopupMenu>
    </>
  )
}

/** Shared edit/delete chapter picker — lists every chapter so the user doesn't need to navigate
 * to it first; `mode` switches icon/label/behavior. */
export function ChapterActionMenu({
  mode,
  anchor,
  chapters,
  zIndexBase = Z_OVERLAY_BACKDROP,
  onPick,
  onClose,
}: {
  mode: "edit" | "delete"
  anchor: DOMRect
  chapters: { page: number; name: string }[]
  zIndexBase?: number
  onPick: (entry: { page: number; name: string }) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { setMenuEl, pos } = useAnchoredMenuPosition(anchor)
  const icon = mode === "edit" ? "fa-pencil-alt" : "fa-trash-alt"
  const label = mode === "edit" ? (t("common.editChapterName") ?? "Edit Chapter name") : (t("common.deleteChapter") ?? "Delete Chapter")
  useEffect(() => {
    incrementOpenChapterActionMenuCount(1)
    return () => {
      incrementOpenChapterActionMenuCount(-1)
    }
  }, [])
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape") return
      e.stopImmediatePropagation()
      onClose()
    }
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
  }, [onClose])
  return (
    <>
      <div
        style={{ position: "fixed", inset: 0, zIndex: zIndexBase }}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
        }}
      />
      <PopupMenu
        ref={setMenuEl}
        style={{ position: "fixed", top: pos.top, left: pos.left, zIndex: zIndexBase + 1, maxHeight: 260, overflowY: "auto" }}
        mainLabel={{ icon, text: label }}
      >
        {chapters.map((entry) => {
          const isPreset = mode === "edit" && isReservedTocIdentifier(entry.name)
          return (
            <PopupMenuItem
              key={entry.page}
              disabled={isPreset}
              onClick={(e) => {
                e.stopPropagation()
                onPick(entry)
                onClose()
              }}
            >
              <i className={`fas ${icon}`} style={{ width: 18 }}></i> {displayTocName(entry.name, t)}
            </PopupMenuItem>
          )
        })}
      </PopupMenu>
    </>
  )
}
