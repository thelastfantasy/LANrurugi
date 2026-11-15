import { useEffect, useRef } from "react"

/** Android's default touch-and-hold timeout, which native apps' long presses follow. */
export const LONG_PRESS_MS = 400
const MOVE_TOLERANCE_PX = 10

/** Touch/pen long press on `target` (an element, or `document`), timed by us rather than by the
 * browser's own `contextmenu`, whose delay differs per browser (Firefox waits 500 ms). A hold is
 * cancelled by moving, scrolling or a second finger. Once it fires, the click its release produces
 * is swallowed, so the press never also activates what is under the finger. Mouse input is left
 * alone: right-click keeps going through `contextmenu`. */
export function useLongPress(
  target: EventTarget | null,
  onLongPress: (pressed: Element, e: PointerEvent) => void,
  {
    enabled = true,
    match,
    blockContextMenu,
  }: {
    enabled?: boolean
    /** Which pressed element counts: return it (passed to `onLongPress`) or null to ignore. */
    match?: (target: Element) => Element | null
    /** Keep the browser's own long-press menu (save image, link callout) closed for touches on
     * these elements; on Android it would cover whatever the long press opens. */
    blockContextMenu?: (target: Element) => boolean
  } = {},
) {
  const latest = useRef({ onLongPress, match, blockContextMenu })
  useEffect(() => {
    latest.current = { onLongPress, match, blockContextMenu }
  })

  useEffect(() => {
    if (!enabled || !target) return
    let timer: number | undefined
    let startX = 0
    let startY = 0
    // A touch is down on the target and has not been released; `pointercancel` does not count,
    // because the browser claiming a held image fires it just before its own menu.
    let touchHeld = false
    const pointers = new Set<number>()

    const stopTimer = () => {
      window.clearTimeout(timer)
      timer = undefined
    }
    const swallowClick = (e: Event) => {
      e.preventDefault()
      e.stopPropagation()
      unswallow()
    }
    // A hold whose release produced no click must not eat the next tap instead.
    const unswallow = () => {
      window.removeEventListener("click", swallowClick, true)
      window.removeEventListener("pointerdown", unswallow, true)
    }
    const movedAt = (x: number, y: number) => {
      if (Math.abs(x - startX) > MOVE_TOLERANCE_PX || Math.abs(y - startY) > MOVE_TOLERANCE_PX) stopTimer()
    }
    const onMove = (e: PointerEvent) => movedAt(e.clientX, e.clientY)
    // Once the browser takes a drag over as a scroll it cancels the pointer, and no pointer moves
    // follow; touch events still do, so the hold is watched through them until the finger lifts.
    const onTouchMove = (e: TouchEvent) => {
      const touch = e.touches[0]
      if (touch) movedAt(touch.clientX, touch.clientY)
    }
    const onTouchEnd = (e: TouchEvent) => {
      if (e.touches.length > 0) return
      stopTimer()
      detach()
    }
    // A browser claiming a held image for its own menu cancels the touch too, before that menu;
    // as with `pointercancel`, the hold itself stands.
    const onTouchCancel = () => detach()
    const onOtherDown = (e: PointerEvent) => {
      pointers.add(e.pointerId)
      if (pointers.size > 1) stopTimer()
    }
    const onUp = (e: PointerEvent) => {
      pointers.delete(e.pointerId)
      touchHeld = false
      if (pointers.size === 0) {
        stopTimer()
        detach()
      }
    }
    // Not a cancel of the hold: see `touchHeld`. Scrolling, the other way a browser cancels a
    // pointer, is caught by `onScroll` and `onTouchMove`, which stay attached until release.
    const onCancel = (e: PointerEvent) => {
      pointers.delete(e.pointerId)
    }
    const onScroll = () => stopTimer()
    const attach = () => {
      document.addEventListener("pointerdown", onOtherDown)
      document.addEventListener("pointermove", onMove)
      document.addEventListener("pointerup", onUp)
      document.addEventListener("pointercancel", onCancel)
      document.addEventListener("scroll", onScroll, true)
      document.addEventListener("touchmove", onTouchMove, { passive: true })
      document.addEventListener("touchend", onTouchEnd)
      document.addEventListener("touchcancel", onTouchCancel)
    }
    const detach = () => {
      document.removeEventListener("pointerdown", onOtherDown)
      document.removeEventListener("pointermove", onMove)
      document.removeEventListener("pointerup", onUp)
      document.removeEventListener("pointercancel", onCancel)
      document.removeEventListener("scroll", onScroll, true)
      document.removeEventListener("touchmove", onTouchMove)
      document.removeEventListener("touchend", onTouchEnd)
      document.removeEventListener("touchcancel", onTouchCancel)
    }

    const onDown = (e: Event) => {
      const ev = e as PointerEvent
      if (ev.pointerType === "mouse") {
        touchHeld = false
        return
      }
      if (!(ev.target instanceof Element)) return
      const { match: matchFn } = latest.current
      const pressed = matchFn ? matchFn(ev.target) : ev.target
      if (!pressed) return
      unswallow()
      stopTimer()
      pointers.clear()
      pointers.add(ev.pointerId)
      touchHeld = true
      startX = ev.clientX
      startY = ev.clientY
      attach()
      timer = window.setTimeout(() => {
        timer = undefined
        window.addEventListener("click", swallowClick, true)
        window.addEventListener("pointerdown", unswallow, true)
        latest.current.onLongPress(pressed, ev)
      }, LONG_PRESS_MS)
    }
    const onContextMenu = (e: Event) => {
      const block = latest.current.blockContextMenu
      if (!block || !(e.target instanceof Element) || !block(e.target)) return
      const pointerType = (e as PointerEvent).pointerType
      const fromTouch =
        pointerType === "touch" || pointerType === "pen" || (!pointerType && touchHeld)
      if (!fromTouch) return
      e.preventDefault()
      e.stopPropagation()
    }

    target.addEventListener("pointerdown", onDown)
    target.addEventListener("contextmenu", onContextMenu)
    return () => {
      stopTimer()
      detach()
      unswallow()
      target.removeEventListener("pointerdown", onDown)
      target.removeEventListener("contextmenu", onContextMenu)
    }
  }, [enabled, target])
}
