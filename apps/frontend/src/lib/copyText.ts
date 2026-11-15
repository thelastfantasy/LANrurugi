/** Copying text from any origin.
 *
 *  The async Clipboard API only exists in a secure context, and a self-hosted instance is normally
 *  opened over plain HTTP (LAN IP, no TLS), so it is frequently missing entirely. Every copy button
 *  must therefore be able to fall back to a selection-based copy, which works everywhere.
 *
 *  Deliberately dependency-free: the embed card bundle uses this and must not pull in React. */

/** Copies `text`, returning whether it worked. */
export async function copyText(text: string): Promise<boolean> {
  // A rejected `writeText` (some browsers expose the API but still refuse off HTTPS) would consume
  // the user activation the fallback needs, so only use the API where it can actually succeed.
  if (window.isSecureContext && navigator.clipboard?.writeText) {
    try {
      await navigator.clipboard.writeText(text)
      return true
    } catch {
      // Fall through to the selection-based copy.
    }
  }
  return copyViaSelection(text)
}

/** `document.execCommand("copy")` path. Must stay synchronous: it has to run inside the click's own
 *  user activation, which any `await` here would have ended. */
function copyViaSelection(text: string): boolean {
  const area = document.createElement("textarea")
  area.value = text
  area.setAttribute("readonly", "")
  // Off-screen but still laid out — `display: none` would make `select()` a no-op.
  area.style.position = "fixed"
  area.style.top = "0"
  area.style.left = "0"
  area.style.opacity = "0"

  const selection = document.getSelection()
  const previous = selection && selection.rangeCount > 0 ? selection.getRangeAt(0) : null
  let ok = false
  try {
    document.body.appendChild(area)
    area.select()
    area.setSelectionRange(0, text.length)
    ok = document.execCommand("copy")
  } catch {
    // Leave `ok` false: a browser without `execCommand` cannot copy this way at all.
  } finally {
    area.remove()
    // Put the user's own selection back; removing the textarea otherwise leaves it collapsed.
    if (previous && selection) {
      selection.removeAllRanges()
      selection.addRange(previous)
    }
  }
  return ok
}
