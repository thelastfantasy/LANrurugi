import { useState } from "react"

// Mirrors legacy reader.js's own localStorage-backed settings exactly (key names, string
// serialization of booleans as "true"/"false", fitMode absent = container mode) — verified
// against `~/LANraragi/public/js/reader.js`. Analogous to `useSettings`/`useUpdateSettings` for
// server-side config, but localStorage-backed (no network, so a plain custom hook, not
// react-query) since these are genuinely per-browser reader preferences in legacy too.

export type FitMode = "container" | "fit-width" | "fit-height"

/** `"percent"` scales with the viewport (`vh` is exactly this, expressed as a CSS unit rather
 * than a plain number — the two aren't different concepts, just different ways of writing the
 * same relative-to-viewport-height fraction) so `j`'s scroll step stays proportionally the same
 * whether it's a phone or an ultrawide monitor; `"px"` is a fixed distance regardless of viewport
 * size, for a reader who wants the *exact same* jump every time. */
export type JScrollUnit = "percent" | "px"

export interface ReaderSettings {
  hideHeader: boolean
  mangaMode: boolean
  doublePageMode: boolean
  ignoreProgress: boolean
  showOverlayByDefault: boolean
  fitMode: FitMode
  containerWidth: string
  markersVisible: boolean
  preloadCount: number
  autoNextPageInterval: number
  infiniteScroll: boolean
  jScrollUnit: JScrollUnit
  jScrollAmount: number
}

const DEFAULTS: ReaderSettings = {
  hideHeader: false,
  mangaMode: false,
  doublePageMode: false,
  ignoreProgress: false,
  // Deliberately `true`, unlike legacy's own `false` default — a real, requested product
  // decision to always land on the archive overview first, not a port of legacy's own behavior.
  showOverlayByDefault: true,
  fitMode: "container",
  containerWidth: "",
  markersVisible: true,
  preloadCount: 2,
  autoNextPageInterval: 10,
  infiniteScroll: false,
  // Matches the `j`/`" "` scroll step's original hardcoded value (`window.innerHeight * 0.8`)
  // exactly, so introducing this setting doesn't change anyone's existing scroll feel by default.
  jScrollUnit: "percent",
  jScrollAmount: 80,
}

function readBool(key: string, fallback: boolean): boolean {
  const raw = localStorage.getItem(key)
  return raw === null ? fallback : raw === "true"
}

function readFromLocalStorage(): ReaderSettings {
  const fitModeRaw = localStorage.getItem("fitMode")
  const fitMode: FitMode =
    fitModeRaw === "fit-width" || fitModeRaw === "fit-height" ? fitModeRaw : "container"

  return {
    hideHeader: readBool("hideHeader", DEFAULTS.hideHeader),
    mangaMode: readBool("mangaMode", DEFAULTS.mangaMode),
    doublePageMode: readBool("doublePageMode", DEFAULTS.doublePageMode),
    ignoreProgress: readBool("ignoreProgress", DEFAULTS.ignoreProgress),
    showOverlayByDefault: readBool("showOverlayByDefault", DEFAULTS.showOverlayByDefault),
    fitMode,
    containerWidth: localStorage.getItem("containerWidth") ?? DEFAULTS.containerWidth,
    markersVisible: readBool("markersVisible", DEFAULTS.markersVisible),
    preloadCount: Number(localStorage.getItem("preloadCount")) || DEFAULTS.preloadCount,
    autoNextPageInterval:
      Number(localStorage.getItem("AutoNextPageInterval")) || DEFAULTS.autoNextPageInterval,
    infiniteScroll: readBool("infiniteScroll", DEFAULTS.infiniteScroll),
    jScrollUnit: localStorage.getItem("jScrollUnit") === "px" ? "px" : DEFAULTS.jScrollUnit,
    jScrollAmount: Number(localStorage.getItem("jScrollAmount")) || DEFAULTS.jScrollAmount,
  }
}

function writeToLocalStorage(partial: Partial<ReaderSettings>) {
  if (partial.hideHeader !== undefined) localStorage.setItem("hideHeader", String(partial.hideHeader))
  if (partial.mangaMode !== undefined) localStorage.setItem("mangaMode", String(partial.mangaMode))
  if (partial.doublePageMode !== undefined) {
    localStorage.setItem("doublePageMode", String(partial.doublePageMode))
  }
  if (partial.ignoreProgress !== undefined) {
    localStorage.setItem("ignoreProgress", String(partial.ignoreProgress))
  }
  if (partial.showOverlayByDefault !== undefined) {
    localStorage.setItem("showOverlayByDefault", String(partial.showOverlayByDefault))
  }
  if (partial.fitMode !== undefined) {
    if (partial.fitMode === "container") {
      localStorage.removeItem("fitMode")
    } else {
      localStorage.setItem("fitMode", partial.fitMode)
    }
  }
  if (partial.containerWidth !== undefined) {
    localStorage.setItem("containerWidth", partial.containerWidth)
  }
  if (partial.markersVisible !== undefined) {
    localStorage.setItem("markersVisible", String(partial.markersVisible))
  }
  if (partial.preloadCount !== undefined) {
    localStorage.setItem("preloadCount", String(partial.preloadCount))
  }
  if (partial.autoNextPageInterval !== undefined) {
    localStorage.setItem("AutoNextPageInterval", String(partial.autoNextPageInterval))
  }
  if (partial.infiniteScroll !== undefined) {
    localStorage.setItem("infiniteScroll", String(partial.infiniteScroll))
  }
  if (partial.jScrollUnit !== undefined) {
    localStorage.setItem("jScrollUnit", partial.jScrollUnit)
  }
  if (partial.jScrollAmount !== undefined) {
    localStorage.setItem("jScrollAmount", String(partial.jScrollAmount))
  }
}

export function useReaderSettings(): [ReaderSettings, (partial: Partial<ReaderSettings>) => void] {
  const [settings, setSettings] = useState<ReaderSettings>(readFromLocalStorage)

  function update(partial: Partial<ReaderSettings>) {
    writeToLocalStorage(partial)
    setSettings((prev) => ({ ...prev, ...partial }))
  }

  return [settings, update]
}
