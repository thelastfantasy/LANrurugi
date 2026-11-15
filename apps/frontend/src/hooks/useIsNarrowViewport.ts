import { useEffect, useState } from "react"

/** True below the breakpoint where a multi-column layout shrinks its content too small to use. */
export function useIsNarrowViewport(query = "(max-width: 640px)"): boolean {
  const [narrow, setNarrow] = useState(() => window.matchMedia(query).matches)
  useEffect(() => {
    const mql = window.matchMedia(query)
    const onChange = () => setNarrow(mql.matches)
    mql.addEventListener("change", onChange)
    return () => mql.removeEventListener("change", onChange)
  }, [query])
  return narrow
}

/** Phone width, where menus and confirmations become bottom sheets (and the reader turns on
 * infinite scroll when asked to). */
export const PHONE_WIDTH_QUERY = "(max-width: 768px)"
