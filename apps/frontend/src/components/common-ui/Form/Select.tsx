import { Select as BaseSelect } from "@base-ui/react/select"
import type { ComponentProps, ReactNode } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, Z_OVERLAY_CONTENT } from "@/theme"

/** Site-wide `.favtag-btn`-styled select, built on Base UI's `Select` rather than a native
 * `<select>` — the trigger renders as a real `<button>` (see `Button.tsx`'s own docs for why that
 * matters for visual parity with `.stdbtn` buttons placed next to it), and the popup list is
 * styled to match legacy's own real `.context-menu-list`/`.context-menu-item`/`.context-menu-
 * item.context-menu-hover` classes via `useMenuPalette()` (the same source `PopupMenu.tsx` already
 * uses for the right-click/gear menus) rather than introducing new colors.
 *
 * `...rootProps` forwards the rest of `Select.Root`'s own real props (`disabled`, `name`,
 * `required`, `readOnly`, `form`, `modal`, ...) straight through — same "wrap, don't narrow" shape
 * as `Button.tsx`'s own `Omit<ComponentProps<typeof BaseButton>, "className">`, so a caller never
 * hits a missing prop this wrapper didn't happen to anticipate. Only the handful this component
 * gives its own opinionated name/behavior to (`value`/`onValueChange`/`items`, all still real
 * `Select.Root` props under the hood — `onValueChange`'s own `(value, eventDetails)` signature is
 * narrowed to `(value)` since no call site here has needed `eventDetails` yet) are pulled out of
 * that union explicitly. */
export function Select<Value extends string>({
  value,
  onValueChange,
  items,
  ariaLabel,
  variant = "favtag-btn",
  showItemIndicator = true,
  style,
  ...rootProps
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
  /** Whether each popup item reserves a leading column for a checkmark on the selected item.
   * Defaults on (Base UI's own recommended anatomy includes `Select.ItemIndicator`) — set `false`
   * for a list where `data-highlighted`'s hover color already reads as "this one" clearly enough
   * on its own (e.g. legacy's own `.context-menu-item` popups never had a persistent checkmark
   * either) and the extra indented column would just be visual noise. */
  showItemIndicator?: boolean
  style?: React.CSSProperties
} & Omit<ComponentProps<typeof BaseSelect.Root>, "value" | "onValueChange" | "items" | "children">) {
  const palette = useMenuPalette()
  return (
    <BaseSelect.Root items={items} value={value} onValueChange={(v) => onValueChange(v as Value)} {...rootProps}>
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
            // `text-left` here (not on each `SelectItem`) is load-bearing, not decorative —
            // legacy's own `body { text-align: center }` (`lrr.css`) is inherited all the way
            // down through `Select.Portal`'s `document.body` mount point, so anything rendered
            // inside this popup renders centered instead of left-aligned without a reset
            // somewhere in the chain — confirmed live, 2026-08-27, by walking the actual computed
            // `text-align` from a popup item up to `<body>`. Set once here so every current and
            // future child (`SelectItem`, a future `Select.GroupLabel`, etc.) inherits the correct
            // value instead of each one needing to remember this trap independently.
            className={`thin-scrollbar rounded-[.2em] py-[.25em] text-left ${FLOATING_POPUP_TRANSITION_CLASSES}`}
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
                <SelectItem key={item.value} value={item.value} showIndicator={showItemIndicator}>
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

function SelectItem({ value, showIndicator, children }: { value: string; showIndicator: boolean; children: ReactNode }) {
  return (
    <BaseSelect.Item
      value={value}
      // `.select-item-highlighted[data-highlighted]` (one real rule per theme file, see
      // `public/legacy/themes/*.css`) drives the hover color directly off Base UI's own
      // `data-highlighted` state attribute — the base-ui.com styling handbook's own recommended
      // pattern (a static `data-[attr]:class` selector reading the library's state attributes
      // directly), preferred here over a `style`-as-a-function-of-state prop (this file's own
      // earlier approach) now that the color has a real theme-CSS home to live in, same as
      // `ActivityCombobox.tsx`'s own `.activity-combobox-item-highlighted[data-highlighted]`.
      //
      // `grid grid-cols-[1em_1fr]` (base-ui.com's own recommended pattern for this) reserves the
      // indicator's column width on every item, not just the selected one — `ItemIndicator` only
      // actually renders an element for the selected item (it isn't `keepMounted`), so without a
      // column reserved up front, every *unselected* item's text started flush left while the
      // selected item's text alone got pushed right by the indicator + gap, an obviously
      // unintentional-looking jump confirmed live from a real screenshot, 2026-08-27. (Text
      // alignment itself is reset once on `Select.Popup` above, not repeated per item — see that
      // element's own `text-left` docs.)
      className={`select-item-highlighted relative box-content w-full cursor-pointer items-center gap-2 select-none whitespace-nowrap py-[.3em] pe-4 outline-none ${
        showIndicator ? "grid grid-cols-[1em_1fr] ps-2" : "flex ps-4"
      }`}
    >
      {showIndicator && (
        // Left-aligned within its own column (no `justify-content: center`) and sized down to
        // `text-xs` — base-ui.com's own official example pairs `ItemIndicator` with a thin SVG
        // `CheckIcon` that reads visually smaller/tighter against the text than Font Awesome's
        // `fa-check` glyph does at its default (inherited, ~text-sm) size; both together (no
        // centering + a smaller glyph) is what actually closes the gap against the official
        // example's tighter look (real user feedback comparing screenshots, 2026-08-27).
        <BaseSelect.ItemIndicator className="col-start-1 flex text-xs">
          <i className="fa fa-check" aria-hidden="true"></i>
        </BaseSelect.ItemIndicator>
      )}
      <BaseSelect.ItemText className={showIndicator ? "col-start-2" : undefined}>{children}</BaseSelect.ItemText>
    </BaseSelect.Item>
  )
}
