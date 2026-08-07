import type { ReactNode } from "react"

import { IconButton } from "./IconButton"

/** Centered, fixed-position dialog — the boilerplate shared by every modal-style overlay in this
 * app (`AiSmartTankoubonModal.tsx`, `TankoubonEdit.tsx`'s own AI Smart Rename overlay): a dimmed
 * backdrop that closes on click, a `.id1`-styled box centered via a full-viewport flex container
 * (not `top/left: 50%` + `translate(-50%, -50%)` — that combination lands the box, and this round
 * close button riding on its corner, on a half-pixel offset whenever the viewport height or
 * `.id1`'s own content height is odd, which the browser can't cleanly anti-alias a circular border
 * against — a real, visibly jagged edge confirmed live; the box's own straight edges hid the same
 * sub-pixel offset much less noticeably. Flexbox centering has no such fractional-percentage step),
 * and a round `fa-times` `IconButton` floating just outside the box's own top-right corner (its
 * own bottom-left quarter overlapping the box's corner) — positioned via a negative offset from a
 * sibling wrapper `div`, rather than living inside `.id1` itself, since `.id1` has `overflowY:
 * auto` and an inset-positioned child would both get clipped/scrolled by that and sit visually
 * inset from the corner instead of straddling it. Uses `size={32}` (equal width/height, a true
 * circle once `borderRadius: "50%"` is applied — a non-square preset like `"large"` would render
 * an oval instead) and the `.modal-close-btn` class (one real rule per theme file, see those
 * files' own docs — solid theme-accent background + white icon + a 1px `box-shadow` ring in the
 * modal box's own background color, distinct from every other bordered/tinted `.stdbtn` on the
 * page since this button floats outside the modal box. A `box-shadow` ring, not a real `border` —
 * confirmed live that a real `border` on a `border-radius: 50%` circle sitting at a half-pixel
 * position (unavoidable whenever `.id1`'s content height and the viewport height have different
 * parity — real math, not a bug — see the centering note above) renders visibly jagged along the
 * curve; `box-shadow` doesn't participate in the same border-rasterization path and renders
 * cleanly at the same fractional position (also carries the drop-shadow that gives the button
 * depth against the backdrop, one `box-shadow` value doing double duty).
 *
 * The outer flex container itself has `pointerEvents: "none"` (it covers the full viewport, and
 * without this it would silently swallow clicks meant for the backdrop underneath); the inner
 * `position: relative` wrapper that actually holds the button + box re-enables `pointerEvents:
 * "auto"` since that's the part that should stay clickable.
 *
 * `onClose` is not called automatically on `Escape` — callers that want that can add their own
 * `keydown` listener; most of this app's modals so far haven't needed it.
 *
 * Doesn't render its own conditional (`{open && <Modal>...}`) — the caller still owns that, same
 * as before, so a caller that needs to run an effect only while open (e.g. firing the initial
 * data-fetching request) keeps full control of when the component (and its effects) mount. */
export function Modal({
  onClose,
  children,
  width = 560,
  textAlign = "center",
}: {
  onClose: () => void
  children: ReactNode
  width?: number
  textAlign?: "left" | "center"
}) {
  return (
    <>
      <div style={{ position: "fixed", inset: 0, background: "rgba(0,0,0,0.6)", zIndex: 9000 }} onClick={onClose} />
      <div
        style={{
          position: "fixed",
          inset: 0,
          zIndex: 9001,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          pointerEvents: "none",
        }}
      >
        <div style={{ position: "relative", pointerEvents: "auto" }}>
          <div style={{ position: "absolute", top: -16, right: -16 }}>
            <IconButton
              icon="fa fa-times"
              onClick={onClose}
              size={32}
              className="modal-close-btn"
              style={{ borderRadius: "50%", transform: "translateZ(0)" }}
            />
          </div>
          <div
            className="id1"
            style={{
              width,
              maxWidth: "95vw",
              maxHeight: "85vh",
              overflowY: "auto",
              padding: 24,
              textAlign,
            }}
          >
            {children}
          </div>
        </div>
      </div>
    </>
  )
}
