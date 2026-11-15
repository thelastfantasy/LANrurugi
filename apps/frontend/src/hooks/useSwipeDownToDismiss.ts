import { useEffect, useRef } from "react"

const DISMISS_PX = 60

/** One-finger drag down on `el` slides it with the finger and dismisses it past `DISMISS_PX`, else
 * springs back. The drag is the element's own: its moves are not passed on as a page scroll, and an
 * upward or sideways drag is left alone. For panels that are not a Base UI `Drawer`, which already
 * does this itself. */
export function useSwipeDownToDismiss(el: HTMLElement | null, onDismiss: () => void) {
  const dismissRef = useRef(onDismiss)
  useEffect(() => {
    dismissRef.current = onDismiss
  })

  useEffect(() => {
    if (!el) return
    let startX = 0
    let startY = 0
    let dragging = false
    let decided = false
    const reset = () => {
      el.style.transition = "transform 0.2s cubic-bezier(0.32, 0.72, 0, 1)"
      el.style.transform = ""
    }
    const onStart = (e: TouchEvent) => {
      if (e.touches.length !== 1) return
      startX = e.touches[0].clientX
      startY = e.touches[0].clientY
      dragging = false
      decided = false
    }
    const onMove = (e: TouchEvent) => {
      if (e.touches.length !== 1) return
      const dx = e.touches[0].clientX - startX
      const dy = e.touches[0].clientY - startY
      if (!decided) {
        if (Math.abs(dx) < 6 && Math.abs(dy) < 6) return
        decided = true
        dragging = dy > 0 && dy > Math.abs(dx)
      }
      if (!dragging) return
      e.preventDefault()
      el.style.transition = "none"
      el.style.transform = `translateY(${Math.max(0, dy)}px)`
    }
    const onEnd = (e: TouchEvent) => {
      if (!dragging) return
      dragging = false
      const dy = (e.changedTouches[0]?.clientY ?? startY) - startY
      if (dy > DISMISS_PX) dismissRef.current()
      else reset()
    }
    el.addEventListener("touchstart", onStart, { passive: true })
    // Non-passive: the drag must not also scroll the page under the panel.
    el.addEventListener("touchmove", onMove, { passive: false })
    el.addEventListener("touchend", onEnd)
    el.addEventListener("touchcancel", reset)
    return () => {
      el.removeEventListener("touchstart", onStart)
      el.removeEventListener("touchmove", onMove)
      el.removeEventListener("touchend", onEnd)
      el.removeEventListener("touchcancel", reset)
    }
  }, [el])
}
