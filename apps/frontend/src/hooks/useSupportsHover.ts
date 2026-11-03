import { useEffect, useState } from "react"

/** True when the device has a real hover-capable pointer (mouse/trackpad) — false on touch-only
 * devices, where `mouseenter`/`mouseleave` either never fire or fire on tap. */
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
