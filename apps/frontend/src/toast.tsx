import { toast as emitToast, type ToastOptions } from 'react-toastify'

export type ToastIcon = 'info' | 'success' | 'warning' | 'error'

export interface ToastConfig {
  heading?: string
  /** Raw HTML, same as legacy's own `text` field — e.g. an inline `<a href=...>` link. Only ever
   * fed developer-authored strings (never user input) at any call site, matching legacy's own
   * security posture for this specific helper. */
  text?: string
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

/** Matches legacy's own `LRR.toast()` (`~/LANraragi/public/js/mod/common.js`'s `toast()`) call
 * shape and defaults exactly — same underlying library (`react-toastify`), just called directly
 * from React instead of through legacy's Preact wrapper around it. Requires a `<ToastContainer
 * limit={7} theme="light" />` mounted once (see `App.tsx`), matching legacy's own
 * `initializeToasts()`. */
export function toast(c: ToastConfig) {
  const type = c.icon ?? 'info'
  const isWarningOrError = type === 'warning' || type === 'error'
  const options: ToastOptions = {
    toastId: c.toastId,
    type,
    position: 'top-left',
    autoClose: c.hideAfter ?? AUTO_CLOSE_TIME[type] ?? 7000,
    closeOnClick: c.closeOnClick ?? !isWarningOrError,
    draggable: c.draggable ?? !isWarningOrError,
  }
  return emitToast(
    <div>
      {c.heading && <h2>{c.heading}</h2>}
      {c.text && <div dangerouslySetInnerHTML={{ __html: c.text }} />}
    </div>,
    options,
  )
}
