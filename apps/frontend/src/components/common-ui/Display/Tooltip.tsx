import { useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"

import { FONT_SIZE_MD, Z_OVERLAY_TOOLTIP } from "@/theme"

/** `'element'` anchors the bubble to the trigger's bounding box; `'cursor'` follows the mouse
 * pointer's live position instead. */
type Anchor = "element" | "cursor"

const GAP = 8
/** Grace period before the tooltip closes after the pointer leaves — long enough to cross `GAP`
 * without closing out from under a cursor still headed toward the bubble's own content. */
const CLOSE_DELAY_MS = 150

/** Hover/focus tooltip — a `title`-attribute replacement for icon-only buttons or richer content.
 * Debounces close, auto-flips to stay in viewport, and portals to `document.body`. */
export function Tooltip({
  label,
  children,
  anchor = "element",
  wrapperStyle,
  maxWidth = 320,
  zIndex = Z_OVERLAY_TOOLTIP,
}: {
  label: React.ReactNode
  children: React.ReactNode
  anchor?: Anchor
  /** Extra styles merged onto the trigger-wrapping `<span>`, e.g. flex sizing for a block-level
   * trigger. */
  wrapperStyle?: React.CSSProperties
  /** Overrides the bubble's default 320px width cap. */
  maxWidth?: number
  /** Overrides the default `Z_OVERLAY_TOOLTIP` — needed when the trigger lives inside something
   * with an even higher z-index (e.g. `Modal.tsx`'s 9001), or the portaled bubble renders behind it. */
  zIndex?: number
}) {
  const [visible, setVisible] = useState(false)
  const [style, setStyle] = useState<React.CSSProperties>({ position: "fixed", top: -9999, left: -9999, visibility: "hidden" })
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const wrapperRef = useRef<HTMLSpanElement>(null)
  const bubbleRef = useRef<HTMLDivElement>(null)
  const cursorPos = useRef<{ x: number; y: number } | null>(null)

  function cancelClose() {
    if (closeTimer.current !== null) {
      clearTimeout(closeTimer.current)
      closeTimer.current = null
    }
  }

  function scheduleClose() {
    cancelClose()
    closeTimer.current = setTimeout(() => setVisible(false), CLOSE_DELAY_MS)
  }

  function open() {
    cancelClose()
    setVisible(true)
  }

  function handleMouseMove(e: React.MouseEvent) {
    if (anchor !== "cursor") return
    cursorPos.current = { x: e.clientX, y: e.clientY }
    if (visible) recompute()
  }

  function recompute() {
    const bubble = bubbleRef.current
    if (!bubble) return
    const bubbleRect = bubble.getBoundingClientRect()
    // scrollHeight, not bubbleRect.height — a previous call's maxHeight cap would otherwise be measured back in.
    const naturalHeight = bubble.scrollHeight

    let anchorRect: { top: number; bottom: number; left: number; right: number; width: number }
    if (anchor === "cursor" && cursorPos.current) {
      const { x, y } = cursorPos.current
      anchorRect = { top: y, bottom: y, left: x, right: x, width: 0 }
    } else {
      const wrapper = wrapperRef.current
      if (!wrapper) return
      anchorRect = wrapper.getBoundingClientRect()
    }

    const spaceBelow = window.innerHeight - anchorRect.bottom
    const spaceAbove = anchorRect.top
    const fitsBelow = spaceBelow >= naturalHeight
    const fitsAbove = spaceAbove >= naturalHeight

    let top: number
    let maxHeight: number | undefined
    if (fitsBelow) {
      top = anchorRect.bottom + GAP
      maxHeight = undefined
    } else if (fitsAbove) {
      top = anchorRect.top - naturalHeight - GAP
      maxHeight = undefined
    } else {
      if (spaceBelow >= spaceAbove) {
        top = anchorRect.bottom + GAP
        maxHeight = Math.max(0, spaceBelow - GAP * 2)
      } else {
        top = GAP
        maxHeight = Math.max(0, spaceAbove - GAP * 2)
      }
    }

    let left = anchorRect.left
    left = Math.max(GAP, Math.min(left, window.innerWidth - bubbleRect.width - GAP))

    setStyle({
      position: "fixed",
      top,
      left,
      visibility: "visible",
      ...(maxHeight !== undefined ? { maxHeight, overflowY: "auto" } : { maxHeight: undefined, overflowY: undefined }),
    })
  }

  useLayoutEffect(() => {
    if (!visible) return
    recompute()
    window.addEventListener("scroll", recompute, true)
    window.addEventListener("resize", recompute)
    let cancelled = false
    void document.fonts?.ready?.then(() => {
      if (!cancelled) recompute()
    })
    return () => {
      cancelled = true
      window.removeEventListener("scroll", recompute, true)
      window.removeEventListener("resize", recompute)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, anchor])

  return (
    <span
      ref={wrapperRef}
      style={{ position: "relative", display: "inline-flex", ...wrapperStyle }}
      onMouseEnter={open}
      onMouseMove={handleMouseMove}
      onMouseLeave={scheduleClose}
      onFocus={open}
      onBlur={scheduleClose}
    >
      {children}
      {visible &&
        createPortal(
          <div
            role="tooltip"
            onMouseEnter={cancelClose}
            onMouseLeave={scheduleClose}
            onContextMenu={(e) => { e.stopPropagation() }}
            style={{ position: style.position, top: style.top, left: style.left, visibility: style.visibility, zIndex }}
          >
            <div
              className="swal2-popup"
              style={{
                display: "block",
                padding: "6px 10px",
                fontSize: FONT_SIZE_MD,
                lineHeight: 1.5,
                whiteSpace: "normal",
                maxWidth,
                textAlign: "left",
                borderRadius: 4,
                boxShadow: "0 2px 8px rgba(0,0,0,0.3)",
              }}
            >
              {/* marginRight/paddingRight claw back most of the parent's padding so the
                  scrollbar sits close to the popup's edge like elsewhere in the app. */}
              <div
                ref={bubbleRef}
                className="thin-scrollbar"
                style={{ maxHeight: style.maxHeight, overflowY: style.overflowY, marginRight: -8, paddingRight: 2 }}
              >
                {label}
              </div>
            </div>
          </div>,
          document.body,
        )}
    </span>
  )
}
