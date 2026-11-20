import { useEffect, useRef } from "react"

/** Set while a closed overlay's history entry is about to be popped, so a re-run of the effect
 * (StrictMode double-invokes it in development) can cancel the pop instead of letting it land on
 * the entry the re-run just pushed. */
let pendingPop: ReturnType<typeof setTimeout> | null = null

/** Takes over one browser-history entry while an overlay is open, so a system back gesture closes
 * it instead of navigating away. Android's back gesture and Firefox Android's own swipe-back are
 * browser/OS UI: a page cannot cancel them, and `preventDefault()` on `touchmove` does not reliably
 * stop them. Pushing an entry gives the gesture something harmless to land on.
 * The entry is popped again when the overlay closes some other way. */
export function useBackToClose(open: boolean, onClose: () => void) {
  const closeRef = useRef(onClose)
  useEffect(() => {
    closeRef.current = onClose
  })

  useEffect(() => {
    if (!open) return
    if (pendingPop !== null) {
      clearTimeout(pendingPop)
      pendingPop = null
    }
    if (!window.history.state?.__backToClose) {
      window.history.pushState({ __backToClose: true }, "", window.location.href)
    }
    let consumedByPop = false
    const onPop = () => {
      consumedByPop = true
      closeRef.current()
    }
    window.addEventListener("popstate", onPop)
    return () => {
      window.removeEventListener("popstate", onPop)
      if (consumedByPop) return
      pendingPop = setTimeout(() => {
        pendingPop = null
        if (window.history.state?.__backToClose) window.history.back()
      }, 0)
    }
  }, [open])
}
