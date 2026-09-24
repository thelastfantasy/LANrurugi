import Lenis from "lenis"
import { useEffect, useRef } from "react"

/** Mounts a Lenis instance on `elRef.current` for horizontal scrolling
 * (`gestureOrientation: "both"` lets vertical wheel input drive horizontal scroll). */
export function useHorizontalScroll(
  elRef: React.RefObject<HTMLElement | null>,
  opts?: { wheelMultiplier?: number; lerp?: number },
) {
  const lenisRef = useRef<Lenis | null>(null)

  useEffect(() => {
    const el = elRef.current
    if (!el) return
    const lenis = new Lenis({
      wrapper: el,
      content: el,
      orientation: "horizontal",
      gestureOrientation: "both",
      wheelMultiplier: opts?.wheelMultiplier ?? 4.5,
      lerp: opts?.lerp ?? 0.1,
      autoRaf: true,
    })
    lenisRef.current = lenis
    return () => {
      lenis.destroy()
      lenisRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return lenisRef
}
