import type { CSSProperties, ReactNode } from 'react'
import { forwardRef } from 'react'
import { createPortal } from 'react-dom'

import { useSettings } from '../api/hooks'
import { DEFAULT_THEME_ID, MENU_PALETTE } from '../theme'

/** Reads the current theme's popup-menu palette — the single place every `PopupMenu` consumer
 * pulls colours from, so a right-click menu, the index settings gear menu, etc. all reskin
 * together when the user switches themes. */
export function useMenuPalette() {
  const settings = useSettings()
  const theme = (settings.data?.theme ?? DEFAULT_THEME_ID) as keyof typeof MENU_PALETTE
  return MENU_PALETTE[theme] ?? MENU_PALETTE[DEFAULT_THEME_ID]
}

/** A themed popup menu box — the shared "floating list of clickable rows" every right-click menu
 * and the index settings gear menu renders. Built from scratch with Tailwind utility classes and
 * this app's own `MENU_PALETTE` colour table — no third-party menu-plugin CSS is linked in, but
 * the visual result still matches each of legacy's 5 real themes.
 *
 * `portal` (default `true`) renders via `createPortal` into `document.body` instead of as a
 * normal DOM child of the trigger — same reasoning as `Tooltip`'s own portal: an ancestor's
 * `overflow: hidden`/`scroll` would otherwise clip a `position: fixed`/`absolute` menu, and an
 * ancestor `transform` breaks `fixed`'s viewport-relative coordinates. Pass `portal={false}` for
 * a menu intentionally positioned relative to its own parent instead of the viewport (e.g. a
 * submenu opening off a parent `PopupMenuItem` via `left: '100%'`).
 *
 * Forwards `ref` onto the rendered `<ul>` itself (not the trigger). A caller's own
 * outside-click-to-close handler that does `triggerRef.current.contains(e.target)` breaks once
 * the menu is portaled to `document.body` — pass a second ref here and also check
 * `menuRef.current?.contains(e.target)` to fix that. */
export const PopupMenu = forwardRef<
  HTMLUListElement,
  {
    style?: CSSProperties
    portal?: boolean
    onMouseEnter?: () => void
    onMouseLeave?: () => void
    children: ReactNode
  }
>(function PopupMenu({ style, portal = true, onMouseEnter, onMouseLeave, children }, ref) {
  const palette = useMenuPalette()
  const menu = (
    <ul
      ref={ref}
      onMouseEnter={onMouseEnter}
      onMouseLeave={onMouseLeave}
      // `inline-block` + `w-max` sizes to its own longest row's real content rather than a
      // guessed min/max range. `text-left` overrides any ancestor's own `text-align`. `ps-0` is
      // load-bearing: `list-none` only resets `list-style-type`, not the UA stylesheet's own
      // `padding-inline-start: 40px` on every `<ul>` — left unreset, that 40px stacks with each
      // `PopupMenuItem`'s own `px-4`, making the menu visibly lopsided.
      className="m-[.3em] inline-block w-max list-none rounded-[.2em] py-[.25em] ps-0 text-left"
      style={{
        background: palette.bg,
        border: `1px solid ${palette.border}`,
        boxShadow: palette.shadow,
        color: palette.text,
        ...style,
      }}
    >
      {children}
    </ul>
  )
  return portal ? createPortal(menu, document.body) : menu
})

/** One clickable (or disabled) row inside a {@link PopupMenu}. `disabled` rows (e.g. a "Display
 * Mode" section header) get the theme's dimmed text colour and no hover/click affordance —
 * matches legacy's own `.context-menu-item.context-menu-disabled`. */
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
        cursor: disabled ? 'default' : 'pointer',
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
        e.currentTarget.style.background = 'transparent'
        e.currentTarget.style.color = palette.text
        onMouseLeave?.()
      }}
    >
      {children}
    </li>
  )
}

/** A horizontal divider between groups of {@link PopupMenuItem}s — matches legacy's own
 * `.context-menu-separator`. */
export function PopupMenuSeparator() {
  const palette = useMenuPalette()
  return <li className="my-[.35em] border-b" style={{ borderColor: palette.separator }}></li>
}
