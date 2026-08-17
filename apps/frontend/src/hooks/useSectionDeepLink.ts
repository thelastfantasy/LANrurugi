import { useEffect, useRef } from "react"
import { useSearchParams } from "react-router-dom"

/** Reads `?section=<id>` (e.g. `/config?section=security`), opens the matching
 * `CollapsibleSection` (`id` prop → `data-section-id`), scrolls it into view, and briefly flashes
 * its background — the same deep-link pattern `PluginCard.tsx`'s own `?focus=<namespace>` effect
 * already established for individual plugin cards, generalized here for whole accordion sections
 * so the Activity page's operation-content links can jump straight to "which settings section
 * changed" instead of just the bare `/config` page. `CollapsibleSection`'s `open` is local state
 * with no external prop to set it from here, so this simulates a real click on the section's own
 * `.collapsible-title` to open it — same effect a user clicking it themselves would have, just
 * fired programmatically once on mount for the matching section. */
export function useSectionDeepLink() {
  const [searchParams] = useSearchParams()
  const sectionId = searchParams.get("section")
  const didFocusRef = useRef(false)

  useEffect(() => {
    if (!sectionId || didFocusRef.current) return
    const el = document.querySelector(`[data-section-id="${CSS.escape(sectionId)}"]`)
    if (!el) return
    didFocusRef.current = true
    const title = el.querySelector<HTMLElement>(".collapsible-title")
    if (title && !title.classList.contains("active")) title.click()
    el.scrollIntoView({ block: "start", behavior: "smooth" })
    const htmlEl = el as HTMLElement
    htmlEl.style.transition = "background-color 0.5s"
    htmlEl.style.backgroundColor = "rgba(230, 126, 34, 0.18)"
    const timer = window.setTimeout(() => {
      htmlEl.style.backgroundColor = ""
    }, 2500)
    return () => window.clearTimeout(timer)
  }, [sectionId])
}
