import { Select as BaseSelect } from "@base-ui/react/select"
import type { ReactNode } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, Z_OVERLAY_CONTENT } from "@/theme"

/** Site-wide `.favtag-btn`-styled select, built on Base UI's `Select` rather than a native
 * `<select>` — the trigger renders as a real `<button>` (see `Button.tsx`'s own docs for why that
 * matters for visual parity with `.stdbtn` buttons placed next to it), and the popup list is
 * styled to match legacy's own real `.context-menu-list`/`.context-menu-item`/`.context-menu-
 * item.context-menu-hover` classes via `useMenuPalette()` (the same source `PopupMenu.tsx` already
 * uses for the right-click/gear menus) rather than introducing new colors. */
export function Select<Value extends string>({
  value,
  onValueChange,
  items,
  ariaLabel,
  variant = "favtag-btn",
  style,
}: {
  value: Value
  onValueChange: (value: Value) => void
  items: { value: Value; label: ReactNode }[]
  ariaLabel?: string
  /** Matches `Button`'s own `variant` — pick `"stdbtn"` when this select sits next to a `.stdbtn`
   * button and needs to match its height/vertical position exactly (legacy's own `.favtag-btn`
   * and `.stdbtn` are different real sizes in every theme, e.g. 25px vs 21px tall in `g.css` —
   * not a bug introduced here, just two classes legacy itself never intended side by side). */
  variant?: "stdbtn" | "favtag-btn"
  style?: React.CSSProperties
}) {
  const palette = useMenuPalette()
  return (
    <BaseSelect.Root items={items} value={value} onValueChange={(v) => onValueChange(v as Value)}>
      {/* Base UI's `Select.Trigger` is unstyled — it has no built-in "value on the left, arrow
          pinned to the far right" slot layout the way a native `<select>`'s own platform chrome
          does, so that has to be built by hand: `justifyContent: "space-between"` pushes
          `Select.Icon` to the trigger's own right edge instead of it sitting flush against
          `Select.Value`'s text (the default with only `display: inline-flex` + `gap`). */}
      <BaseSelect.Trigger
        className={variant}
        aria-label={ariaLabel}
        style={{ display: "inline-flex", alignItems: "center", justifyContent: "space-between", gap: 4, ...style }}
      >
        <BaseSelect.Value />
        <BaseSelect.Icon>
          <i className="fa fa-caret-down" aria-hidden="true"></i>
        </BaseSelect.Icon>
      </BaseSelect.Trigger>
      <BaseSelect.Portal>
        <BaseSelect.Positioner sideOffset={4} className="outline-none" style={{ zIndex: Z_OVERLAY_CONTENT }}>
          <BaseSelect.Popup
            className={`thin-scrollbar rounded-[.2em] py-[.25em] ${FLOATING_POPUP_TRANSITION_CLASSES}`}
            style={{
              background: palette.bg,
              border: `1px solid ${palette.border}`,
              boxShadow: FLOATING_POPUP_SHADOW,
              color: palette.text,
              transformOrigin: "var(--transform-origin)",
            }}
          >
            <BaseSelect.List>
              {items.map((item) => (
                <SelectItem key={item.value} value={item.value}>
                  {item.label}
                </SelectItem>
              ))}
            </BaseSelect.List>
          </BaseSelect.Popup>
        </BaseSelect.Positioner>
      </BaseSelect.Portal>
    </BaseSelect.Root>
  )
}

function SelectItem({ value, children }: { value: string; children: ReactNode }) {
  const palette = useMenuPalette()
  return (
    <BaseSelect.Item
      value={value}
      className="relative box-content cursor-pointer select-none whitespace-nowrap px-4 py-[.3em] outline-none"
      // `highlighted` is Base UI's own keyboard/pointer-hover state (its `data-highlighted`
      // attribute, surfaced here through `style`-as-a-function-of-state instead) — the hover color
      // itself is per-theme data (`palette.hoverBg`/`hoverText`), not a static Tailwind value, so
      // it can't be a Tailwind arbitrary-variant class the way `PopupMenuItem`'s onMouseEnter/Leave
      // pair achieves the same effect for its own hand-rolled (non-Base-UI) menu.
      style={(state) =>
        state.highlighted ? { background: palette.hoverBg, color: palette.hoverText } : { background: "transparent", color: palette.text }
      }
    >
      <BaseSelect.ItemText>{children}</BaseSelect.ItemText>
    </BaseSelect.Item>
  )
}
