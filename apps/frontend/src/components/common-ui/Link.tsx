import { type AnchorHTMLAttributes, type MouseEvent, type ReactNode } from "react"
import { useNavigate } from "react-router-dom"

export interface LinkProps extends Omit<AnchorHTMLAttributes<HTMLAnchorElement>, "href"> {
  /** In-app path, built through `routes.*` — never a raw template string. */
  to: string
  /** Replace the history entry instead of pushing one, e.g. for redirects. */
  replace?: boolean
  children: ReactNode
}

/** True for same-origin absolute paths, which `BrowserRouter` can take over. Anything else
 * (`https://…`, `//host`) is the browser's to load. */
function isInAppPath(to: string) {
  return to.startsWith("/") && !to.startsWith("//")
}

/** True when a click is the app's to act on: a plain left click that is not a modified gesture
 * (new tab/window, Alt save-link) and did not just end a text selection. Callers that need their
 * own click side effects (recording navigation state, toggling selection) check this before
 * `preventDefault()`, so the gestures an `<a>` owns stay the browser's. */
export function isPlainLinkClick(e: MouseEvent, target?: string | null) {
  if (e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return false
  if (target && target !== "_self") return false
  // The selection a drag just made (or an older one the click did not collapse) means the user was
  // selecting text, not following the link.
  return window.getSelection()?.isCollapsed ?? true
}

/** In-app link. Keeps real `<a href>` markup — status-bar preview, middle click and "open in new
 * tab" all stay the browser's — and only takes over the plain left click, which would otherwise do
 * a full page reload through `BrowserRouter`.
 *
 * The interception is deliberately narrow. Modifier and non-primary clicks are left alone so the
 * browser can open a new tab/window, and a click that ends a text selection is not navigation:
 * Firefox's Alt-drag selects part of a link's text, and a handler that grabbed `click` on the link
 * (`preventDefault` + `navigate`) both broke that selection and opened the page. */
export function Link({ to, replace = false, onClick, target, children, ...rest }: LinkProps) {
  const navigate = useNavigate()

  function handleClick(e: MouseEvent<HTMLAnchorElement>) {
    onClick?.(e)
    if (e.defaultPrevented) return
    if (!isPlainLinkClick(e, target)) return
    if (!isInAppPath(to)) return
    e.preventDefault()
    navigate(to, { replace })
  }

  return (
    <a href={to} target={target} onClick={handleClick} {...rest}>
      {children}
    </a>
  )
}
