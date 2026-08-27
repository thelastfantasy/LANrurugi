import type { ReactNode } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FONT_SIZE_MD, Z_OVERLAY_TOOLTIP } from "@/theme"

/** Which side of the anchor point the bubble opens towards, and how it's aligned along that
 * side — the same two-axis vocabulary Base UI's own `Popover.Positioner` uses (`side`/`align`),
 * kept here as plain string literals since this component doesn't actually depend on Base UI at
 * all (see below for why). */
export type TooltipSide = "top" | "bottom" | "left" | "right"

export type TooltipAlign = "start" | "center" | "end"

const GAP = 8
/** Half of `.marker`'s own real 24x24px box (`lrr.css`) — the anchor point this component is
 * given (e.g. a stamp's `iconPos`, in the same `%`-of-image coordinate space `.marker` itself
 * uses) is the icon's *center*, so the bubble needs to clear the icon's own radius before `GAP`
 * even starts, on whichever side it opens towards. */
const ICON_RADIUS_PX = 12

/** An always-visible tooltip anchored to a `%`-of-container point rather than a real DOM
 * element/`Popover.Trigger` — for a hover-less touch device, where there's no `mouseenter` to
 * ever open a hover-triggered tooltip (`Tooltip.tsx`) in the first place, but the label still
 * needs to be visible (e.g. a stamp's content on mobile). Deliberately plain CSS (`position:
 * absolute` + `transform`) instead of Base UI `Popover`/Floating UI: every caller here already
 * knows its own anchor's exact position as a percentage of an already-`position: relative`
 * container (`MarkerLayer.tsx`'s `wrapperRef`, the same coordinate space `.marker` itself is
 * placed in) and which direction is meant to stay clear of a same-anchor rect selection outline
 * (`stampTooltipPlacement`'s anchor-outward mapping) — there's no real DOM element whose on-screen
 * box needs measuring, which is the entire problem Floating UI's anchor positioning exists to
 * solve. Must be rendered as a sibling inside that same `%`-coordinate container, not via a portal
 * — unlike `Tooltip.tsx`, this one relies on inheriting its anchor's own positioning context. */
export function StaticTooltip({
  xPercent,
  yPercent,
  side,
  align,
  label,
}: {
  /** The anchor point's own position, in the same `%`-of-container space `.marker`'s `left`/`top`
   * already use (i.e. `iconPos`, not the container's own pixel box) — the icon's center, not one
   * of its corners. */
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

