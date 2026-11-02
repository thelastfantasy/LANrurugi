import { useEffect, useState } from "react"

/** How many series colours each theme's `.stats-chart-palette` block defines. */
const SERIES_COUNT = 8
/** The class each of the five theme files carries the series ramp on (an additive class — legacy
 *  has no charts; see that block's own comment in each `public/legacy/themes/*.css`). */
const PROBE_CLASS = "stats-chart-palette"
/** Used only when a theme genuinely defines nothing (e.g. a theme file that predates this class):
 *  inherits the surrounding text colour instead of inventing a literal here. */
const FALLBACK = "currentColor"
/** Re-read delays after a theme change, in ms. The theme switch swaps the theme stylesheet's
 *  `href`, and the browser then fetches/parses it asynchronously — so the new custom properties do
 *  not exist the instant `<html data-theme>` flips, and a single read at that moment can return the
 *  outgoing theme's colours. A short, bounded retry costs three extra reads on a theme switch and
 *  depends on no event plumbing (a `load` listener on the link would have to be attached from a
 *  *parent* effect to reliably beat the first load, since React runs child effects first). */
const REREAD_DELAYS_MS = [150, 600, 1500]

function readPalette(): string[] {
  if (typeof document === "undefined" || !document.body) {
    return Array.from({ length: SERIES_COUNT }, () => FALLBACK)
  }
  const probe = document.createElement("div")
  probe.className = PROBE_CLASS
  probe.style.display = "none"
  document.body.appendChild(probe)
  const computed = getComputedStyle(probe)
  const colors = Array.from(
    { length: SERIES_COUNT },
    (_, i) => computed.getPropertyValue(`--stats-series-${i + 1}`).trim() || FALLBACK,
  )
  probe.remove()
  return colors
}

/** The active theme's chart series colours, read out of the theme stylesheet itself rather than
 *  hardcoded per theme in TS — so the palette follows all five themes (and any future theme that
 *  carries the same class) with no second place to keep in sync.
 *
 *  Re-read whenever `<html data-theme>` changes: `theme.ts::useApplyTheme` swaps the theme
 *  `<link>` and sets that attribute, and that is exactly when the resolved custom properties are
 *  about to change. */
export function useChartPalette(): string[] {
  const [palette, setPalette] = useState<string[]>(readPalette)

  useEffect(() => {
    const timers: number[] = []
    const readNowAndAfterTheStylesheetSettles = () => {
      setPalette(readPalette())
      for (const delay of REREAD_DELAYS_MS) {
        timers.push(window.setTimeout(() => setPalette(readPalette()), delay))
      }
    }
    readNowAndAfterTheStylesheetSettles()
    const observer = new MutationObserver(readNowAndAfterTheStylesheetSettles)
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    })
    return () => {
      for (const timer of timers) window.clearTimeout(timer)
      observer.disconnect()
    }
  }, [])

  return palette
}
