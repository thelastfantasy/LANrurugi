import { useEffect, useRef, useState } from "react"

/** True once the element has scrolled within `rootMargin` of the viewport, and stays true.
 *
 *  Long lists (a selection gathered from every search result) mount thousands of rows; each one
 *  wants its archive's metadata. Deferring that request until the row is about to be seen keeps
 *  the drawer responsive without dropping any row from the DOM, which the sortable list needs. */
export function useNearViewport<T extends HTMLElement = HTMLDivElement>(rootMargin = "300px") {
  const ref = useRef<T | null>(null)
  const [near, setNear] = useState(false)

  useEffect(() => {
    if (near) return
    const element = ref.current
    if (!element) return
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          setNear(true)
          observer.disconnect()
        }
      },
      { rootMargin },
    )
    observer.observe(element)
    return () => observer.disconnect()
  }, [near, rootMargin])

  return [ref, near] as const
}
