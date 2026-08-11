import { toast as emitToast, type ToastOptions } from "react-toastify"

export type ToastIcon = "info" | "success" | "warning" | "error"

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
 * shape and defaults, with one deliberate deviation — same underlying library (`react-toastify`),
 * just called directly from React instead of through legacy's Preact wrapper around it. Requires a
 * `<ToastContainer limit={7} theme="light" />` mounted once (see `App.tsx`), matching legacy's own
 * `initializeToasts()`.
 *
 * `position: 'bottom-right'`, not legacy's own `top-left` — that placement sits directly under
 * the top nav/search bar, where it's both easy to miss (outside the natural reading path for most
 * of this app's own UI, which is centered/left-to-right below the header) and easy to *mistake*
 * for part of the page's own header content at a glance. `bottom-right` is the de facto standard
 * placement for transient notifications (matching most modern web apps' own convention) and keeps
 * toasts clear of every other interactive chrome on this app's pages (issue #58).
 */
/** Dismisses a specific toast by the id `toast()` returned (or, called with no id, every toast
 * currently showing) — `react-toastify`'s own `toast.dismiss`, re-exported so call sites needing
 * a "processing…" toast that later gets replaced by a real result don't import that library
 * directly just for this one function. */
export function dismissToast(id?: ReturnType<typeof emitToast>) {
  emitToast.dismiss(id)
}

export function toast(c: ToastConfig) {
  const type = c.icon ?? "info"
  const isWarningOrError = type === "warning" || type === "error"
  const options: ToastOptions = {
    toastId: c.toastId,
    type,
    position: "bottom-right",
    autoClose: c.hideAfter ?? AUTO_CLOSE_TIME[type] ?? 7000,
    closeOnClick: c.closeOnClick ?? !isWarningOrError,
    draggable: c.draggable ?? !isWarningOrError,
  }
  return emitToast(
    // `Toastify__toast-body` class + `textAlign: 'left'` inline — `lrr.css` already carries a real
    // `.Toastify__toast-body h2` rule (14px, weight 600, `margin: 0; padding: 4px 0 8px`) sized
    // correctly for this exact toast, but this app's actual `react-toastify` version (11.x) never
    // renders a `.Toastify__toast-body` wrapper on its own (confirmed live via `innerHTML`
    // inspection — the content div is an unclassed direct child of `.Toastify__toast` instead), so
    // that selector never matched and the `<h2>` fell through to the bare browser default
    // (`font-size: 16px`, `margin: 13.28px 0` — issue #58's "ugly whitespace above the heading").
    // Adding the class back here (rather than re-declaring the same typography inline) makes
    // legacy's own rule apply again, matching its intent exactly. `textAlign: 'left'` still needed
    // on top of that: the active theme's own `body { text-align: center }` (`g.css` etc., legacy's
    // real global centering convention for un-positioned elements like images) inherits all the way
    // down into the portal, and `lrr.css`'s `.Toastify__toast-body` rule doesn't itself override it.
    <div className="Toastify__toast-body" style={{ textAlign: "left" }}>
      {c.heading && <h2>{c.heading}</h2>}
      {c.text && <div dangerouslySetInnerHTML={{ __html: c.text }} />}
    </div>,
    options,
  )
}
