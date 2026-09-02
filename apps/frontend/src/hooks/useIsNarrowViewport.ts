import { useEffect, useState } from "react"

/** True below the breakpoint where a multi-column layout shrinks its content too small to use. */
export function useIsNarrowViewport(): boolean {
  const query = "(max-width: 640px)"
  const [narrow, setNarrow] = useState(() => window.matchMedia(query).matches)
  useEffect(() => {
    const mql = window.matchMedia(query)
    const onChange = () => setNarrow(mql.matches)
    mql.addEventListener("change", onChange)
    return () => mql.removeEventListener("change", onChange)
  }, [])
  return narrow
}
