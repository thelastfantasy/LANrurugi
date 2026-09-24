import { Menu as BaseMenu } from "@base-ui/react/menu"
import { type ComponentProps, type ReactNode, useCallback, useRef } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, Z_OVERLAY_CONTENT } from "@/theme"

/** The side Base UI actually rendered the popup on — collision-aware, so it can differ from the
 * `side` prop requested (e.g. `"bottom"` requested near the bottom of the viewport flips to
 * `"top"` to stay on screen). `Menu.Positioner`'s own `data-side` DOM attribute is the only place
 * this fact is exposed outside the positioner's own children (Base UI's `useMenuPositionerContext`
 * that `Menu.Arrow` reads internally isn't part of the package's public API surface) — read via a
 * ref rather than depended on as an internal import that could move across a Base UI version. */
export type MenuSide = "top" | "bottom" | "left" | "right" | "inline-end" | "inline-start"

/** Site-wide dropdown/context menu built on Base UI's `Menu`, children-composed. Distinct from
 * `PopupMenu.tsx` (hand-rolled, pre-Base-UI) — use `Menu` for new menus going forward. */
export function Menu({
  trigger,
  children,
  side,
  align,
  onSideChange,
  ...rootProps
}: {
  trigger: ReactNode
  children: ReactNode
  /** Forwarded to Base UI's `Menu.Positioner` — defaults to its own default (`"bottom"`) when
   * omitted. Needed for a trigger anchored somewhere other than the popup's natural top edge, e.g.
   * a toolbar button near the top of the viewport where the menu must open downward regardless of
   * where the trigger itself sits on screen. */
  side?: ComponentProps<typeof BaseMenu.Positioner>["side"]
  align?: ComponentProps<typeof BaseMenu.Positioner>["align"]
  /** Reports the side Base UI actually rendered on (see {@link MenuSide}), read off the
   * positioner's own `data-side` attribute right after each open. Lets a caller-supplied trigger
   * (e.g. an arrow icon) point the direction the popup genuinely opened, not just the requested
   * `side` — which collision detection may have overridden. */
  onSideChange?: (side: MenuSide) => void
} & Omit<ComponentProps<typeof BaseMenu.Root>, "children">) {
  const palette = useMenuPalette()
  const positionerRef = useRef<HTMLDivElement>(null)

  const handleOpenChange = useCallback(
    (open: boolean, eventDetails: Parameters<NonNullable<ComponentProps<typeof BaseMenu.Root>["onOpenChange"]>>[1]) => {
      rootProps.onOpenChange?.(open, eventDetails)
      if (!open || !onSideChange) return
      // The positioner repositions itself on the frame after `open` flips true, so its `data-side`
      // isn't settled yet at this exact callback — one rAF is enough to read the value it actually
      // committed to the DOM.
      requestAnimationFrame(() => {
        const actual = positionerRef.current?.getAttribute("data-side") as MenuSide | null
        if (actual) onSideChange(actual)
      })
    },
    [onSideChange, rootProps],
  )

  return (
    <BaseMenu.Root {...rootProps} onOpenChange={handleOpenChange}>
      {/* render prop avoids Base UI wrapping an already-interactive trigger in a redundant <button>. */}
      <BaseMenu.Trigger render={trigger as React.ReactElement} />
      <BaseMenu.Portal>
        <BaseMenu.Positioner
          ref={positionerRef}
          side={side}
          align={align}
          sideOffset={4}
          className="outline-none"
          style={{ zIndex: Z_OVERLAY_CONTENT }}
        >
          <BaseMenu.Popup
            className={`m-0 w-max list-none rounded-[.2em] py-[.25em] text-left ${FLOATING_POPUP_TRANSITION_CLASSES}`}
            style={{
              background: palette.bg,
              border: `1px solid ${palette.border}`,
              boxShadow: FLOATING_POPUP_SHADOW,
              color: palette.text,
              transformOrigin: "var(--transform-origin)",
            }}
          >
            {children}
          </BaseMenu.Popup>
        </BaseMenu.Positioner>
      </BaseMenu.Portal>
    </BaseMenu.Root>
  )
}

/** One clickable (or disabled) row inside a {@link Menu} — closes on click by default
 * (`closeOnClick={true}`); pass `false` for an item that shouldn't dismiss the menu. */
export function MenuItem({
  disabled,
  onClick,
  closeOnClick,
  children,
}: {
  disabled?: boolean
  onClick?: () => void
  closeOnClick?: boolean
  children: ReactNode
}) {
  return (
    <BaseMenu.Item
      disabled={disabled}
      onClick={onClick}
      closeOnClick={closeOnClick}
      // .menu-item-highlighted[data-highlighted] rule lives per-theme in public/legacy/themes/*.css.
      className="menu-item-highlighted relative box-border w-full cursor-pointer select-none whitespace-nowrap px-4 py-[.3em] outline-none data-[disabled]:cursor-default data-[disabled]:opacity-60"
    >
      {children}
    </BaseMenu.Item>
  )
}

/** A horizontal divider between groups of {@link MenuItem}s — mirrors `PopupMenuSeparator`'s own
 * role and visual weight for a `Menu`-based popup. */
export function MenuSeparator() {
  const palette = useMenuPalette()
  return <BaseMenu.Separator className="my-[.35em] border-b" style={{ borderColor: palette.separator }} />
}
