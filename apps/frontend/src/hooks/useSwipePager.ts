import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react"

const SLIDE_MS = 260
const EASE = "cubic-bezier(0.32, 0.72, 0, 1)"
/** A drag past this share of the page width (or this many px, whichever is less) turns the page. */
const TURN_FRACTION = 0.25
const TURN_MAX_PX = 80
/** How much a drag past the first or last page still moves, as a hint that there is nothing more. */
const EDGE_RESISTANCE = 0.3

/** Book-like paging, one page at a time: the caller renders the current page in flow with its
 * neighbours beside it (`slotStyle`) inside `trackRef`'s element. A sideways drag on `areaRef`'s
 * element slides the track with the finger, so the current page leaves as the next one comes in;
 * past the threshold the slide finishes and only then is the new page reported, else it springs
 * back. `go` plays the same slide for buttons and keys. A rightward drag on the first page is left
 * to an enclosing swipe-to-close drawer. */
export function useSwipePager({
  page,
  count,
  onPageChange,
}: {
  page: number
  count: number
  onPageChange: (page: number) => void
}) {
  const [area, areaRef] = useState<HTMLElement | null>(null)
  const trackRef = useRef<HTMLDivElement>(null)
  const busy = useRef(false)
  const timer = useRef<number | undefined>(undefined)
  const latest = useRef({ page, count, onPageChange })
  useEffect(() => {
    latest.current = { page, count, onPageChange }
  })

  const setOffset = useCallback((offset: string | null, animate: boolean) => {
    const el = trackRef.current
    if (!el) return
    el.style.transition = animate ? `transform ${SLIDE_MS}ms ${EASE}` : "none"
    el.style.transform = offset ? `translateX(${offset})` : ""
  }, [])

  // Whatever changed the page (a finished slide, a jump, a dismissed card), the new current page
  // now sits in the middle slot: drop the offset before painting.
  useLayoutEffect(() => {
    window.clearTimeout(timer.current)
    busy.current = false
    setOffset(null, false)
  }, [page, setOffset])
  useEffect(() => () => window.clearTimeout(timer.current), [])

  const go = useCallback(
    (delta: 1 | -1) => {
      const { page: current, count: total } = latest.current
      const target = current + delta
      if (busy.current || target < 0 || target >= total) return
      busy.current = true
      setOffset(delta > 0 ? "-100%" : "100%", true)
      timer.current = window.setTimeout(() => latest.current.onPageChange(target), SLIDE_MS)
    },
    [setOffset],
  )

  useEffect(() => {
    if (!area) return
    let drag: { x: number; y: number; dx: number; axis: "x" | "y" | null } | null = null

    // Moves and the release are taken on `window` in the capture phase, ahead of Base UI's drawer
    // (which listens on `document` and would otherwise claim a sideways drag as its own swipe).
    const onMove = (e: TouchEvent) => {
      const touch = e.touches[0]
      if (!drag || !touch || e.touches.length !== 1) return
      const dx = touch.clientX - drag.x
      const dy = touch.clientY - drag.y
      if (!drag.axis) {
        if (Math.abs(dx) < 6 && Math.abs(dy) < 6) return
        drag.axis = Math.abs(dx) > Math.abs(dy) ? "x" : "y"
        if (drag.axis === "x" && dx > 0 && latest.current.page === 0) {
          end()
          return
        }
      }
      if (drag.axis !== "x") return
      e.stopPropagation()
      const { page: current, count: total } = latest.current
      const atEdge = (dx > 0 && current === 0) || (dx < 0 && current === total - 1)
      drag.dx = atEdge ? dx * EDGE_RESISTANCE : dx
      setOffset(`${drag.dx}px`, false)
    }
    const onEnd = () => {
      const d = drag
      end()
      if (!d || d.axis !== "x") return
      const width = trackRef.current?.offsetWidth ?? 0
      const { page: current, count: total } = latest.current
      const target = current + (d.dx < 0 ? 1 : -1)
      if (Math.abs(d.dx) > Math.min(TURN_MAX_PX, width * TURN_FRACTION) && target >= 0 && target < total) {
        go(d.dx < 0 ? 1 : -1)
      } else {
        setOffset(null, true)
      }
    }
    const onCancel = () => {
      const claimed = drag?.axis === "x"
      end()
      if (claimed && !busy.current) setOffset(null, true)
    }
    const end = () => {
      drag = null
      window.removeEventListener("touchmove", onMove, true)
      window.removeEventListener("touchend", onEnd, true)
      window.removeEventListener("touchcancel", onCancel, true)
    }
    const onStart = (e: TouchEvent) => {
      const touch = e.touches[0]
      end()
      if (!touch || e.touches.length !== 1 || busy.current) return
      drag = { x: touch.clientX, y: touch.clientY, dx: 0, axis: null }
      window.addEventListener("touchmove", onMove, true)
      window.addEventListener("touchend", onEnd, true)
      window.addEventListener("touchcancel", onCancel, true)
    }
    area.addEventListener("touchstart", onStart, { passive: true })
    return () => {
      end()
      area.removeEventListener("touchstart", onStart)
    }
  }, [area, go, setOffset])

  /** Where each rendered page sits: the current one in flow, its neighbours one width to either
   * side (overflowing downward rather than stretching the current page's height). */
  const slotStyle = (index: number) =>
    index === page
      ? undefined
      : ({ position: "absolute", top: 0, left: index < page ? "-100%" : "100%", width: "100%" } as const)

  return { areaRef, trackRef, go, slotStyle }
}
