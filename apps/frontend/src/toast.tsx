import { toast as emitToast, type ToastOptions } from "react-toastify"

export type ToastIcon = "info" | "success" | "warning" | "error"

export interface ToastConfig {
  heading?: string
  /** Rendered as plain text (React's default escaping) unless `html: true` is also set — see that
   * field's own docs. */
  text?: string
  /** One inline action (e.g. "Undo" after moving an archive to the recycle bin). Rendered as a
   * `.stdbtn` button inside the toast; clicking it does not dismiss the toast — the handler owns
   * that, so a failed undo can leave its own error visible. */
  action?: { label: string; onClick: () => void }
  /** Opt-in to rendering `text` as raw HTML — only for developer-authored strings with no user-
   * or plugin-controlled content; never interpolate untrusted input with `html: true`. */
  html?: boolean
  icon?: ToastIcon
  hideAfter?: number | false
  closeOnClick?: boolean
  draggable?: boolean
  toastId?: string
}

const AUTO_CLOSE_TIME: Record<ToastIcon, number | false> = {
  info: 5000,
  success: 5000,
  warning: 10000,
  error: false,
}

/** Dismisses a toast by id (or, with no id, every toast) — re-export of `toast.dismiss`. */
export function dismissToast(id?: ReturnType<typeof emitToast>) {
  emitToast.dismiss(id)
}

/** Matches legacy's own `LRR.toast()` shape/defaults, except `position: 'bottom-right'` (legacy's
 * `top-left` sits under the nav/search bar). Requires `<ToastContainer>` mounted once (`App.tsx`). */
export function toast(c: ToastConfig) {
  const type = c.icon ?? "info"
  const isWarningOrError = type === "warning" || type === "error"
  const options: ToastOptions = {
    toastId: c.toastId,
    type,
    position: "bottom-right",
    autoClose: c.hideAfter ?? AUTO_CLOSE_TIME[type] ?? 7000,
    closeOnClick: c.closeOnClick ?? !isWarningOrError,
    // Draggable even for warnings/errors: those are exactly the toasts that stay until dismissed,
    // and a phone has no room for a row of X buttons (reported 2026-11-11). Dismissal by accidental
    // tap is what `closeOnClick: false` guards against — a deliberate swipe is not that.
    draggable: c.draggable ?? true,
  }
  return emitToast(
    // react-toastify 11.x doesn't render its own `.Toastify__toast-body` wrapper, so this class is
    // added back manually to pick up lrr.css's h2 styling; textAlign overrides the theme's centered body.
    <div className="Toastify__toast-body" style={{ textAlign: "left" }}>
      {c.heading && <h2>{c.heading}</h2>}
      {c.text && c.html && <div dangerouslySetInnerHTML={{ __html: c.text }} />}
      {c.text && !c.html && <div>{c.text}</div>}
      {c.action && (
        <button
          type="button"
          className="stdbtn"
          style={{ marginTop: 8 }}
          onClick={c.action.onClick}
        >
          {c.action.label}
        </button>
      )}
    </div>,
    options,
  )
}
