import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"

import { FONT_SIZE_MD, Z_OVERLAY_TOOLTIP } from "@/theme"

/** `'element'` anchors the bubble to the trigger's bounding box; `'cursor'` follows the mouse
 * pointer's live position instead. */
type Anchor = "element" | "cursor"

const GAP = 8
/** Grace period before the tooltip closes after the pointer leaves — long enough to cross `GAP`
 * without closing out from under a cursor still headed toward the bubble's own content. */
const CLOSE_DELAY_MS = 150

/** How long a touch has to be held before its tooltip opens. A tap must stay a tap: a bubble that
 * opened on `mouseover` (which a tap synthesizes) covered the queue row it belonged to, so a
 * finished download's title could not be opened on a phone (reported 2026-11-25). */
const LONG_PRESS_MS = 500

/** The hover tooltip currently showing, module-wide. Opening another one closes this one right
 * away instead of letting it sit next to the new bubble for the rest of its close delay. */
let activeTooltip: { close: () => void } | null = null

/** Hover/focus tooltip — a `title`-attribute replacement for icon-only buttons or richer content.
 * Debounces close, auto-flips to stay in viewport, and portals to `document.body`. */
const HIDDEN_STYLE: React.CSSProperties = { position: "fixed", top: -9999, left: -9999, visibility: "hidden" }

