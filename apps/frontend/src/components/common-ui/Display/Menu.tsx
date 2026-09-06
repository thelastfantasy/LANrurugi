import { Menu as BaseMenu } from "@base-ui/react/menu"
import type { ComponentProps, ReactNode } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, Z_OVERLAY_CONTENT } from "@/theme"

/** Site-wide dropdown/context menu built on Base UI's `Menu`, children-composed. Distinct from
 * `PopupMenu.tsx` (hand-rolled, pre-Base-UI) — use `Menu` for new menus going forward. */
export function Menu({
  trigger,
  children,
  ...rootProps
}: {
  trigger: ReactNode
  children: ReactNode
} & Omit<ComponentProps<typeof BaseMenu.Root>, "children">) {
  const palette = useMenuPalette()
  return (
    <BaseMenu.Root {...rootProps}>
      {/* render prop avoids Base UI wrapping an already-interactive trigger in a redundant <button>. */}
      <BaseMenu.Trigger render={trigger as React.ReactElement} />
      <BaseMenu.Portal>
        <BaseMenu.Positioner sideOffset={4} className="outline-none" style={{ zIndex: Z_OVERLAY_CONTENT }}>
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
