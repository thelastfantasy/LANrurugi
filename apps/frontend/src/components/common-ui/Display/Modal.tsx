import type { ReactNode } from "react"

import { IconButton } from "@/components/common-ui/Form"

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
        <div style={{ position: "relative", pointerEvents: "auto", animation: "modal-pop-in 0.3s cubic-bezier(0.2, 0.9, 0.3, 1.2)" }}>
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
