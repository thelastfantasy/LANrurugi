import { useRef, useState } from "react"

import type { StatTag } from "@/api/types"

/** `${namespace}:${text}` composite identity shared between `TagCloud`'s click handler and this
 * hook's highlight state. */
export function tagKey(tag: { namespace: string | null; text: string }): string {
  return `${tag.namespace ?? ""}:${tag.text}`
}

const HIGHLIGHT_DURATION_MS = 10_000

/** Drives the `TagCloud` → `#tagList` "click a tag, jump to its stats row" behavior through React
 * state; scrolling is the one imperative piece, confined to a `ref` callback on row mount. */
export function useTagCloudHighlight() {
  const [detailedStatsOpen, setDetailedStatsOpen] = useState(false)
  const [highlightedKey, setHighlightedKey] = useState<string | null>(null)
  const pendingScrollKeyRef = useRef<string | null>(null)
  const highlightTimerRef = useRef<number | null>(null)

  function highlightTag(tag: StatTag) {
    const key = tagKey(tag)
    pendingScrollKeyRef.current = key
    setDetailedStatsOpen(true)
    setHighlightedKey(key)

    if (highlightTimerRef.current !== null) window.clearTimeout(highlightTimerRef.current)
    highlightTimerRef.current = window.setTimeout(() => {
      setHighlightedKey((current) => (current === key ? null : current))
      highlightTimerRef.current = null
    }, HIGHLIGHT_DURATION_MS)
  }

  /** Pass as a row's own `ref` — scrolls it into view the first time it mounts after becoming the
   * highlight target (it may not exist in the DOM yet when `highlightTag` itself runs). */
  function highlightedRowRef(key: string) {
    return (el: HTMLElement | null) => {
      if (!el || pendingScrollKeyRef.current !== key) return
      pendingScrollKeyRef.current = null
      el.scrollIntoView({ block: "center", behavior: "smooth" })
    }
  }

  return {
    detailedStatsOpen,
    onDetailedStatsOpenChange: setDetailedStatsOpen,
    highlightedKey,
    highlightTag,
    highlightedRowRef,
  }
}
