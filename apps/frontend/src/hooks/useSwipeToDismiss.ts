import { useEffect, useRef } from "react"

const DISMISS_PX = 60
/** Movement before a drag picks a direction — under this it is still just a tap. */
const SLOP_PX = 6

/** One-finger drag along `direction` starting inside `el` slides it with the finger and dismisses it
 * past `DISMISS_PX`, else springs back. A drag the other way is left alone, and a touch that starts
 * inside a `[data-swipe-ignore]` subtree (a horizontally scrolling strip, say) keeps that subtree's
 * gesture. For panels that are not a Base UI `Drawer`, which already does this itself.
 *
 * The move listener sits on the document in the capture phase and is non-passive, like Base UI's own
 * swipe: a page-level `touchmove` handler is what actually suppresses the browser's swipe-to-go-back
 * (Android's back gesture, Firefox Android's own), which no amount of styling prevents. */
export function useSwipeToDismiss(
  el: HTMLElement | null,
  onDismiss: () => void,
  direction: "down" | "right" = "down",
) {
  const dismissRef = useRef(onDismiss)
  useEffect(() => {
    dismissRef.current = onDismiss
  })

  useEffect(() => {
    if (!el) return
    const horizontal = direction === "right"
    let active = false
    let startX = 0
    let startY = 0
    let dragging = false
    let decided = false
    let claimed = false
    const reset = () => {
      el.style.transition = "transform 0.2s cubic-bezier(0.32, 0.72, 0, 1)"
      el.style.transform = ""
    }
    const onStart = (e: TouchEvent) => {
      active = false
      if (e.touches.length !== 1) return
      if (!el.contains(e.target as Node)) return
      if ((e.target as Element | null)?.closest("[data-swipe-ignore]")) return
      active = true
      startX = e.touches[0].clientX
      startY = e.touches[0].clientY
      dragging = false
      decided = false
      claimed = false
    }
    const onMove = (e: TouchEvent) => {
      if (!active || e.touches.length !== 1) return
      const touch = e.touches[0]
      const dx = touch.clientX - startX
      const dy = touch.clientY - startY
      if (!claimed) {
        claimed = true
        // Claim the gesture on its first move, before the browser can read it as a navigation.
        if (e.cancelable) e.preventDefault()
      }
      if (!decided) {
        if (Math.abs(dx) < SLOP_PX && Math.abs(dy) < SLOP_PX) return
        decided = true
        dragging = horizontal ? dx > 0 && dx > Math.abs(dy) : dy > 0 && dy > Math.abs(dx)
      }
      if (!dragging) return
      if (e.cancelable) e.preventDefault()
      e.stopPropagation()
      el.style.transition = "none"
      el.style.transform = horizontal ? `translateX(${dx}px)` : `translateY(${dy}px)`
    }
    const onEnd = (e: TouchEvent) => {
      if (!active) return
      active = false
      if (!dragging) return
      dragging = false
      const touch = e.changedTouches[0]
      const moved = horizontal
        ? (touch?.clientX ?? startX) - startX
        : (touch?.clientY ?? startY) - startY
      if (moved > DISMISS_PX) dismissRef.current()
      else reset()
    }
    const onCancel = () => {
      if (!active) return
      active = false
      if (dragging) reset()
      dragging = false
    }
    document.addEventListener("touchstart", onStart, { capture: true, passive: true })
    document.addEventListener("touchmove", onMove, { capture: true, passive: false })
    document.addEventListener("touchend", onEnd, { capture: true })
    document.addEventListener("touchcancel", onCancel, { capture: true })
    return () => {
      document.removeEventListener("touchstart", onStart, { capture: true })
      document.removeEventListener("touchmove", onMove, { capture: true })
      document.removeEventListener("touchend", onEnd, { capture: true })
      document.removeEventListener("touchcancel", onCancel, { capture: true })
    }
  }, [el, direction])
}
