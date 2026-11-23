import type { MouseEvent, ReactNode } from "react"
import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { BottomSheet, SheetGrid, SheetTile, Tooltip } from "@/components/common-ui/Display"
import { useLongPress } from "@/hooks/useLongPress"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { displayTocName, TOC_CHAPTER_COUNT, tocChapterIdentifier } from "@/lib/utils/tocValidation"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"

import { QuickAddTocPopover, useQuickTocPresets } from "./shared"

/** The sheet's two longest action labels get a row of their own — half a row is too narrow for them,
 * and the tiles are single-line buttons (`SheetTile` truncates rather than wraps). The icon stays
 * pinned to the left edge; the label centres in what is left of the row. */
const FULL_ROW = { gridColumn: "1 / -1", display: "flex", alignItems: "center" }
const FULL_ROW_LABEL = {
  flex: "1 1 auto",
  minWidth: 0,
  textAlign: "center" as const,
  overflow: "hidden",
  textOverflow: "ellipsis",
}

/** One page thumbnail in the overview grid — shows a spin icon while its `<img>` hasn't loaded yet.
 * The native `<img>` `onLoad` event is the ready signal (no job-progress polling needed here). */

/** On touch screens a row is at most this many times as tall as it is wide (see the overview's
 * row fitting); a page longer than that shows its top, cropped to the cell's width. */
export const MAX_ROW_ASPECT = 1.5

function OverviewThumbnail({ src, alt, cropLong }: { src: string; alt: string | undefined; cropLong: boolean }) {
  const [loaded, setLoaded] = useState(false)
  const [long, setLong] = useState(false)
  const crop = cropLong && long
  return (
    <div style={{ height: "100%", width: "100%", display: "flex", justifyContent: "center" }}>
      {!loaded && (
        <span
          style={{
            position: "absolute",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
          }}
        >
          <i className="fa fa-4x fa-circle-notch fa-spin" aria-hidden="true"></i>
        </span>
      )}
      <img
        loading="lazy"
        alt={alt}
        src={src}
        // `objectPosition` overrides legacy's `div.id3:not(.nocrop) img { object-position: left top }`,
        // which pinned every letterboxed thumbnail to the cell's corner.
        style={{ display: "block", width: "100%", maxWidth: "none", height: "100%", maxHeight: "none", objectFit: crop ? "cover" : "contain", objectPosition: crop ? "center top" : "center", ...(loaded ? undefined : { visibility: "hidden" }) }}
        onLoad={(e) => {
          const img = e.currentTarget
          setLong(img.naturalWidth > 0 && img.naturalHeight / img.naturalWidth > MAX_ROW_ASPECT)
          setLoaded(true)
        }}
      />
    </div>
  )
}

/** The "第 N 页" label shown over a page-grid cell. Genuinely centered (`left: 50%` +
 * `translateX(-50%)`), not legacy's own `left: 30%` (an artifact of untracked text width). */
function PageNumberLabel({ children, hovered }: { children: ReactNode; hovered: boolean }) {
  return (
    <span
      className="page-number"
      style={{
        left: "50%",
        transform: "translateX(-50%)",
        backgroundColor: "rgba(0,0,0,0.5)",
        borderRadius: 4,
        whiteSpace: "nowrap",
        opacity: hovered ? 1 : 0,
        zIndex: hovered ? 300 : 0,
        transition: "opacity 0.1s",
      }}
    >
      {children}
    </span>
  )
}