export function Tooltip({
  label,
  children,
  anchor = "element",
  wrapperStyle,
  maxWidth = 320,
  zIndex = Z_OVERLAY_TOOLTIP,
  closeDelay = CLOSE_DELAY_MS,
}: {
  label: React.ReactNode
  children: React.ReactNode
  anchor?: Anchor
  /** Extra styles merged onto the trigger-wrapping `<span>`, e.g. flex sizing for a block-level
   * trigger. */
  wrapperStyle?: React.CSSProperties
  /** Overrides the bubble's default 320px width cap; any CSS length, so a caller can widen it by a
   *  character or two in `rem` without redoing the pixel arithmetic. */
  maxWidth?: number | string
  /** Overrides the default `Z_OVERLAY_TOOLTIP` — needed when the trigger lives inside something
   * with an even higher z-index (e.g. `Modal.tsx`'s 9001), or the portaled bubble renders behind it. */
  zIndex?: number
  /** How long the tooltip stays open after the pointer leaves the trigger, in ms. Set to 0 for
   * immediate close. */
  closeDelay?: number
}) {
  const [visible, setVisible] = useState(false)
  const [style, setStyle] = useState<React.CSSProperties>(HIDDEN_STYLE)
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  /** This instance's entry in `activeTooltip`; `close` mirrors `cancelClose` + hide. */
  const self = useRef<{ close: () => void }>({ close: () => {} })
  self.current.close = () => {
    cancelClose()
    hide()
  }

  /** Back off-screen as well as invisible, so a reopen never starts at the last position. */
  function hide() {
    setVisible(false)
    setStyle(HIDDEN_STYLE)
    touchOpened.current = false
  }
  const wrapperRef = useRef<HTMLSpanElement>(null)
  const bubbleRef = useRef<HTMLDivElement>(null)
  /** The portaled bubble box, for "is this touch outside both?" comparisons. */
  const bubbleBoxRef = useRef<HTMLDivElement>(null)
  /** The kind of pointer the trigger last saw, so a tap's synthesized `mouseover` cannot open the
   * bubble behind the user's finger. */
  const pointerType = useRef("mouse")
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const pressOrigin = useRef<{ x: number; y: number } | null>(null)
  /** A long press opened the bubble: swallow the click it ends with, and the leave right after it. */
  const swallowClick = useRef(false)
  const swallowLeave = useRef(false)
  /** Whether the open bubble came from a long press, which is dismissed by tapping outside. */
  const touchOpened = useRef(false)
  const cursorPos = useRef<{ x: number; y: number } | null>(null)

  function cancelClose() {
    if (closeTimer.current !== null) {
      clearTimeout(closeTimer.current)
      closeTimer.current = null
    }
  }

  function forgetActive() {
    if (activeTooltip === self.current) activeTooltip = null
  }

  function scheduleClose() {
    cancelClose()
    closeTimer.current = setTimeout(() => {
      closeTimer.current = null
      forgetActive()
      hide()
    }, closeDelay)
  }

  function open() {
    cancelClose()
    if (activeTooltip !== null && activeTooltip !== self.current) {
      const previous = activeTooltip
      activeTooltip = null
      previous.close()
    }
    activeTooltip = self.current
    setVisible(true)
  }

  function cancelLongPress() {
    if (longPressTimer.current !== null) {
      clearTimeout(longPressTimer.current)
      longPressTimer.current = null
    }
    pressOrigin.current = null
  }

  /** Only a mouse opens on hover; a finger opens the bubble by holding still instead. */
  function openFromHover() {
    if (pointerType.current !== "mouse") return
    open()
  }

  function handlePointerDown(e: React.PointerEvent) {
    pointerType.current = e.pointerType || "mouse"
    // A press that never produced its click (the finger slid away) must not eat a later one.
    swallowClick.current = false
    swallowLeave.current = false
    cancelLongPress()
    if (pointerType.current === "mouse") return
    pressOrigin.current = { x: e.clientX, y: e.clientY }
    longPressTimer.current = setTimeout(() => {
      longPressTimer.current = null
      swallowClick.current = true
      swallowLeave.current = true
      touchOpened.current = true
      open()
    }, LONG_PRESS_MS)
  }

  function handlePointerMove(e: React.PointerEvent) {
    pointerType.current = e.pointerType || pointerType.current
    if (pressOrigin.current) {
      const dx = e.clientX - pressOrigin.current.x
      const dy = e.clientY - pressOrigin.current.y
      // Scrolling or dragging the row is not a long press.
      if (dx * dx + dy * dy > 100) cancelLongPress()
    }
    handleMouseMove(e)
  }

  function handleLeave() {
    if (swallowLeave.current) {
      swallowLeave.current = false
      return
    }
    scheduleClose()
  }

  function handleMouseMove(e: React.MouseEvent) {
    if (anchor !== "cursor") return
    cursorPos.current = { x: e.clientX, y: e.clientY }
    if (visible) recompute()
  }

  function recompute() {
    const bubble = bubbleRef.current
    if (!bubble) return
    // The outer popup, padding and border included: the inner box alone let it overrun the right edge.
    const bubbleRect = (bubble.parentElement ?? bubble).getBoundingClientRect()
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

  useEffect(
    () => () => {
      if (activeTooltip === self.current) activeTooltip = null
    },
    [],
  )

  // A bubble opened by a long press has no pointer-leave to close it: the finger is already where
  // it opened it. Tap anywhere outside — the row, the bubble's own text excepted — to dismiss.
  useEffect(() => {
    if (!visible || !touchOpened.current) return
    function onOutside(event: PointerEvent) {
      const target = event.target as Node
      if (wrapperRef.current?.contains(target) || bubbleBoxRef.current?.contains(target)) return
      hide()
      if (activeTooltip === self.current) activeTooltip = null
    }
    document.addEventListener("pointerdown", onOutside, true)
    return () => document.removeEventListener("pointerdown", onOutside, true)
  }, [visible])

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
      style={{
        position: "relative",
        display: "inline-flex",
        // A long press on touch must reach this component, not iOS's link preview/magnifier.
        WebkitTouchCallout: "none",
        ...wrapperStyle,
      }}
      onPointerDown={handlePointerDown}
      onPointerUp={cancelLongPress}
      onPointerCancel={cancelLongPress}
      onPointerMove={handlePointerMove}
      onPointerLeave={handleLeave}
      onMouseEnter={openFromHover}
      // Every move over the trigger (or the bubble, a React child through the portal) cancels a
      // pending close. `mouseenter` alone missed one path: the pointer falling from the bubble back
      // onto the trigger — e.g. when the bubble is repositioned out from under a resting pointer —
      // which React does not count as entering, so the bubble's own leave closed it.
      onMouseOver={openFromHover}
      onMouseLeave={handleLeave}
      // A long press ends with the platform's own menu; the bubble replaces it on touch.
      onContextMenu={(e) => {
        if (pointerType.current !== "mouse") e.preventDefault()
      }}
      // The long press that opened the bubble must not also follow the link underneath it.
      onClickCapture={(e) => {
        if (!swallowClick.current) return
        swallowClick.current = false
        e.preventDefault()
        e.stopPropagation()
      }}
      // Keyboard focus only: focus handed back by a closing drawer or dialog after a tap or click
      // must not pop the bubble open on its own.
      onFocus={(e) => {
        if ((e.target as Element).matches?.(":focus-visible")) open()
      }}
      onBlur={scheduleClose}
    >
      {children}
      {visible &&
        createPortal(
          <div
            ref={bubbleBoxRef}
            role="tooltip"
            onMouseEnter={cancelClose}
            onMouseLeave={(e) => {
              // Back onto the trigger is not leaving.
              if (e.relatedTarget instanceof Node && wrapperRef.current?.contains(e.relatedTarget)) return
              scheduleClose()
            }}
            onContextMenu={(e) => { e.stopPropagation() }}
            style={{ position: style.position, top: style.top, left: style.left, visibility: style.visibility, zIndex }}
          >
            <div
              className="swal2-popup"
              style={{
                display: "block",
                padding: "6px 10px", // + 1px border: 22px across, in the viewport cap below
                fontSize: FONT_SIZE_MD,
                lineHeight: 1.5,
                whiteSpace: "normal",
                // Sized by its content, not by the room left of where it was last placed: a fixed
                // box shrinks to fit the viewport from its `left`, so near the right edge it was
                // measured squeezed and then kept that squeezed width once moved back into view.
                width: "max-content",
                maxWidth: `min(${typeof maxWidth === "number" ? `${maxWidth}px` : maxWidth}, calc(100vw - ${GAP * 2 + 22}px))`,
                textAlign: "left",
                borderRadius: 4,
                boxShadow: "0 2px 8px rgba(0,0,0,0.3)",
              }}
            >
              {/* Only claw back the parent's right padding when the bubble is actually
                  height-capped/scrollable; otherwise the 10px outer padding should stay
                  symmetric on both sides (the old unconditional -8/+2 made every short
                  tooltip's right padding 4px while the left stayed 10px). */}
              <div
                ref={bubbleRef}
                className="thin-scrollbar"
                style={{
                  maxHeight: style.maxHeight,
                  overflowY: style.overflowY,
                  ...(style.maxHeight !== undefined
                    ? { marginRight: -8, paddingRight: 2 }
                    : {}),
                }}
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
