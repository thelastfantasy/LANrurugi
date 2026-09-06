import { useEffect, useRef } from "react"
import { useSearchParams } from "react-router-dom"

/** Reads `?section=<id>`, opens the matching `CollapsibleSection`, scrolls it into view, and
 * briefly flashes its background — simulates a click since `open` has no external prop. */
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
