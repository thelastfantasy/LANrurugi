import { useState } from 'react'

/** Legacy's real `.collapsible.extensible.with-right-caret > li.option-flyout` accordion pattern
 * (`allcollapsible.js`, verified via the real vendor source) — every `.collapsible-body` starts
 * hidden, and clicking a `.collapsible-title` toggles its own `.active` class (which the real
 * `allcollapsible.css`/theme CSS uses to flip the `::after` caret glyph) independently of any
 * other section, since the wrapping `<ul>` carries `.extensible` (no accordion-exclusive
 * collapsing of siblings). */
export default function CollapsibleSection({
  icon,
  title,
  defaultOpen = false,
  children,
}: {
  icon: string
  title: React.ReactNode
  defaultOpen?: boolean
  children: React.ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)

  return (
    <li className="option-flyout">
      <div className={`collapsible-title caret-right${open ? ' active' : ''}`} onClick={() => setOpen((o) => !o)}>
        <i className={`fa ${icon}`} aria-hidden="true"></i> {title}
      </div>
      {open && <div className="collapsible-body">{children}</div>}
    </li>
  )
}
