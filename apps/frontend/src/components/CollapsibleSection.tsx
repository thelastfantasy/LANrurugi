import { useState } from 'react'

/** Legacy's real `.collapsible.extensible.with-right-caret > li.option-flyout` accordion pattern
 * (`allcollapsible.js`, verified via the real vendor source) — every `.collapsible-body` starts
 * hidden, and clicking a `.collapsible-title` toggles its own `.active` class (which the real
 * `allcollapsible.css`/theme CSS uses to flip the `::after` caret glyph) independently of any
 * other section, since the wrapping `<ul>` carries `.extensible` (no accordion-exclusive
 * collapsing of siblings).
 *
 * `caretStyle` picks which of `allcollapsible.css`'s two real caret glyph pairs to use, both
 * verified against that file's own `:after`/`.active:after` rules — `'up-down'` (the default,
 * used by every other page: Plugins/Stats/Settings) is `.caret-right`'s own pair (▼ closed, ▲
 * open); `'right-down'` is a distinct real class in the same file, `.caret`'s pair (▶ closed, ▼
 * open) — a caller opts into it explicitly rather than this component silently picking a
 * different glyph pair per page. */
export default function CollapsibleSection({
  icon,
  title,
  defaultOpen = false,
  caretStyle = 'up-down',
  children,
}: {
  icon: string
  title: React.ReactNode
  defaultOpen?: boolean
  caretStyle?: 'up-down' | 'right-down'
  children: React.ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  const caretClass = caretStyle === 'right-down' ? 'caret-right-down' : 'caret-right'

  return (
    <li className="option-flyout">
      <div className={`collapsible-title ${caretClass}${open ? ' active' : ''}`} onClick={() => setOpen((o) => !o)}>
        <i className={`fa ${icon}`} aria-hidden="true"></i> {title}
      </div>
      {open && <div className="collapsible-body">{children}</div>}
    </li>
  )
}
