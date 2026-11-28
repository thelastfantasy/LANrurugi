import type { CSSProperties, PointerEvent } from "react"
import { useRef, useState } from "react"

/** Where the unfolded copy goes: the folded title's own box, in its offset parent's coordinates,
 *  on the first opaque background behind the region so it matches every theme and highlight. */
export interface FullTitleOverlay {
  bg: string
  left: number
  top: number
  width: number
}

/** The title itself, or any line inside it — a block of one-line ellipsised lines is cut off when
 *  one of them is. */
function isTruncated(el: HTMLElement) {
  return [el, ...el.querySelectorAll<HTMLElement>("*")].some(
    (node) => node.scrollHeight > node.clientHeight + 1 || node.scrollWidth > node.clientWidth + 1,
  )
}

function opaqueBackground(el: Element | null): string {
  for (let node = el; node; node = node.parentElement) {
    const bg = getComputedStyle(node).backgroundColor
    if (bg && bg !== "transparent" && !/^rgba\(.*,\s*0\)$/.test(bg)) return bg
  }
  return getComputedStyle(document.body).backgroundColor
}

/** A title cut off by a line clamp or an ellipsis is shown whole, laid over what follows it. Put
 *  `titleRef` on the folded title and render the overlay while it is non-null. `handlers` is the
 *  library card's trigger — hover (mouse) or press (touch) anywhere on the region but `ignore`;
 *  a caller with its own trigger calls `open`/`hide` instead. */
export function useFullTitle<T extends HTMLElement>(ignore?: string) {
  const titleRef = useRef<T | null>(null)
  const [overlay, setOverlay] = useState<FullTitleOverlay | null>(null)

  /** Unfolds the title if it is cut off; `region` is what the overlay's background is read from.
   *  Returns whether the title is (now) unfolded. */
  function open(region: HTMLElement) {
    const title = titleRef.current
    if (!title) return false
    if (overlay !== null) return true
    if (!isTruncated(title)) return false
    setOverlay({
      bg: opaqueBackground(region),
      left: title.offsetLeft,
      top: title.offsetTop,
      width: title.getBoundingClientRect().width,
    })
    return true
  }

  function show(e: PointerEvent<HTMLElement>) {
    if (ignore && (e.target as Element).closest(ignore)) {
      setOverlay(null)
      return
    }
    open(e.currentTarget)
  }
  const hide = () => setOverlay(null)

  const handlers = {
    onPointerMove: (e: PointerEvent<HTMLElement>) => {
      if (e.pointerType === "mouse") show(e)
    },
    onPointerLeave: (e: PointerEvent<HTMLElement>) => {
      if (e.pointerType === "mouse") hide()
    },
    onPointerDown: (e: PointerEvent<HTMLElement>) => {
      if (e.pointerType !== "mouse") show(e)
    },
    onPointerUp: (e: PointerEvent<HTMLElement>) => {
      if (e.pointerType !== "mouse") hide()
    },
    onPointerCancel: hide,
  }
  return { titleRef, overlay, handlers, open, hide }
}

/** The overlay box itself; the caller adds the title's own typography. */
export function fullTitleOverlayStyle(overlay: FullTitleOverlay): CSSProperties {
  return {
    position: "absolute",
    top: overlay.top,
    left: overlay.left,
    width: overlay.width,
    boxSizing: "border-box",
    zIndex: 2,
    display: "block",
    background: overlay.bg,
    pointerEvents: "none",
  }
}
