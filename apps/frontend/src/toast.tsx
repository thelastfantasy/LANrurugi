import { useEffect, useLayoutEffect } from "react"
import {
  toast as emitToast,
  type ToastOptions,
  type ToastTransitionProps,
} from "react-toastify"

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

const EXIT_MS = 240
const COLLAPSE_MS = 260

/** Horizontal offset the drag left the card at, so the exit animation can start where the finger
 * let go instead of jumping back to the resting position first. Reads the inline value first: the
 * drag writes `translate3d(<x>px, var(--y), 0)`, and until the container defines `--y` that whole
 * declaration is invalid at computed-value time (computed `transform` is `none`), which made this
 * report 0 and skip the fly-out entirely. */
function offsetX(el: HTMLElement): number {
  const inline = /translate3d\(\s*(-?[\d.]+)px/.exec(el.style.transform)
  if (inline) return Number(inline[1])
  const computed = getComputedStyle(el).transform
  const match = /matrix3d?\(([^)]+)\)/.exec(computed)
  if (!match) return 0
  const parts = match[1].split(",").map(Number)
  return parts.length === 16 ? parts[12] : (parts[4] ?? 0)
}

/** Closes the gap the removed card leaves. The library's own version transitions `all` with the
 * default easing; this one animates height only, with a curve that settles instead of stopping. */
function collapse(el: HTMLElement, done: () => void) {
  const height = el.scrollHeight
  const style = el.style
  style.minHeight = "initial"
  style.height = `${height}px`
  style.transition = `height ${COLLAPSE_MS}ms cubic-bezier(.2,.9,.25,1), margin ${COLLAPSE_MS}ms cubic-bezier(.2,.9,.25,1)`
  requestAnimationFrame(() => {
    style.height = "0px"
    style.padding = "0px"
    style.margin = "0px"
    window.setTimeout(done, COLLAPSE_MS)
  })
}

/**
 * Android-notification dismissal: the card leaves the way it was thrown — accelerating, fading and
 * rotating slightly — and the rest of the stack then closes the gap.
 *
 * Written by hand rather than built from `cssTransition` because the library *skips* its exit
 * animation for a drag dismissal (`preventExitTransition` is set once the swipe passes the
 * threshold, on the assumption that the drag itself was the animation) and collapses the height
 * from wherever the finger left the card. The result was a card that vanished in place at 35% of
 * the way out, which reads as "no closing animation" (reported 2026-11-11).
 */
export function AndroidToastTransition({
  children,
  isIn,
  done,
  nodeRef,
  preventExitTransition,
}: ToastTransitionProps) {
  useLayoutEffect(() => {
    const el = nodeRef.current
    if (!el) return
    el.classList.add("lrr-toast-enter")
    // The library fades the card out as it approaches the swipe threshold, so it is already
    // invisible by the time the finger lifts. Keep it solid while dragging and let the fly-out own
    // the fade (`opacity: 1 !important` has to sit on a class, which is why this is tracked here).
    const grab = () => el.classList.add("lrr-toast-dragging")
    const release = () => el.classList.remove("lrr-toast-dragging")
    el.addEventListener("pointerdown", grab)
    window.addEventListener("pointerup", release)
    window.addEventListener("pointercancel", release)
    return () => {
      el.removeEventListener("pointerdown", grab)
      window.removeEventListener("pointerup", release)
      window.removeEventListener("pointercancel", release)
    }
  }, [nodeRef])

  useEffect(() => {
    if (isIn) return
    const el = nodeRef.current
    if (!el) {
      done()
      return
    }
    el.classList.remove("lrr-toast-dragging")
    const offset = offsetX(el)
    const thrown = Math.abs(offset) > 4
    // Even a drag dismissal animates: `preventExitTransition` only means the library declined to.
    if (!thrown && preventExitTransition) {
      collapse(el, done)
      return
    }
    // Start from exactly where the drag left the card instead of snapping it back first.
    el.style.setProperty("--lrr-exit-from", `translate3d(${offset}px, 0, 0)`)
    el.classList.add(
      !thrown
        ? "lrr-toast-exit-fade"
        : offset < 0
          ? "lrr-toast-exit-left"
          : "lrr-toast-exit-right",
    )
    // A timer rather than `animationend`: the durations are ours, and a cancelled animation (reduced
    // motion, a hidden tab) must still finish the removal instead of leaving the toast stuck.
    const reduced = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false
    window.setTimeout(() => collapse(el, done), reduced ? 0 : thrown ? EXIT_MS : 180)
  }, [isIn, done, nodeRef, preventExitTransition])

  return <>{children}</>
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
