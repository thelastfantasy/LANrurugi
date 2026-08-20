import { useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"

import { FONT_SIZE_MD, Z_OVERLAY_TOOLTIP } from "@/theme"

/** `'element'` (default) anchors the bubble to the trigger's own bounding box, in a fixed spot
 * relative to it — the usual "attached to this button" tooltip. `'cursor'` instead follows the
 * mouse pointer's live position (updated on every `mousemove` over the trigger) — useful for a
 * trigger the pointer can move around inside of (e.g. a wide text block) where "always in the
 * same spot" feels disconnected from what's actually being hovered. */
type Anchor = "element" | "cursor"

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
  /** Extra styles merged onto the trigger-wrapping `<span>` — most callers don't need this (the
   * default `inline-flex` is right for wrapping a button/checkbox), but a caller wrapping a
   * block-level element that itself needs to participate in a parent flex layout (e.g.
   * `flex: '1 1 180px'`) can pass that through here instead of adding an extra wrapping element
   * of their own just to carry the flex sizing. */
  wrapperStyle?: React.CSSProperties
  /** Overrides the bubble's default 320px cap — for content that reads worse wrapped that narrow
   * (e.g. a multi-column list of keyboard shortcuts). */
  maxWidth?: number
  /** Overrides the default `Z_OVERLAY_TOOLTIP` (1100) — needed when the trigger itself lives
   * inside something that already clears 1100 on its own, e.g. `Modal.tsx`'s hardcoded
   * `zIndex: 9001` (that component's own docs on why it needs to sit above legacy's `.base-overlay`
   * — see `Z_OVERLAY_ABOVE_LEGACY_MODAL`'s own docs in `theme.ts` for the same "clear legacy's
   * 9000 tier" reasoning). Without this, a `Tooltip` triggered from inside such a modal renders its
   * portaled bubble at the DOM's very end but still loses the stacking fight to the modal's own
   * much higher z-index, appearing to render *behind* it instead of on top. */
  zIndex?: number
}) {
  const [visible, setVisible] = useState(false)
  // `position: 'fixed'` (not just `visibility: 'hidden'`) from the very first render — a bubble
  // that's still `static` when `recompute()` first measures it lays out inline in the normal
  // document flow (portaled to the end of `<body>`), reporting a different width/wrapping than
  // its eventual `position: fixed` box, especially before web fonts finish loading. Parking
  // off-screen at a fixed position from the start means the first measurement is already accurate.
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
    // The bubble's *unconstrained* height — once a previous `recompute()` call has already
    // applied a `maxHeight` cap (see the `cappedAbove` case below), `bubbleRect.height` itself
    // reads back as the *capped* height, not the content's real size, since `getBoundingClientRect`
    // reflects the box as CSS `max-height` has already clamped it. Using that capped value as this
    // call's own "does it fit" measurement made the bubble spuriously "fit" against the very space
    // its cap was sized to (confirmed live: the second `recompute()` this component's own
    // `document.fonts.ready` callback triggers was reading the first call's capped height, deciding
    // it now fit `spaceAbove` after all, and clearing the cap/scrollbar it had just set). `scrollHeight`
    // is unaffected by `max-height`/`overflow` — it's always the content's actual full height,
    // capped or not.
    const naturalHeight = bubble.scrollHeight

    // The point/box the bubble is placed relative to — a zero-size point at the cursor in
    // `'cursor'` mode (falling back to the trigger's bounds if no pointer position is known yet,
    // e.g. opened via keyboard focus), or the trigger's full bounds in `'element'` mode.
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
      // Preferred/default side, unconstrained — matches the original two-side preference order.
      top = anchorRect.bottom + GAP
      maxHeight = undefined
    } else if (fitsAbove) {
      top = anchorRect.top - naturalHeight - GAP
      maxHeight = undefined
    } else {
      // Neither side has room for the bubble at its natural height — pick whichever has more
      // space (below wins ties, matching the preferred side above) and cap the bubble's height to
      // what that side actually has, with a scrollbar for the rest, rather than either picking a
      // side and letting it overflow the viewport (the original bug report: a tooltip opening
      // downward near the bottom edge ran off-screen with no way to reach its own lower content)
      // or assuming only the "opens upward" direction could ever be cramped (a later, narrower
      // version of this fix only handled that one case, which turned out to be the wrong
      // asymmetry — a downward-opening tooltip near the bottom edge hits the exact same problem).
      if (spaceBelow >= spaceAbove) {
        top = anchorRect.bottom + GAP
        maxHeight = Math.max(0, spaceBelow - GAP * 2)
      } else {
        top = GAP
        maxHeight = Math.max(0, spaceAbove - GAP * 2)
      }
    }

    // Left-aligned to the anchor's own left edge (not centered) — keeps the bubble visually
    // anchored to where the trigger text/icon actually starts.
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
    // Web fonts can still be swapping in after this first `recompute()` ran, especially on a
    // cold page load — `document.fonts.ready` resolves once that settles, so this re-measures
    // with the bubble's real, final glyph metrics.
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
          // Plain positioning shell — `top`/`left`/`position: fixed` only. Height-capping +
          // scrolling live on `.swal2-popup` below instead (see that element's own `ref`/style),
          // not here: this element has no border/background/radius of its own, so a scrollbar on
          // *it* would render outside the popup's visible border — confirmed live: the very first
          // version of the height-cap fix put `maxHeight`/`overflowY` here, and the resulting
          // scrollbar sat flush with the portal's own edge, outside the rounded border the popup
          // actually draws, looking like a stray scrollbar floating next to the tooltip rather
          // than part of it.
          <div
            role="tooltip"
            onMouseEnter={cancelClose}
            onMouseLeave={scheduleClose}
            onContextMenu={(e) => { e.stopPropagation() }}
            style={{ position: style.position, top: style.top, left: style.left, visibility: style.visibility, zIndex }}
          >
            {/* `swal2-popup` (not an actual SweetAlert2 dialog — just its class name) — every
                legacy theme already styles it for this "informational popup" role, so the tooltip
                matches whichever theme is active with zero hardcoded color. Nested one level in
                so this element's own inline `fontSize` doesn't fight `lrr.css`'s
                `.swal2-popup { font-size: 9pt !important }` (a plain style prop can't win against
                `!important`, but a size on a child the outer rule doesn't target isn't in that
                fight at all).
                This element itself no longer scrolls — see the *next* nested `div`'s own comment
                for why an `overflow-y: auto` element needs its border-radius'd ancestor to stay a
                plain, non-scrolling box: with `overflowY`/`thin-scrollbar` here directly (an
                earlier version of this fix), the scrollbar track sat flush against this element's
                own rounded corners, visually clipping/covering them at the top and bottom — a
                border-radius doesn't automatically clip a same-element scrollbar the way it clips
                overflowing *content*. `ref`/`maxHeight`/`getBoundingClientRect` measurements in
                `recompute()` stay on the *inner* scrolling div now (see below), not this one. */}
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
              {/* The actual scrolling element — nested one level *inside* the rounded/padded
                  `.swal2-popup` box above so the scrollbar track clears the rounded corners (see
                  that element's own comment). `marginRight`/`paddingRight` claw back most of the
                  parent's own 10px right padding — pushing the scrollbar itself close to the
                  popup's edge (matching every other scrollbar in this app, which sits at its
                  container's own edge, not visibly indented from it) while keeping a small,
                  deliberate gap so the track still clears the rounded border rather than
                  reproducing the very "flush against the corner" look this nesting fixed. */}
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
