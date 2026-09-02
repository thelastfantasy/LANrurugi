import { useState } from "react"

/** Legacy's `.collapsible.extensible.with-right-caret > li.option-flyout` accordion pattern —
 * clicking `.collapsible-title` toggles `.active` independently per section (no sibling collapse). */
export function CollapsibleSection({
  id,
  icon,
  title,
  defaultOpen = false,
  open: openProp,
  onOpenChange,
  caretStyle = "up-down",
  children,
}: {
  /** Used as `data-section-id` for `useSectionDeepLink` to auto-open/scroll-to via `?section=<id>`.
   * Omit for a section that doesn't need to be independently linkable. */
  id?: string
  icon: string
  title: React.ReactNode
  defaultOpen?: boolean
  /** Controlled open state — when provided (with `onOpenChange`), a caller drives it externally.
   * Omit both for uncontrolled behavior. */
  open?: boolean
  onOpenChange?: (open: boolean) => void
  caretStyle?: "up-down" | "right-down"
  children: React.ReactNode
}) {
  const isControlled = openProp !== undefined
  const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen)
  const open = isControlled ? openProp : uncontrolledOpen
  const caretClass = caretStyle === "right-down" ? "caret-right-down" : "caret-right"

  function toggle() {
    if (isControlled) onOpenChange?.(!open)
    else setUncontrolledOpen((o) => !o)
  }

  return (
    <li className="option-flyout" data-section-id={id}>
      <div className={`collapsible-title ${caretClass}${open ? " active" : ""}`} onClick={toggle}>
        <i className={`fa ${icon}`} aria-hidden="true"></i> {title}
      </div>
      {(open || id) && <div className="collapsible-body" style={open ? undefined : { display: "none" }}>{children}</div>}
    </li>
  )
}