/** One of the two hover-revealed action buttons in a page-grid cell. */
function PageGridActionIcon({
  icon,
  corner,
  title,
  hovered,
  onClick,
  onContextMenu,
}: {
  icon: string
  /** `'top-right'`/`'bottom-right'` mirror legacy's two icons; `'bottom-left'` is purely additive. */
  corner: "top-right" | "bottom-right" | "bottom-left"
  title: string | undefined
  /** Lifted to the parent cell — at rest this icon sits behind the thumbnail `<img>` (`z-index:
   * -1`), so the pointer never reaches it to fire its own hover. */
  hovered: boolean
  onClick: (e: MouseEvent) => void
  /** Only the "add chapter" icon supplies this — additive, no legacy equivalent. */
  onContextMenu?: (e: MouseEvent) => void
}) {
  const vertical = corner === "top-right" ? "top" : "bottom"
  const horizontal = corner === "bottom-left" ? "left" : "right"
  return (
    <a
      href="#"
      title={title}
      className={`fas ${icon}`}
      onClick={onClick}
      onContextMenu={onContextMenu}
      style={{
        position: "absolute",
        [vertical]: "0",
        [horizontal]: "0",
        padding: 12,
        fontSize: 20,
        color: "lightskyblue",
        // `opacity: 0`, not `z-index: -1` — a `contain`-fit image doesn't fully cover the cell,
        // so a negative z-index alone would leave the icon visibly floating over empty background.
        opacity: hovered ? 1 : 0,
        pointerEvents: hovered ? "auto" : "none",
        zIndex: hovered ? 300 : 0,
        backgroundColor: hovered ? "rgba(0,0,0,0.5)" : undefined,
        borderRadius: 4,
        transition: "opacity 0.1s",
      }}
    />
  )
}

