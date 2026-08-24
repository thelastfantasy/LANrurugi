import { useEffect, useState } from "react"

/** True when the device has a real hover-capable pointer (mouse/trackpad) — false on a
 * touch-only device (phone, tablet with no attached mouse), where `mouseenter`/`mouseleave`
 * either never fire or fire on tap in a way that doesn't match a genuine "hovering to preview"
 * intent. Same `(hover: hover)` media query `TagCloud.tsx`'s own docs describe a third-party
 * library using internally to gate its `mousemove` listener — this is the first place in this
 * app's own code that reads it directly, for `BookmarkedArchiveHoverCard`'s tap-to-expand
 * fallback. */
export function useSupportsHover(): boolean {
  const query = "(hover: hover)"
  const [supportsHover, setSupportsHover] = useState(() => window.matchMedia(query).matches)
  useEffect(() => {
    const mql = window.matchMedia(query)
    const onChange = () => setSupportsHover(mql.matches)
    mql.addEventListener("change", onChange)
    return () => mql.removeEventListener("change", onChange)
  }, [])
  return supportsHover
}
