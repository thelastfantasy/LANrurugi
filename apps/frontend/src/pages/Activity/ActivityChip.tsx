import type { CSSProperties, ReactNode } from "react"

import type { ChipColor } from "./activityColors"

/** Shared by both the standalone remove button ({@link ActivityChip}'s own `onRemove` prop) and
 * `ActivityCombobox.tsx`'s `Combobox.ChipRemove` (which renders this same look via its `render`
 * prop instead of duplicating the style object a second time) — a solid dark circular hit target
 * sitting inside the chip's own colored background, matching MUI `Chip`'s real delete-icon
 * treatment (a filled, not just outlined/opacity-dimmed, circle). */
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

/** A real colored, fully-rounded pill badge — MUI `Chip`'s own filled-color visual (solid
 * saturated background, fully rounded ends, compact ~24px height, regular — not bold — weight)
 * is the concrete reference this matches (`border-radius: 16px` at a 24px chip height is MUI's
 * own real value, not an arbitrary round number — half the height, which is what makes both ends
 * fully semicircular rather than merely "very rounded"). `display: inline-flex` so it never claims
 * a full row/line on its own and several sit naturally side by side (e.g. a filter's row of
 * selected chips, or a table cell that shows both an action-type chip and an actor chip together). */
export function ActivityChip({
  color,
  children,
  onRemove,
  removeLabel,
  removeSlot,
}: {
  color: ChipColor
  children: ReactNode
  /** Present only for a removable chip that manages its own click handling directly (row/detail-
   * panel usage has no such case today, but this stays the simple path for any future one) —
   * renders a `CHIP_REMOVE_BUTTON_STYLE`-styled `<button onClick={onRemove}>`. Mutually exclusive
   * with `removeSlot` below (a caller needs one or the other, never both). */
  onRemove?: () => void
  removeLabel?: string
  /** Present when the caller already has its own remove control with its own click/keyboard
   * handling to compose in (`ActivityCombobox.tsx`'s `Combobox.ChipRemove`, whose own logic must
   * stay in charge of the click) — rendered as-is instead of wrapping it in a second button. */
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
      {/* Own inline-block wrapper for the label text specifically, with an explicit `lineHeight:
          "normal"` — centering the *outer* flex container's `line-height` (an earlier version)
          only indirectly nudges where text sits within it; measured at the sub-pixel level as
          already centered, but still visibly inconsistent chip-to-chip depending on background
          color/font hinting. Sizing this wrapper to its own natural line box and centering that as
          a whole flex item is a more direct, class-of-fix-independent way to keep every chip's
          text vertically consistent regardless of which color it happens to render on. */}
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
