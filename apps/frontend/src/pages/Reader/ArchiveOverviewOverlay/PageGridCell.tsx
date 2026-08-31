import type { MouseEvent, ReactNode } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Tooltip } from "@/components/common-ui/Display"

import { QuickAddTocPopover } from "./shared"

/** One page thumbnail in the overview grid — shows a spin icon while its `<img>` hasn't loaded yet.
 * The native `<img>` `onLoad` event is the ready signal (no job-progress polling needed here). */

// Sized to a 1:√2 (A4/B4) page ratio — without it, an unloaded cell collapses to the bare
// `min-width: 100px` CSS hint, visibly narrower than its loaded siblings.
const PLACEHOLDER_WIDTH_PX = 205

function OverviewThumbnail({ src, alt }: { src: string; alt: string | undefined }) {
  const [loaded, setLoaded] = useState(false)
  return (
    <div style={{ height: "100%", display: "flex", justifyContent: "center", ...(loaded ? undefined : { width: PLACEHOLDER_WIDTH_PX }) }}>
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
        style={{ display: "block", width: "auto", maxWidth: "100%", height: "100%", maxHeight: "none", objectFit: "contain", ...(loaded ? undefined : { visibility: "hidden" }) }}
        onLoad={() => setLoaded(true)}
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
}: {
  page: number
  isStamped: boolean
  loggedIn: boolean
  isTank?: boolean
  /** Briefly true right after the overlay opens, for whichever page it opened on — rendered as a
   * pulsing accent outline to draw the eye across a grid of otherwise-identical cells. */
  highlighted: boolean
  thumbnailSrc: string
  onSelectPage: (page: number) => void
  onSetThumbnail: (e: MouseEvent, page: number) => void
  onAddToc: (e: MouseEvent, page: number) => void
  onQuickAddToc: (page: number, title: string) => void
  onOpenLightbox: (page: number) => void
  /** True when this page came from a `.patch.zip` — gets a distinct green-tinted background. */
  isPatch?: boolean
}) {
  const { t } = useTranslation()
  const [hovered, setHovered] = useState(false)
  const [quickAddAt, setQuickAddAt] = useState<DOMRect | null>(null)
  return (
    <div
      // Not `className="id1"` — that class's `min-height: 335px` is tuned for the library grid's
      // taller `ArchiveCard`, not this shorter thumbnail-only cell.
      data-page-cell={page}
      style={{ display: "inline-block", cursor: "pointer" }}
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
        <OverviewThumbnail src={thumbnailSrc} alt={t("common.pageN", { n: page }) ?? undefined} />
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
        {!isTank && (
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
        {loggedIn && (
          <>
            <PageGridActionIcon
              icon="fa-file-image"
              corner="top-right"
              title={t("reader.setThisPageAsThumbnail") ?? undefined}
              hovered={hovered}
              onClick={(e) => onSetThumbnail(e, page)}
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
                onClick={(e) => onAddToc(e, page)}
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
