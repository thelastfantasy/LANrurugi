import type { ComponentProps, ReactNode } from "react"

import { Menu, type MenuSide } from "@/components/common-ui/Display/Menu"

/** A `.stdbtn` action whose right-hand caret opens a menu, the two halves reading as one control.
 *
 * Two real `<button>`s rather than one, because the caret has to be the `Menu` trigger itself, and
 * `Menu` renders its trigger directly (no wrapper). The halves therefore only remove the border
 * they share — the left half's right one — so the caret's own left border is the single divider,
 * at whatever width the active theme gives `.stdbtn`; `.stdbtn`'s 1px side margins are zeroed on
 * the inner edges too, or the missing border would show up as a transparent sliver. */
export function SplitButton({
  label,
  onClick,
  disabled,
  caretDisabled = false,
  hideCaretWhenDisabled = false,
  menuLabel,
  caret = <i className="fa fa-caret-down" aria-hidden="true" />,
  menuSide = "bottom",
  menuAlign = "end",
  buttonStyle,
  className = "stdbtn",
  children,
}: {
  label: ReactNode
  onClick: () => void
  disabled?: boolean
  /** Disables just the caret: for "there is nothing to choose from right now" while the main
   * action stays live. Rendered disabled unless {@link hideCaretWhenDisabled} says otherwise. */
  caretDisabled?: boolean
  /** When the caret is disabled, drop it entirely and let the main button take back the full
   * outline. Off by default, so an unavailable caret still shows (and reads) as unavailable. */
  hideCaretWhenDisabled?: boolean
  /** Accessible name of the caret; the menu itself should carry visible headings. */
  menuLabel: string
  /** What the right half shows — any node, its own width decides the half's width. Defaults to a
   * caret icon. */
  caret?: ReactNode
  menuSide?: MenuSide
  menuAlign?: ComponentProps<typeof Menu>["align"]
  /** Applied to both halves, so they keep the caller's toolbar sizing/spacing. */
  buttonStyle?: React.CSSProperties
  className?: string
  children: ComponentProps<typeof Menu>["children"]
}) {
  const caretShown = !(caretDisabled && hideCaretWhenDisabled)
  return (
    <span style={{ display: "inline-flex" }}>
      <button
        type="button"
        className={className}
        style={{
          ...buttonStyle,
          ...(caretShown && {
            borderTopRightRadius: 0,
            borderBottomRightRadius: 0,
            borderRight: "none",
            marginRight: 0,
          }),
        }}
        disabled={disabled}
        onClick={onClick}
      >
        {label}
      </button>
      {caretShown && <Menu
        side={menuSide}
        align={menuAlign}
        trigger={
          <button
            type="button"
            className={className}
            style={{
              ...buttonStyle,
              borderTopLeftRadius: 0,
              borderBottomLeftRadius: 0,
              marginLeft: 0,
              // The half is sized by whatever `caret` renders, so the caller's padding would only
              // add empty space around it and make it read as a square button again.
              paddingLeft: 2,
              paddingRight: 2,
              // Legacy `.stdbtn` carries `min-width: 150px`, which would make the caret a wide bar.
              minWidth: 0,
              // Dim only the glyph, never the divider: the shared border belongs to the pair, so
              // it has to follow the main button's state, not this half's own availability.
              opacity: disabled ? 0.4 : 1,
            }}
            aria-label={menuLabel}
            title={menuLabel}
            disabled={disabled || caretDisabled}
          >
            <span style={{ opacity: caretDisabled && !disabled ? 0.4 : 1 }}>{caret}</span>
          </button>
        }
      >
        {children}
      </Menu>}
    </span>
  )
}
