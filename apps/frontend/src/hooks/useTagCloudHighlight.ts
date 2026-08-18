import { useRef, useState } from "react"

import type { StatTag } from "@/api/types"

/** `${namespace}:${text}` composite — the same key `Stats.tsx`'s own `#tagList` rows already use
 * as their React `key`, reused here as the identity `TagCloud`'s click handler and this hook's
 * highlight state agree on. */
export function tagKey(tag: { namespace: string | null; text: string }): string {
  return `${tag.namespace ?? ""}:${tag.text}`
}

/** How long a `#tagList` row stays highlighted after a `TagCloud` click jumps to it — long enough
 * to actually read/locate among the surrounding rows, short enough that it doesn't linger as
 * visual clutter once its job (drawing the eye to this one row) is done. */
const HIGHLIGHT_DURATION_MS = 10_000

/** Drives the `TagCloud` → `#tagList` "click a tag in the 3D cloud, jump to its detailed-stats
 * row" behavior entirely through React state instead of the `document.querySelector` +
 * `classList` DOM poking an earlier version of this used — same spirit as `useSectionDeepLink`'s
 * own hook-organized effect, but state-driven rather than `.collapsible-title.click()`-driven now
 * that `CollapsibleSection` accepts a real controlled `open`/`onOpenChange` pair. Scrolling is the
 * one piece that's unavoidably imperative (no React API scrolls an element into view), so it's
 * confined to a `ref` callback that fires once React has actually mounted/opened the target row —
 * never a `querySelector` reaching back into the DOM to *find* that row in the first place. */
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

  /** Pass as a row's own `ref` — scrolls it into view the first time it mounts/re-mounts after
   * becoming the highlight target (`CollapsibleSection`'s body only renders once `open`, so the
   * very row this needs to scroll to may not exist in the DOM yet at the moment `highlightTag`
   * itself runs). */
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
