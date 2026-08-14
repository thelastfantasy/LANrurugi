import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

/** Generic confirm/warning dialog — the `.swal2-popup`/`.swal2-actions` styling `dialog.tsx`'s own
 * `confirmDialog()` already established (real classes, themed per theme file — see those files'
 * own `.swal2-popup` rules — not a hardcoded white box/gray border the way an earlier,
 * from-scratch version of this dialog did), packaged as a reusable component so a caller with its
 * own two-button confirm flow (e.g. `DeleteConfirmDialog.tsx`) doesn't have to hand-roll the same
 * backdrop/box/button-row markup again. Doesn't own any business copy (no "delete" wording baked
 * in) — `message`, `confirmLabel`, and `danger` are all caller-supplied, same generic-component
 * shape `Modal.tsx` uses.
 *
 * Plays a scale-up entrance animation on mount (`confirm-pop-in`, defined once in `index.css` and
 * shared with `dialog.tsx`'s own `confirmDialog()`/`promptDialog()` render — see that keyframe's
 * own comment for why it's not colocated here). CSS `animation`, not a JS transition library,
 * since this is a one-shot mount-triggered effect with no state to coordinate. */
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
            // `.stdbtn-danger`, not an inline `style={{ background: '#d33', ... }}` — a real class
            // per theme file (see g.css's own comment on this class), since an inline style always
            // wins over `.stdbtn:hover`'s own CSS rule and silently kills the hover effect
            // entirely (a real, live-reported bug: the earlier inline-style version of this button
            // never visibly changed color on hover).
            className={danger ? "stdbtn stdbtn-danger" : "stdbtn"}
            value={confirmLabel ?? t("common.ok") ?? undefined}
            onClick={onConfirm}
          />
        </div>
      </div>
    </>
  )
}
