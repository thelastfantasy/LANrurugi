import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

/** Generic confirm/warning dialog using `dialog.tsx`'s `.swal2-popup`/`.swal2-actions` styling.
 * Owns no business copy — `message`/`confirmLabel`/`danger` are all caller-supplied. */
export function Confirm({
  message,
  icon = "fa-exclamation-triangle",
  onConfirm,
  onCancel,
  confirmLabel,
  cancelLabel,
  danger = false,
}: {
  message: ReactNode
  icon?: string
  onConfirm: () => void
  onCancel: () => void
  confirmLabel?: string
  cancelLabel?: string
  danger?: boolean
}) {
  const { t } = useTranslation()
  return (
    <>
      <div
        style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }}
        onClick={onCancel}
      />
      <div
        role="dialog"
        aria-modal="true"
        className="swal2-popup"
        style={{
          position: "fixed",
          top: "50%",
          left: "50%",
          transform: "translate(-50%, -50%)",
          zIndex: Z_OVERLAY_CONTENT,
          display: "block",
          width: 360,
          padding: 20,
          textAlign: "center",
          borderRadius: ".2em",
          boxShadow: "0 2px 10px rgba(0,0,0,.4)",
          animation: "confirm-pop-in 0.3s cubic-bezier(0.2, 0.9, 0.3, 1.2)",
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel()
          if (e.key === "Enter") onConfirm()
        }}
      >
        <i className={`fa ${icon} fa-2x${danger ? " confirm-danger-icon" : ""}`} aria-hidden="true"></i>
        <p style={{ fontWeight: "bold", margin: "12px 0" }}>{message}</p>
        <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
          <input type="button" className="stdbtn" value={cancelLabel ?? t("common.cancel") ?? undefined} onClick={onCancel} />
          <input
            type="button"
            // .stdbtn-danger, not an inline style — an inline style would beat .stdbtn:hover's rule.
            className={danger ? "stdbtn stdbtn-danger" : "stdbtn"}
            value={confirmLabel ?? t("common.ok") ?? undefined}
            onClick={onConfirm}
          />
        </div>
      </div>
    </>
  )
}
