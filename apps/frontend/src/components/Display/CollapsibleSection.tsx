import { useState } from "react"

/** Legacy's `.collapsible.extensible.with-right-caret > li.option-flyout` accordion pattern —
 * clicking `.collapsible-title` toggles `.active` independently per section (no sibling collapse).
 *
 * The title is not selectable: a fast double click to open/close used to select the header text and,
 * once a selection existed, the next drag selected the section's whole body (2026-11-01). */
export function CollapsibleSection({
  id,
  icon,
  title,
  defaultOpen = false,
  open: openProp,
  onOpenChange,
  caretStyle = "up-down",
  as: Container = "li",
  iconClassName,
  titleStyle,
  bodyStyle,
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
  /** The element the section is wrapped in. Settings sections are list items; a panel embedded in a
   * card is a `div`. */
  as?: "li" | "div"
  /** A full icon class list used verbatim, for callers that need more than one token
   * (`fas fa-sliders-h fa-2x`); `icon` remains the short form that gets the `fa ` prefix. */
  iconClassName?: string
  titleStyle?: React.CSSProperties
  bodyStyle?: React.CSSProperties
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
    <Container className="option-flyout" data-section-id={id}>
      <div
        className={`collapsible-title ${caretClass}${open ? " active" : ""}`}
        style={{ userSelect: "none", ...titleStyle }}
        onClick={toggle}
      >
        <i className={iconClassName ?? `fa ${icon}`} aria-hidden="true"></i> {title}
      </div>
      {(open || id) && (
        <div className="collapsible-body" style={open ? bodyStyle : { display: "none" }}>
          {children}
        </div>
      )}
    </Container>
  )
}
