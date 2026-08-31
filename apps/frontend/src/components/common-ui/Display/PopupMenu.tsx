import { Separator } from "@base-ui/react/separator"
import type { CSSProperties, ReactNode } from "react"
import { forwardRef } from "react"
import { createPortal } from "react-dom"

import { useMenuPalette } from "@/hooks/useMenuPalette"

/** A themed popup menu box for right-click menus and the settings gear menu. `portal` (default
 * `true`) renders into `document.body`; pass `false` for a submenu positioned via `left: '100%'`. */
export const PopupMenu = forwardRef<
  HTMLUListElement,
  {
    style?: CSSProperties
    portal?: boolean
    onMouseEnter?: () => void
    onMouseLeave?: () => void
    mainLabel?: { text: string; icon: string }
    children: ReactNode
  }
>(function PopupMenu({ style, portal = true, onMouseEnter, onMouseLeave, mainLabel, children }, ref) {
  const palette = useMenuPalette()
  const menu = (
    <ul
      ref={ref}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      // ps-0 is load-bearing: list-none only resets list-style-type, not the UA's own
      // padding-inline-start: 40px, which would otherwise stack with PopupMenuItem's own px-4.
      className="m-[.3em] inline-block w-max list-none rounded-[.2em] py-[.25em] ps-0 text-left"
      style={{
        background: palette.bg,
        border: `1px solid ${palette.border}`,
        boxShadow: palette.shadow,
        color: palette.text,
        ...style,
      }}
    >
      {mainLabel && (
        <>
          <PopupMenuItem disabled>
            <i className={`fa ${mainLabel.icon}`} style={{ width: 18 }}></i> {mainLabel.text}
          </PopupMenuItem>
          <PopupMenuSeparator />
        </>
      )}
      {children}
    </ul>
  )
  return portal ? createPortal(menu, document.body) : menu
})

/** One clickable (or disabled) row inside a {@link PopupMenu} — matches legacy's own
 * `.context-menu-item.context-menu-disabled` for the disabled state. */
export function PopupMenuItem({
  disabled,
  onClick,
  onMouseDown,
  onMouseEnter,
  onMouseLeave,
  style,
  children,
}: {
  disabled?: boolean
  onClick?: (e: React.MouseEvent) => void
  onMouseDown?: (e: React.MouseEvent) => void
  onMouseEnter?: () => void
  onMouseLeave?: () => void
  style?: CSSProperties
  children: ReactNode
}) {
  const palette = useMenuPalette()
  return (
    <li
      className="relative box-content whitespace-nowrap select-none px-4 py-[.3em]"
      style={{
        cursor: disabled ? "default" : "pointer",
        opacity: disabled ? 0.6 : 1,
        ...style,
      }}
      onClick={disabled ? undefined : onClick}
      onMouseDown={disabled ? undefined : onMouseDown}
      onMouseEnter={(e) => {
        if (disabled) return
        e.currentTarget.style.background = palette.hoverBg
        e.currentTarget.style.color = palette.hoverText
        onMouseEnter?.()
      }}
      onMouseLeave={(e) => {
        if (disabled) return
        e.currentTarget.style.background = "transparent"
        e.currentTarget.style.color = palette.text
        onMouseLeave?.()
      }}
    >
      {children}
    </li>
  )
}

/** A horizontal divider between groups of {@link PopupMenuItem}s — matches legacy's own
 * `.context-menu-separator`. Renders as `<li>` (via `render`) since it sits inside `PopupMenu`'s
 * own `<ul>`, not Base UI's default `<div>`. Negative horizontal margin cancels out the `<ul>`'s
 * own `m-[.3em]` so the line reaches the Popup's real outer border instead of stopping at the
 * `<ul>`'s own inset edge, which the menu rows' `px-4` (padding, not margin) doesn't share. */
export function PopupMenuSeparator() {
  const palette = useMenuPalette()
  return <Separator render={<li />} className="my-[.35em] -mx-[.3em] border-b" style={{ borderColor: palette.separator }} />
}
