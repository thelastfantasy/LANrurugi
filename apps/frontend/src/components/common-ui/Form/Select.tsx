import { Select as BaseSelect } from "@base-ui/react/select"
import type { ComponentProps, ReactNode } from "react"
import { FaCaretDown, FaCheck } from "react-icons/fa6"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, FONT_SIZE_MD, FONT_SIZE_SM, Z_OVERLAY_CONTENT } from "@/theme"

/** Chakra-UI-style size presets — `sm`/`md` reuse this codebase's own existing
 * `FONT_SIZE_SM`/`FONT_SIZE_MD` constants (`theme.ts`) rather than inventing new values, `lg` is
 * new (no existing `FONT_SIZE_LG` constant — `theme.ts`'s own three sizes are all legacy-`pt`-
 * derived small sizes, none in this larger range) sized for a select that's meant to stand out as
 * a page's primary control (e.g. Categories.tsx's own category picker) rather than sit inline with
 * body text. A fixed `height` per size (not just `fontSize`) keeps the trigger's own box from
 * reflowing to whatever height that font size happens to need on its own — every call site that
 * picks a size gets both dimensions handled together instead of needing a separate `style.height`
 * the way Categories.tsx's original hand-rolled `style={{ fontSize: 20, height: 30 }}` did. */
const SELECT_SIZES = {
  sm: { fontSize: FONT_SIZE_SM, height: 21 },
  md: { fontSize: FONT_SIZE_MD, height: 25 },
  lg: { fontSize: "1.25rem", height: 30 },
} as const

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
  size,
  ...rootProps
}: {
  value: Value
  onValueChange: (value: Value) => void
  /** `itemClassName`/`itemStyle` (optional, per-item) merge onto that one popup item's own
   * `className`/inline `style` — e.g. Categories.tsx passes `itemClassName:
   * "select-item-guest-visible"` (one real per-theme class, see `public/legacy/themes/*.css`,
   * reusing `.tankoubon-member-row`'s own color) for guest-visible categories and `itemStyle: {
   * fontWeight: "bold" }` for dynamic ones, without every caller of `Select` needing its own
   * bespoke item component just to express "this particular option looks different." Left off
   * entirely for items that don't need it (every call site before these fields existed).
   * `itemClassName` (a real theme-CSS class) is preferred over `itemStyle.backgroundColor` for
   * colors specifically — see `SelectItem`'s own docs on why a raw inline background can't
   * coexist with this popup's hover/keyboard-highlight state. */
  items: { value: Value; label: ReactNode; itemClassName?: string; itemStyle?: React.CSSProperties }[]
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
  /** Chakra-UI-style size preset (see `SELECT_SIZES` above) applied to the trigger *and* every
   * popup item's text together — a caller bumping only the trigger's own `fontSize` via `style`
   * (Categories.tsx's original approach) resized just the closed trigger, since `Select.Popup`/
   * `SelectItem` are a portal-mounted sibling tree, not a descendant of the trigger, and never
   * inherited that `style`; reported live, 2026-08-28, as "trigger got bigger, options inside are
   * still tiny." Omit for the default browser/CSS-inherited size (every existing call site before
   * this prop existed) — this is opt-in, not a forced resize of every `Select` in the app. */
  size?: keyof typeof SELECT_SIZES
} & Omit<ComponentProps<typeof BaseSelect.Root>, "value" | "onValueChange" | "items" | "children">) {
  const palette = useMenuPalette()
  const sizeStyle = size ? SELECT_SIZES[size] : undefined
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
        style={{ display: "inline-flex", alignItems: "center", justifyContent: "space-between", gap: 4, ...sizeStyle, ...style }}
      >
        <BaseSelect.Value />
        <BaseSelect.Icon>
          <FaCaretDown aria-hidden="true" />
        </BaseSelect.Icon>
      </BaseSelect.Trigger>
      <BaseSelect.Portal>
        {/* `alignItemWithTrigger={false}` — Base UI's own default (`true`) mimics a native
            `<select>`'s "selected item lands exactly under the cursor/trigger" placement, which
            computes the popup's available height from *that* anchor point rather than the
            trigger's own top/bottom edge. With a selected item partway down a long list this
            leaves little room above it, so the popup gets clipped to a `max-height` (real
            scrollbar) even though the full list would fit the viewport just fine as an ordinary
            below-the-trigger dropdown — reported live, 2026-08-28, as "why is there always a
            scrollbar, and it changes if I scroll first?" (the anchor point shifting per the
            highlighted item as arrow keys move through it). Disabling reverts to the ordinary
            "open below (or above, on collision) the trigger" placement every other popup in this
            app already uses, so the list only scrolls when it genuinely doesn't fit the
            viewport. */}
        <BaseSelect.Positioner
          sideOffset={4}
          alignItemWithTrigger={false}
          className="outline-none"
          style={{ zIndex: Z_OVERLAY_CONTENT }}
        >
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
              fontSize: sizeStyle?.fontSize,
            }}
          >
            <BaseSelect.List>
              {items.map((item) => (
                <SelectItem
                  key={item.value}
                  value={item.value}
                  showIndicator={showItemIndicator}
                  className={item.itemClassName}
                  style={item.itemStyle}
                >
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

function SelectItem({
  value,
  showIndicator,
  className,
  style,
  children,
}: {
  value: string
  showIndicator: boolean
  className?: string
  style?: React.CSSProperties
  children: ReactNode
}) {
  // `backgroundColor` is split out of `style` and re-applied as a CSS custom property instead of
  // a plain inline `background`/`backgroundColor` — an inline style always wins over
  // `.select-item-highlighted[data-highlighted]`'s own class-selector `background-color` rule
  // below regardless of specificity math, so an item-level background set the ordinary way would
  // permanently block that item from ever showing its hover/keyboard-highlight color at all. The
  // custom property is read back by this same class's own base (non-`[data-highlighted]`) rule in
  // each theme file, which the highlighted-state rule's higher specificity (class + attribute
  // selector vs. class alone) still legitimately overrides on hover/highlight, restoring normal
  // highlight behavior for these items too.
  const { backgroundColor, ...restStyle } = style ?? {}
  return (
    <BaseSelect.Item
      value={value}
      style={{ ...restStyle, ...(backgroundColor ? { "--select-item-bg": backgroundColor } : {}) } as React.CSSProperties}
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
      // `box-border` (not `box-content`, this file's own earlier choice) — `w-full` resolves to
      // the popup's own 100% width, but `content-box` sizing then adds this item's own
      // padding/border *on top of* that instead of inside it, pushing the item's real rendered
      // width (and its `data-highlighted`/`data-selected` background) past the popup's right edge
      // — confirmed live on a narrow (375px) mobile viewport, 2026-08-27: the selected item's
      // highlight visibly overflowed the popup box by exactly its own horizontal padding.
      // `border-box` makes `w-full` actually mean "100% including padding," matching every other
      // `w-full` element in this codebase's own implicit assumption.
      // `bg-[var(--select-item-bg,transparent)]` reads the custom property set above — a plain
      // class-selector rule, so `.select-item-highlighted[data-highlighted]`'s own higher-
      // specificity class+attribute selector still overrides it on hover/keyboard-highlight.
      className={`select-item-highlighted relative box-border w-full cursor-pointer items-center gap-2 select-none whitespace-nowrap bg-[var(--select-item-bg,transparent)] py-[.3em] pe-4 outline-none ${
        showIndicator ? "grid grid-cols-[1em_1fr] ps-2" : "flex ps-4"
      } ${className ?? ""}`}
    >
      {showIndicator && (
        // Left-aligned within its own column (no `justify-content: center`) and sized down to
        // `text-xs` — base-ui.com's own official example pairs `ItemIndicator` with a thin SVG
        // `CheckIcon` that reads visually smaller/tighter against the text than Font Awesome's
        // `fa-check` glyph does at its default (inherited, ~text-sm) size; both together (no
        // centering + a smaller glyph) is what actually closes the gap against the official
        // example's tighter look (real user feedback comparing screenshots, 2026-08-27).
        // `FaCheck` (react-icons/fa6, a real SVG) rather than the CSS-class `<i>` version — same
        // icon-font-vs-SVG centering/crispness reasoning as `Switch.tsx`'s own
        // `FaToggleOn`/`FaToggleOff` and `BookmarkHoverGrid.tsx`'s `FaTrashCan`.
        <BaseSelect.ItemIndicator className="col-start-1 flex text-xs">
          <FaCheck aria-hidden="true" />
        </BaseSelect.ItemIndicator>
      )}
      <BaseSelect.ItemText className={showIndicator ? "col-start-2" : undefined}>{children}</BaseSelect.ItemText>
    </BaseSelect.Item>
  )
}
