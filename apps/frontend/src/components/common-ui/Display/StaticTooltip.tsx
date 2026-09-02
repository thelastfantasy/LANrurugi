import type { ReactNode } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FONT_SIZE_MD, Z_OVERLAY_TOOLTIP } from "@/theme"

/** Which side of the anchor point the bubble opens towards, and how it's aligned along that side. */
export type TooltipSide = "top" | "bottom" | "left" | "right"

export type TooltipAlign = "start" | "center" | "end"

const GAP = 8
/** Half of `.marker`'s 24x24px box — the anchor point is the icon's center, so the bubble must
 * clear this radius before `GAP` starts. */
const ICON_RADIUS_PX = 12

/** An always-visible tooltip anchored to a `%`-of-container point (for touch devices with no
 * hover). Must render as a sibling inside the same `%`-coordinate container, not via a portal. */
export function StaticTooltip({
  xPercent,
  yPercent,
  side,
  align,
  label,
}: {
  /** Anchor position in `%`-of-container space (`.marker`'s `left`/`top`), icon center. */
  xPercent: number
  yPercent: number
  side: TooltipSide
  align: TooltipAlign
  label: ReactNode
}) {
  const palette = useMenuPalette()

  let translateX: string
  let translateY: string
  if (side === "top" || side === "bottom") {
    translateY = side === "top" ? `calc(-100% - ${ICON_RADIUS_PX + GAP}px)` : `${ICON_RADIUS_PX + GAP}px`
    translateX = align === "start" ? "0%" : align === "end" ? "-100%" : "-50%"
  } else {
    translateX = side === "left" ? `calc(-100% - ${ICON_RADIUS_PX + GAP}px)` : `${ICON_RADIUS_PX + GAP}px`
    translateY = align === "start" ? "0%" : align === "end" ? "-100%" : "-50%"
  }

  return (
    <div
      style={{
        position: "absolute",
        left: `${xPercent}%`,
        top: `${yPercent}%`,
        transform: `translate(${translateX}, ${translateY})`,
        zIndex: Z_OVERLAY_TOOLTIP,
        pointerEvents: "none",
        background: palette.bg,
        border: `1px solid ${palette.border}`,
        boxShadow: FLOATING_POPUP_SHADOW,
        color: palette.text,
        borderRadius: 4,
        padding: "6px 10px",
        fontSize: FONT_SIZE_MD,
        lineHeight: 1.5,
        whiteSpace: "normal",
        maxWidth: 240,
        width: "max-content",
        textAlign: "left",
      }}
    >
      {label}
    </div>
  )
}

