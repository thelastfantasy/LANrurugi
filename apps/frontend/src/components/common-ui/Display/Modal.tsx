import type { ReactNode } from "react"
import { useEffect, useState } from "react"
import { createPortal } from "react-dom"

import { IconButton } from "@/components/common-ui/Form"
import { FLOATING_POPUP_SHADOW } from "@/theme"

/** Room on every side for the close button, which sits 16px outside the box's corner: a 95vw box
 * left about 10px on a phone and cut the button off. `dvh` is the visible height, so the browser's
 * own toolbar does not push the box under the top edge. */
const MAX_WIDTH = "calc(100vw - 40px)"
const MAX_HEIGHT = "min(85vh, calc(100dvh - 40px))"

/** Open modals, counted so a stacked modal closing does not unlock the page under the one below. */
let openModals = 0
let savedOverflow = ""

/** Keeps the page behind an open modal from scrolling: a wheel or swipe over the backdrop, or past
 * the end of the modal's own content, otherwise scrolls the page. `html`'s `scrollbar-gutter:
 * stable` (index.css) keeps the layout from shifting when the scrollbar goes. */
function useBodyScrollLock() {
  useEffect(() => {
    if (openModals++ === 0) {
      savedOverflow = document.body.style.overflow
      document.body.style.overflow = "hidden"
    }
    return () => {
      if (--openModals === 0) document.body.style.overflow = savedOverflow
    }
  }, [])
}

/** Centered, fixed-position dialog — dimmed backdrop, `.id1`-styled box, round `fa-times` close
 * button. Doesn't render its own `{open && <Modal>}` conditional — the caller owns that. */
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
  useBodyScrollLock()
  // A modal opened on top of another one keeps a backdrop (a click outside still closes it) but does
  // not dim again: two stacked 60% backdrops turned the modal underneath nearly black. A shadow sets
  // it apart from the modal below instead.
  const [stacked] = useState(() => openModals > 0)
  // Rendered into `body` rather than where it was declared. A fixed-position overlay still obeys an
  // ancestor's `display: none`, `overflow` or `transform`, so a modal opened from inside a collapsible
  // panel vanished — or collapsed to zero width — when that panel was closed.
  return createPortal(
    <>
      <div
        style={{ position: "fixed", inset: 0, background: stacked ? "transparent" : "rgba(0,0,0,0.6)", zIndex: 9000 }}
        onClick={onClose}
      />
      <div
        // Marks the layer for anything portalled out of a dialog's contents but needing to sit above
        // it — `Select` reads this to lift its own popup (see its own docs).
        data-overlay="modal"
        style={{
          position: "fixed",
          inset: 0,
          zIndex: 9001,
          display: "flex",
          // `flex-start` + the dialog's own `margin: auto` instead of `align-items: center`: a dialog
          // taller than the viewport (the diagnostics panel on a phone) lost its top and bottom to
          // centring, with nothing to scroll — the content was simply unreachable.
          alignItems: "flex-start",
          justifyContent: "center",
          overflowY: "auto",
          padding: "24px 12px",
          pointerEvents: "none",
        }}
      >
        {/* The width cap lives here too: on a phone index.css forces `div.id1` to `max-width: 100%`,
            which resolves against this shrink-to-fit wrapper and let the box run off screen. */}
        <div
          style={{
            position: "relative",
            maxWidth: MAX_WIDTH,
            margin: "auto",
            pointerEvents: "auto",
            animation: "modal-pop-in 0.3s cubic-bezier(0.2, 0.9, 0.3, 1.2)",
          }}
        >
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
              maxWidth: MAX_WIDTH,
              maxHeight: MAX_HEIGHT,
              overflowY: "auto",
              overscrollBehavior: "contain",
              boxShadow: stacked ? FLOATING_POPUP_SHADOW : undefined,
              padding: 24,
              textAlign,
            }}
          >
            {children}
          </div>
        </div>
      </div>
    </>,
    document.body,
  )
}