/** One cell in the page-grid — holds the hover state both `PageGridActionIcon`s need. */
export function PageGridCell({
  page,
  isStamped,
  loggedIn,
  highlighted,
  thumbnailSrc,
  onSelectPage,
  onSetThumbnail,
  onAddToc,
  onQuickAddToc,
  onOpenLightbox,
  isTank,
  isPatch = false,
  chapterName,
  toc = [],
  cornerButtons = true,
}: {
  /** False on touch screens, where a long press opens the same actions as a sheet. */
  cornerButtons?: boolean
  page: number
  isStamped: boolean
  loggedIn: boolean
  isTank?: boolean
  /** Briefly true right after the overlay opens, for whichever page it opened on — rendered as a
   * pulsing accent outline to draw the eye across a grid of otherwise-identical cells. */
  highlighted: boolean
  thumbnailSrc: string
  onSelectPage: (page: number) => void
  onSetThumbnail: (page: number) => void
  onAddToc: (page: number) => void
  onQuickAddToc: (page: number, title: string) => void
  onOpenLightbox: (page: number) => void
  /** True when this page came from a `.patch.zip` — gets a distinct green-tinted background. */
  isPatch?: boolean
  /** Set only when this page starts a chapter — painted top-left, always visible (unlike the
   * hover-only page-number label and action icons, a chapter start is worth seeing at a glance). */
  chapterName?: string
  /** The archive's whole ToC, so the phone sheet's quick-add buttons can say where an entry each
   * button would add already sits. Pages are whatever the grid shows (global in Tankoubon mode). */
  toc?: { page: number; name: string }[]
}) {
  const { t } = useTranslation()
  const [hovered, setHovered] = useState(false)
  const [quickAddAt, setQuickAddAt] = useState<DOMRect | null>(null)
  // A touch screen has no hover to reveal the corner buttons, so a long press opens the same actions
  // as a sheet. Mounted only while in use: the grid can hold hundreds of cells.
  const [cellEl, setCellEl] = useState<HTMLDivElement | null>(null)
  const [sheetMounted, setSheetMounted] = useState(false)
  const [sheetOpen, setSheetOpen] = useState(false)
  // The bottom-right button's right-click presets, as a sheet over the first one (the library's
  // "add to category" works the same way).
  const [presetSheetOpen, setPresetSheetOpen] = useState(false)
  const presets = useQuickTocPresets()
  const palette = useMenuPalette()
  const afterSheet = useRef<(() => void) | null>(null)
  useLongPress(
    cellEl,
    () => {
      setSheetMounted(true)
      requestAnimationFrame(() => setSheetOpen(true))
    },
    { blockContextMenu: () => true },
  )
  /** Closes the sheet, then runs `action` once it has slid away. */
  const closeSheetThen = (action: () => void) => () => {
    afterSheet.current = action
    setSheetOpen(false)
  }
  const hasActions = !isTank || loggedIn
  /** The sheets below are portaled into `body`, but React still bubbles their clicks through
   * the component tree — without this, tapping the backdrop or an item also hits the cell's
   * `onClick` (select that page, closing the overview). */
  const stopCellClick = (e: MouseEvent) => e.stopPropagation()
  return (
    <div
      ref={hasActions ? setCellEl : undefined}
      // Not `className="id1"` — that class's `min-height: 335px` is tuned for the library grid's
      // taller `ArchiveCard`, not this shorter thumbnail-only cell.
      data-page-cell={page}
      className="overview-page-cell"
      style={{ cursor: "pointer" }}
      onClick={() => onSelectPage(page)}
    >
      <div
        className={`id3 quick-thumbnail${isPatch ? " patch" : ""}`}
        data-stamped={isStamped || undefined}
        style={{
          position: "relative",
          ...(highlighted && {
            outline: "3px solid #3b97ea",
            outlineOffset: 2,
            animation: "lrr-overview-highlight-pulse 0.6s ease-in-out 4",
          }),
        }}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <PageNumberLabel hovered={hovered}>{t("common.pageN", { n: page })}</PageNumberLabel>
        {chapterName && (
          <span
            title={chapterName}
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              maxWidth: "100%",
              padding: "2px 6px",
              fontSize: 13,
              lineHeight: 1.4,
              // Same treatment as the page-number label and patch badge: a dark chip over the
              // thumbnail, so it stays legible on every theme without its own theme rule.
              backgroundColor: "rgba(0,0,0,0.6)",
              color: "lightskyblue",
              borderRadius: "0 0 4px 0",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
              pointerEvents: "none",
              zIndex: 2,
            }}
          >
            {chapterName}
          </span>
        )}
        <OverviewThumbnail src={thumbnailSrc} alt={t("common.pageN", { n: page }) ?? undefined} cropLong={!cornerButtons} />
        {isPatch && (
          <Tooltip label={t("reader.patchPage")} wrapperStyle={{ position: "static" }}>
            <span
              style={{
                position: "absolute",
                bottom: 8,
                left: "50%",
                transform: "translateX(-50%)",
                fontSize: 14,
                backgroundColor: "rgba(0,0,0,0.5)",
                color: "lightskyblue",
                padding: "0.5rem",
                borderRadius: 4,
                pointerEvents: "auto",
                cursor: "default",
                zIndex: 2,
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
              }}
            >
              <i className="fa fa-circle-plus" aria-hidden="true" />
            </span>
          </Tooltip>
        )}
        {cornerButtons && !isTank && (
        <PageGridActionIcon
          icon="fa-magnifying-glass"
          corner="bottom-left"
          title={t("reader.viewFullPage") ?? undefined}
          hovered={hovered}
          onClick={(e) => {
            e.preventDefault()
            e.stopPropagation()
            onOpenLightbox(page)
          }}
        />
        )}
        {cornerButtons && loggedIn && (
          <>
            <PageGridActionIcon
              icon="fa-file-image"
              corner="top-right"
              title={t("reader.setThisPageAsThumbnail") ?? undefined}
              hovered={hovered}
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                onSetThumbnail(page)
              }}
            />
            <Tooltip
              label={t("reader.addChapterAtThisPage") + " " + t("reader.rightclickForQuickPresets")}
              wrapperStyle={{ position: "static" }}
              anchor="cursor"
            >
              <PageGridActionIcon
                icon="fa-book-medical"
                corner="bottom-right"
                title={undefined}
                hovered={hovered}
                onClick={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  onAddToc(page)
                }}
                onContextMenu={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  setQuickAddAt(e.currentTarget.getBoundingClientRect())
                }}
              />
            </Tooltip>
          </>
        )}
      </div>
      {sheetMounted && (
        <div style={{ display: "contents" }} onClick={stopCellClick} onContextMenu={stopCellClick}>
          <BottomSheet
            open={sheetOpen}
            onClose={() => setSheetOpen(false)}
            onExited={() => {
              setSheetMounted(false)
              const action = afterSheet.current
              afterSheet.current = null
              action?.()
            }}
            cancelLabel={t("common.cancel")}
            // Above the overview itself, which sits on legacy's 9000 overlay layer.
            zIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
          >
            <div style={{ padding: "6px 0 6px", fontWeight: 700, fontSize: 16 }}>
              {t("common.pageN", { n: page })}
            </div>
            {/* One tile per corner button; the bottom-right one does two things (click, right-click). */}
            <SheetGrid>
              {loggedIn && (
                <SheetTile
                  icon="fa-file-image"
                  style={FULL_ROW}
                  onClick={closeSheetThen(() => onSetThumbnail(page))}
                >
                  <span style={FULL_ROW_LABEL}>{t("reader.setThisPageAsThumbnail")}</span>
                </SheetTile>
              )}
              {!isTank && (
                <SheetTile
                  icon="fa-magnifying-glass"
                  style={FULL_ROW}
                  onClick={closeSheetThen(() => onOpenLightbox(page))}
                >
                  <span style={FULL_ROW_LABEL}>{t("reader.viewFullPage")}</span>
                </SheetTile>
              )}
              {loggedIn && (
                <>
                  <SheetTile icon="fa-book-medical" onClick={closeSheetThen(() => onAddToc(page))}>
                    {t("reader.addChapterAtThisPage")}
                  </SheetTile>
                  <SheetTile icon="fa-bolt" onClick={() => setPresetSheetOpen(true)}>
                    {t("reader.quickAddChapter")}
                  </SheetTile>
                </>
              )}
            </SheetGrid>
            {loggedIn && (
              <BottomSheet
                open={presetSheetOpen}
                onClose={() => setPresetSheetOpen(false)}
                cancelLabel={t("common.cancel")}
                zIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL + 10}
              >
                <div style={{ padding: "6px 0 6px", fontWeight: 700, fontSize: 16 }}>
                  {t("reader.quickAddChapter")}
                </div>
                {(() => {
                  // Picking one adds it and closes both sheets.
                  const pick = (title: string) => {
                    setPresetSheetOpen(false)
                    closeSheetThen(() => onQuickAddToc(page, title))()
                  }
                  // Where an entry this button would add already sits, if it was set before. Matches
                  // the stored identifier (`c3`, `toc`) and its visible label, because older quick
                  // adds stored the label; the lowest page wins if it was set twice.
                  const pageOf = (identifier: string) => {
                    const wanted = [identifier.trim().toLowerCase(), displayTocName(identifier, t).trim().toLowerCase()]
                    let setAt: number | undefined
                    for (const entry of toc) {
                      if (!wanted.includes(entry.name.trim().toLowerCase())) continue
                      if (setAt === undefined || entry.page < setAt) setAt = entry.page
                    }
                    return setAt
                  }
                  const tile = (key: string | number, icon: string, label: string, identifier: string) => {
                    const setAt = pageOf(identifier)
                    return (
                      <SheetTile key={key} icon={icon} style={{ position: "relative" }} onClick={() => pick(identifier)}>
                        {label}
                        {/* Phone only in practice: this sheet is the long-press path. A bare page
                            number, so it reads as "already at 86" without another label. */}
                        {setAt !== undefined && (
                          <span
                            title={t("common.pageN", { n: setAt })}
                            style={{
                              position: "absolute",
                              right: 7,
                              bottom: 3,
                              fontSize: 11,
                              lineHeight: 1.1,
                              opacity: 0.55,
                              fontVariantNumeric: "tabular-nums",
                              pointerEvents: "none",
                            }}
                          >
                            {setAt}
                          </span>
                        )}
                      </SheetTile>
                    )
                  }
                  return (
                    <>
                      <SheetGrid>{presets.map((p) => tile(p.title, p.icon, p.title, p.value))}</SheetGrid>
                      <div style={{ height: 10 }} />
                      <SheetGrid separator={palette.separator}>
                        {Array.from({ length: TOC_CHAPTER_COUNT }, (_, i) => i + 1).map((n) =>
                          tile(n, "fa-book-medical", t("reader.chapterN", { n }), tocChapterIdentifier(n)),
                        )}
                      </SheetGrid>
                    </>
                  )
                })()}
              </BottomSheet>
            )}
          </BottomSheet>
        </div>
      )}
      {quickAddAt && (
        <QuickAddTocPopover
          anchor={quickAddAt}
          onPick={(title) => onQuickAddToc(page, title)}
          onClose={() => setQuickAddAt(null)}
        />
      )}
    </div>
  )
}
