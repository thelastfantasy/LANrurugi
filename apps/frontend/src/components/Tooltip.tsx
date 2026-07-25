import { useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'

import { FONT_SIZE_9PT, Z_OVERLAY_TOOLTIP } from '../theme'

/** `'element'` (default) anchors the bubble to the trigger's own bounding box, in a fixed spot
 * relative to it — the usual "attached to this button" tooltip. `'cursor'` instead follows the
 * mouse pointer's live position (updated on every `mousemove` over the trigger) — useful for a
 * trigger the pointer can move around inside of (e.g. a wide text block) where "always in the
 * same spot" feels disconnected from what's actually being hovered. */
type Anchor = 'element' | 'cursor'

const GAP = 8
/** Grace period between the pointer leaving the trigger/bubble and the tooltip actually closing —
 * long enough to cross the `GAP` above without the bubble closing out from under a cursor still
 * headed toward its content (e.g. a link inside the tooltip). */
const CLOSE_DELAY_MS = 150

/** Hover/focus tooltip — a `title`-attribute replacement for cases (icon-only buttons, or content
 * richer than plain text) where a plain browser tooltip's styling/timing/interactivity isn't
 * enough. `label` accepts any node so a caller can render a multi-line body.
 *
 * Four things a naive hover-only tooltip gets wrong, fixed here:
 * - Closing the instant the pointer leaves the trigger makes the bubble's content (e.g. a link)
 *   unreachable before the pointer crosses the gap to it. Debounces the close with a short grace
 *   period; the bubble's own `onMouseEnter`/`onMouseLeave` feed the same timer, so moving onto the
 *   bubble cancels the pending close.
 * - A fixed "always above, centered" position overflows the viewport near an edge. This measures
 *   the anchor's (trigger's or cursor's, see `anchor`) and bubble's real bounding boxes in
 *   viewport pixel coordinates and picks whichever side has room, preferring below.
 * - Rendering the bubble as a normal DOM child of the trigger means any ancestor's
 *   `overflow: hidden`/`scroll` clips it, and any ancestor with a CSS `transform` breaks a
 *   `position: fixed` bubble's viewport-relative coordinates. `createPortal` renders it as a
 *   direct child of `document.body` instead.
 * - `anchor` lets a caller opt into cursor-following placement instead of the default
 *   fixed-to-the-trigger-element placement.
 */
export default function Tooltip({
  label,
  children,
  anchor = 'element',
  wrapperStyle,
}: {
  label: React.ReactNode
  children: React.ReactNode
  anchor?: Anchor
  /** Extra styles merged onto the trigger-wrapping `<span>` — most callers don't need this (the
   * default `inline-flex` is right for wrapping a button/checkbox), but a caller wrapping a
   * block-level element that itself needs to participate in a parent flex layout (e.g.
   * `flex: '1 1 180px'`) can pass that through here instead of adding an extra wrapping element
   * of their own just to carry the flex sizing. */
  wrapperStyle?: React.CSSProperties
}) {
  const [visible, setVisible] = useState(false)
  // `position: 'fixed'` (not just `visibility: 'hidden'`) from the very first render — a bubble
  // that's still `static` when `recompute()` first measures it lays out inline in the normal
  // document flow (portaled to the end of `<body>`), reporting a different width/wrapping than
  // its eventual `position: fixed` box, especially before web fonts finish loading. Parking
  // off-screen at a fixed position from the start means the first measurement is already accurate.
  const [style, setStyle] = useState<React.CSSProperties>({ position: 'fixed', top: -9999, left: -9999, visibility: 'hidden' })
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
    if (anchor !== 'cursor') return
    cursorPos.current = { x: e.clientX, y: e.clientY }
    if (visible) recompute()
  }

  function recompute() {
    const bubble = bubbleRef.current
    if (!bubble) return
    const bubbleRect = bubble.getBoundingClientRect()

    // The point/box the bubble is placed relative to — a zero-size point at the cursor in
    // `'cursor'` mode (falling back to the trigger's bounds if no pointer position is known yet,
    // e.g. opened via keyboard focus), or the trigger's full bounds in `'element'` mode.
    let anchorRect: { top: number; bottom: number; left: number; right: number; width: number }
    if (anchor === 'cursor' && cursorPos.current) {
      const { x, y } = cursorPos.current
      anchorRect = { top: y, bottom: y, left: x, right: x, width: 0 }
    } else {
      const wrapper = wrapperRef.current
      if (!wrapper) return
      anchorRect = wrapper.getBoundingClientRect()
    }

    const spaceBelow = window.innerHeight - anchorRect.bottom
    const top =
      spaceBelow >= bubbleRect.height || anchorRect.top < bubbleRect.height
        ? anchorRect.bottom + GAP
        : anchorRect.top - bubbleRect.height - GAP

    // Left-aligned to the anchor's own left edge (not centered) — keeps the bubble visually
    // anchored to where the trigger text/icon actually starts.
    let left = anchorRect.left
    left = Math.max(GAP, Math.min(left, window.innerWidth - bubbleRect.width - GAP))

    setStyle({ position: 'fixed', top, left, visibility: 'visible' })
  }

  useLayoutEffect(() => {
    if (!visible) return
    recompute()
    window.addEventListener('scroll', recompute, true)
    window.addEventListener('resize', recompute)
    // Web fonts can still be swapping in after this first `recompute()` ran, especially on a
    // cold page load — `document.fonts.ready` resolves once that settles, so this re-measures
    // with the bubble's real, final glyph metrics.
    let cancelled = false
    void document.fonts?.ready?.then(() => {
      if (!cancelled) recompute()
    })
    return () => {
      cancelled = true
      window.removeEventListener('scroll', recompute, true)
      window.removeEventListener('resize', recompute)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible, anchor])

  return (
    <span
      ref={wrapperRef}
      style={{ position: 'relative', display: 'inline-flex', ...wrapperStyle }}
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
            ref={bubbleRef}
            role="tooltip"
            onMouseEnter={cancelClose}
            onMouseLeave={scheduleClose}
            style={{ ...style, zIndex: Z_OVERLAY_TOOLTIP }}
          >
            {/* `swal2-popup` (not an actual SweetAlert2 dialog — just its class name) — every
                legacy theme already styles it for this "informational popup" role, so the tooltip
                matches whichever theme is active with zero hardcoded color. Nested one level in
                so this element's own inline `fontSize` doesn't fight `lrr.css`'s
                `.swal2-popup { font-size: 9pt !important }` (a plain style prop can't win against
                `!important`, but a size on a child the outer rule doesn't target isn't in that
                fight at all). */}
            <div
              className="swal2-popup"
              style={{
                display: 'block',
                padding: '6px 10px',
                fontSize: FONT_SIZE_9PT,
                lineHeight: 1.5,
                whiteSpace: 'normal',
                maxWidth: 320,
                textAlign: 'left',
                borderRadius: 4,
                boxShadow: '0 2px 8px rgba(0,0,0,0.3)',
              }}
            >
              {label}
            </div>
          </div>,
          document.body,
        )}
    </span>
  )
}
