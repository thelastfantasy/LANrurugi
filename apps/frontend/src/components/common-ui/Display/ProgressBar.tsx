import type { CSSProperties } from "react"

import { FONT_SIZE_XS } from "@/theme"

const BAR_HEIGHT = 20
const BAR_BACKGROUND = "rgba(128,128,128,0.25)"

/** A full-width bar with its reading inside the track, split at the fill boundary: theme text colour
 * over the empty track, white over the fill. `ratio: null` shows an indeterminate sliding block
 * (rendered faint so the text stays readable over it). */
export function ProgressBar({
  ratio,
  label,
  color = "var(--job-active-color, rgb(26, 165, 26))",
  flowing = false,
}: {
  ratio: number | null
  label: string
  /** Fill colour; defaults to the per-theme job-active green. */
  color?: string
  /** Animate the fill while the transfer is actually running; a paused/finished row must stay still. */
  flowing?: boolean
}) {
  const pct = ratio == null ? 0 : Math.max(0, Math.min(1, ratio)) * 100
  const textStyle: CSSProperties = {
    position: "absolute",
    inset: 0,
    display: "flex",
    alignItems: "center",
    // Right-aligned, not centered: the fill grows from the left, so a centred reading is cut by the
    // fill boundary through most of the transfer, while a right-aligned one only meets it late.
    // Numbers also read naturally right-aligned.
    justifyContent: "flex-end",
    padding: "0 10px",
    boxSizing: "border-box",
    fontSize: FONT_SIZE_XS,
    lineHeight: 1,
    whiteSpace: "nowrap",
    overflow: "hidden",
    fontVariantNumeric: "tabular-nums",
  }
  return (
    <div
      style={{
        position: "relative",
        width: "100%",
        height: BAR_HEIGHT,
        background: BAR_BACKGROUND,
        borderRadius: 4,
        overflow: "hidden",
      }}
    >
      {ratio == null ? (
        <div
          style={{
            position: "absolute",
            top: 0,
            bottom: 0,
            left: 0,
            width: "30%",
            background: color,
            opacity: 0.35,
            borderRadius: 4,
            animation: "lrr-indeterminate-bar 1.2s ease-in-out infinite",
          }}
        />
      ) : (
        <div
          style={{
            position: "absolute",
            top: 0,
            bottom: 0,
            left: 0,
            width: `${pct}%`,
            backgroundColor: color,
            // Clips the stripe layer, which is deliberately one tile wider than the fill.
            overflow: "hidden",
          }}
        >
          {flowing ? <div className="lrr-flowing-fill" /> : null}
        </div>
      )}
      {ratio == null ? (
        <span style={textStyle}>{label}</span>
      ) : (
        <>
          <span style={{ ...textStyle, clipPath: `inset(0 0 0 ${pct}%)` }}>{label}</span>
          <span
            aria-hidden="true"
            style={{
              ...textStyle,
              color: "#fff",
              clipPath: `inset(0 ${100 - pct}% 0 0)`,
            }}
          >
            {label}
          </span>
        </>
      )}
    </div>
  )
}
