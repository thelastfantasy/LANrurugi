import { type TouchEvent, useRef } from "react"

/** A horizontal swipe is at least this far, and clearly more sideways than vertical. */
const MIN_DISTANCE_PX = 50
const MIN_SLOPE = 1.5

/** Touch-only left/right swipe detection for an element. Spread the result onto it; it also sets
 * `touch-action: pan-y`, so vertical scrolling still belongs to the browser. Touch events rather
 * than pointer events: a touch's events always go to the element it started on, while a pointer
 * can be captured mid-gesture by an ancestor (e.g. a swipe-to-dismiss drawer). */
export function useHorizontalSwipe({ onSwipeLeft, onSwipeRight }: { onSwipeLeft: () => void; onSwipeRight: () => void }) {
  const start = useRef<{ x: number; y: number } | null>(null)
  return {
    style: { touchAction: "pan-y" } as const,
    onTouchStart: (e: TouchEvent) => {
      const touch = e.touches[0]
      // React bubbles events out of portals; only a touch that starts inside this element's own
      // DOM counts, not one on content portaled elsewhere (e.g. a `DrawerFooter`).
      const inside = e.currentTarget.contains(e.target as Node)
      start.current = touch && e.touches.length === 1 && inside ? { x: touch.clientX, y: touch.clientY } : null
    },
    onTouchEnd: (e: TouchEvent) => {
      const s = start.current
      start.current = null
      const touch = e.changedTouches[0]
      if (!s || !touch) return
      const dx = touch.clientX - s.x
      const dy = touch.clientY - s.y
      if (Math.abs(dx) < MIN_DISTANCE_PX || Math.abs(dx) < MIN_SLOPE * Math.abs(dy)) return
      if (dx < 0) onSwipeLeft()
      else onSwipeRight()
    },
    onTouchCancel: () => {
      start.current = null
    },
  }
}
