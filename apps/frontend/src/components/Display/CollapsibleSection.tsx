import { useState } from "react"
import { FaCaretDown, FaCaretRight, FaCaretUp } from "react-icons/fa6"

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
  // The legacy glyphs came from the stylesheets (`.caret-right` closed ▼ / open ▲,
  // `.caret-right-down` closed ► / open ▼); they were plain text characters, so a real icon is
  // rendered here instead and the pseudo-element classes are no longer applied.
  const Caret =
    caretStyle === "right-down"
      ? open
        ? FaCaretDown
        : FaCaretRight
      : open
        ? FaCaretUp
        : FaCaretDown

  function toggle() {
    if (isControlled) onOpenChange?.(!open)
    else setUncontrolledOpen((o) => !o)
  }

  return (
    <Container className="option-flyout" data-section-id={id}>
      <div
        className={`collapsible-title${open ? " active" : ""}`}
        // Reserve the right-hand strip the caret occupies: it is absolutely positioned so it no
        // longer takes part in the line box (which is what kept the title's own content centered).
        style={{ userSelect: "none", position: "relative", ...titleStyle, paddingRight: "2.5rem" }}
        onClick={toggle}
      >
        <i className={iconClassName ?? `fa ${icon}`} aria-hidden="true"></i> {title}
        <span
          aria-hidden="true"
          style={{
            position: "absolute",
            right: "0.75rem",
            top: "50%",
            transform: "translateY(-50%)",
            display: "inline-flex",
            lineHeight: 1,
            fontSize: "1em",
            pointerEvents: "none",
          }}
        >
          <Caret />
        </span>
      </div>
      {(open || id) && (
        <div className="collapsible-body" style={open ? bodyStyle : { display: "none" }}>
          {children}
        </div>
      )}
    </Container>
  )
}
