import type { CSSProperties, ReactNode } from "react"

import type { ChipColor } from "./activityColors"

export const CHIP_REMOVE_BUTTON_STYLE: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  width: 18,
  height: 18,
  padding: 0,
  border: "none",
  borderRadius: "50%",
  background: "rgba(0,0,0,0.25)",
  color: "inherit",
  cursor: "pointer",
  fontSize: "0.8em",
  lineHeight: 1,
}

/** Colored, fully-rounded pill badge (MUI `Chip` filled-color style). */
export function ActivityChip({
  color,
  children,
  onRemove,
  removeLabel,
  removeSlot,
}: {
  color: ChipColor
  children: ReactNode
  /** Mutually exclusive with `removeSlot` — only one remove control at a time. */
  onRemove?: () => void
  removeLabel?: string
  removeSlot?: ReactNode
}) {
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
        height: 24,
        padding: onRemove || removeSlot ? "0 6px 0 12px" : "0 12px",
        borderRadius: 16,
        fontSize: "0.6875rem",
        fontWeight: 400,
        whiteSpace: "nowrap",
        background: color.bg,
        color: color.text,
      }}
    >
      <span style={{ lineHeight: "normal" }}>{children}</span>
      {removeSlot}
      {onRemove && (
        <button type="button" onClick={onRemove} aria-label={removeLabel} style={CHIP_REMOVE_BUTTON_STYLE}>
          <i className="fa fa-times"></i>
        </button>
      )}
    </span>
  )
}
