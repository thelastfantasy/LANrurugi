import { useState } from "react"

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
export function CollapsibleSection({
  id,
  icon,
  title,
  defaultOpen = false,
  caretStyle = "up-down",
  children,
}: {
  /** Stable identifier for this section, used only as a `data-section-id` attribute for
   * `useSectionDeepLink` (`hooks/useSectionDeepLink.ts`) to find and auto-open/scroll-to via a
   * `?section=<id>` URL param — e.g. `/config?section=security` deep-linking straight to the
   * Security accordion from the Activity page's own operation-content links. Omit for a section
   * that doesn't need to be independently linkable. */
  id?: string
  icon: string
  title: React.ReactNode
  defaultOpen?: boolean
  caretStyle?: "up-down" | "right-down"
  children: React.ReactNode
}) {
  const [open, setOpen] = useState(defaultOpen)
  const caretClass = caretStyle === "right-down" ? "caret-right-down" : "caret-right"

  return (
    <li className="option-flyout" data-section-id={id}>
      <div className={`collapsible-title ${caretClass}${open ? " active" : ""}`} onClick={() => setOpen((o) => !o)}>
        <i className={`fa ${icon}`} aria-hidden="true"></i> {title}
      </div>
      {/* Rendered even while closed once this section has an `id` — a deep link needs the body's
          own DOM node to exist so `useSectionDeepLink` can scroll to it and flip `open` via the
          `data-section-id` click below; the collapsed default (`display: none` in
          `allcollapsible.css`) means always-rendering costs nothing visible until opened, unlike
          conditionally mounting only once `open` first becomes true. Sections without an `id`
          keep the original mount-on-open behavior — nothing else observes their DOM presence. */}
      {(open || id) && <div className="collapsible-body" style={open ? undefined : { display: "none" }}>{children}</div>}
    </li>
  )
}
