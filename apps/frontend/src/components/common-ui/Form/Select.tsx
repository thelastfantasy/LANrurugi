import { Select as BaseSelect } from "@base-ui/react/select"
import { type ComponentProps, type ReactNode,useLayoutEffect, useRef, useState } from "react"
import { FaCaretDown, FaCheck } from "react-icons/fa6"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, FONT_SIZE_MD, FONT_SIZE_SM, Z_OVERLAY_ABOVE_LEGACY_MODAL, Z_OVERLAY_CONTENT } from "@/theme"

/** Chakra-UI-style size presets; fixed `height` avoids reflow based on font size alone. */
const SELECT_SIZES = {
  sm: { fontSize: FONT_SIZE_SM, height: 21 },
  md: { fontSize: FONT_SIZE_MD, height: 25 },
  lg: { fontSize: "1.25rem", height: 30 },
} as const

/** Site-wide `.favtag-btn`-styled select, built on Base UI's `Select`. Popup styled via
 * `useMenuPalette()` to match legacy's context-menu classes. */
export function Select<Value extends string>({
  value,
  onValueChange,
  items,
  ariaLabel,
  variant = "favtag-btn",
  showItemIndicator = true,
  style,
  size,
  align,
  popupZIndex,
  ...rootProps
}: {
  value: Value
  onValueChange: (value: Value) => void
  /** `itemClassName`/`itemStyle` (per-item, optional) merge onto that popup item's own
   * `className`/`style` — prefer `itemClassName` over `itemStyle.backgroundColor` for colors. */
  items: { value: Value; label: ReactNode; itemClassName?: string; itemStyle?: React.CSSProperties }[]
  ariaLabel?: string
  /** Pick `"stdbtn"` to match a neighboring `.stdbtn`'s height exactly. */
  variant?: "stdbtn" | "favtag-btn"
  /** Whether each popup item reserves a leading column for a checkmark on the selected item. */
  showItemIndicator?: boolean
  style?: React.CSSProperties
  /** Size preset for the trigger *and* popup item text — the popup is a portal-mounted sibling
   * that doesn't inherit the trigger's own `style`. Omit for the default inherited size. */
  size?: keyof typeof SELECT_SIZES
  /** Which edge of the trigger the popup lines up with. Base UI centres it by default, which
   * pushes a wide menu out past both sides of a narrow trigger; `"start"` lines the left edges
   * up, the usual look for a labelled form dropdown. */
  align?: "start" | "center" | "end"
  /** The popup is portalled to `body`, so a Select inside a dialog needs a z-index above that
   * dialog's own (`Z_OVERLAY_ABOVE_LEGACY_MODAL`) or its menu opens behind it. */
  popupZIndex?: number
} & Omit<ComponentProps<typeof BaseSelect.Root>, "value" | "onValueChange" | "items" | "children">) {
  const palette = useMenuPalette()
  const sizeStyle = size ? SELECT_SIZES[size] : undefined
  const triggerRef = useRef<HTMLButtonElement | null>(null)
  const [autoZIndex, setAutoZIndex] = useState<number | undefined>(undefined)

  // A dropdown portalled from inside a dialog renders *behind* the dialog unless its z-index clears
  // the dialog's own — one caller at a time remembering to pass `popupZIndex` was a bug that kept
  // recurring (sharing, then diagnostics). Detect the overlay here instead: any ancestor marked
  // `data-overlay` (see `Modal`) lifts the popup above it. An explicit `popupZIndex` still wins.
  useLayoutEffect(() => {
    if (popupZIndex !== undefined) return
    const overlay = triggerRef.current?.closest<HTMLElement>("[data-overlay]")
    setAutoZIndex(overlay ? Z_OVERLAY_ABOVE_LEGACY_MODAL : undefined)
  }, [popupZIndex])

  return (
    <BaseSelect.Root items={items} value={value} onValueChange={(v) => onValueChange(v as Value)} {...rootProps}>
      <BaseSelect.Trigger
        ref={triggerRef}
        className={variant}
        aria-label={ariaLabel}
        style={{ display: "inline-flex", alignItems: "center", justifyContent: "space-between", gap: 4, ...sizeStyle, ...style }}
      >
        {/* Render the selected item's own label, not just its value text: a caller that puts a glyph
            in `label` (the diagnostics state filter) otherwise loses it as soon as the popup closes. */}
        <BaseSelect.Value>
          {(value: Value | null) => {
            const selected = items.find((item) => item.value === value)
            return selected ? selected.label : String(value ?? "")
          }}
        </BaseSelect.Value>
        <BaseSelect.Icon>
          <FaCaretDown aria-hidden="true" />
        </BaseSelect.Icon>
      </BaseSelect.Trigger>
      <BaseSelect.Portal>
        <BaseSelect.Positioner
          sideOffset={4}
          align={align}
          alignItemWithTrigger={false}
          className="outline-none"
          style={{ zIndex: popupZIndex ?? autoZIndex ?? Z_OVERLAY_CONTENT }}
        >
          <BaseSelect.Popup
            // text-left resets legacy's body { text-align: center }, which inherits into the portal.
            className={`thin-scrollbar rounded-[.2em] py-[.25em] text-left ${FLOATING_POPUP_TRANSITION_CLASSES}`}
            style={{
              background: palette.bg,
              border: `1px solid ${palette.border}`,
              boxShadow: FLOATING_POPUP_SHADOW,
              color: palette.text,
              transformOrigin: "var(--transform-origin)",
              fontSize: sizeStyle?.fontSize,
              maxHeight: "var(--available-height)",
              overflowY: "auto",
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

/** `backgroundColor` is routed through the `--select-item-bg` CSS var, not a plain inline style,
 * so it doesn't permanently outrank the `[data-highlighted]` hover-color class rule. */
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
  const { backgroundColor, ...restStyle } = style ?? {}
  return (
    <BaseSelect.Item
      value={value}
      style={{ ...restStyle, ...(backgroundColor ? { "--select-item-bg": backgroundColor } : {}) } as React.CSSProperties}
      className={`select-item-highlighted relative box-border w-full cursor-pointer items-center gap-2 select-none whitespace-nowrap bg-[var(--select-item-bg,transparent)] py-[.3em] pe-4 outline-none ${
        showIndicator ? "grid grid-cols-[1em_1fr] ps-2" : "flex ps-4"
      } ${className ?? ""}`}
    >
      {showIndicator && (
        <BaseSelect.ItemIndicator className="col-start-1 flex text-xs">
          <FaCheck aria-hidden="true" />
        </BaseSelect.ItemIndicator>
      )}
      <BaseSelect.ItemText className={showIndicator ? "col-start-2" : undefined}>{children}</BaseSelect.ItemText>
    </BaseSelect.Item>
  )
}
