import { type ReactNode, useEffect, useRef, useState } from "react"

import { BottomSheet } from "./BottomSheet"

/** Phone-width form of a confirmation: the message over one full-width confirm button, with the
 * sheet's own bar as cancel. The answer is reported once the sheet has slid away, so the caller
 * can unmount it in that callback without cutting the animation short. */
export function ConfirmSheet({
  message,
  icon = "fa-exclamation-triangle",
  danger = false,
  confirmLabel,
  cancelLabel,
  confirmDisabled = false,
  children,
  onConfirm,
  onCancel,
}: {
  message: ReactNode
  icon?: string | null
  danger?: boolean
  confirmLabel: ReactNode
  cancelLabel: ReactNode
  confirmDisabled?: boolean
  /** Extra controls between the message and the button (e.g. a typed confirmation). */
  children?: ReactNode
  onConfirm: () => void
  onCancel: () => void
}) {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    const frame = requestAnimationFrame(() => setOpen(true))
    return () => cancelAnimationFrame(frame)
  }, [])
  const confirmed = useRef(false)
  return (
    <BottomSheet
      open={open}
      onClose={() => setOpen(false)}
      onExited={() => (confirmed.current ? onConfirm() : onCancel())}
      cancelLabel={cancelLabel}
    >
      <div style={{ textAlign: "center", padding: "14px 4px 16px" }}>
        {icon && (
          <i className={`fa ${icon} fa-2x${danger ? " confirm-danger-icon" : ""}`} aria-hidden="true"></i>
        )}
        <div style={{ fontWeight: "bold", fontSize: 15, marginTop: icon ? 12 : 0 }}>{message}</div>
      </div>
      {children}
      <button
        type="button"
        disabled={confirmDisabled}
        // `.stdbtn-danger` carries each theme's destructive colors; layout is inline.
        className={danger ? "stdbtn stdbtn-danger" : "stdbtn"}
        style={{
          display: "block",
          width: "100%",
          height: 48,
          margin: "0 0 4px",
          fontSize: 16,
          borderRadius: 6,
          ...(confirmDisabled ? { opacity: 0.5, cursor: "not-allowed" } : {}),
        }}
        onClick={() => {
          confirmed.current = true
          setOpen(false)
        }}
      >
        {confirmLabel}
      </button>
    </BottomSheet>
  )
}
