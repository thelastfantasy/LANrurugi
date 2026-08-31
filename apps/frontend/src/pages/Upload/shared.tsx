import type Lenis from "lenis"
import { useCallback, useEffect, useRef } from "react"

import type { PluginInfo } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { TagTable } from "@/components/Display"
import { useHorizontalScroll } from "@/hooks"
import { splitTagsByNamespace } from "@/lib/tagFormat"
import { FONT_SIZE_XS } from "@/theme"

/* ─── PatchAssignmentView 共享常量 + ScrollRow ─── */

export const ROW_GAP_PX = 6

export const THUMB_ASPECT_RATIO = "150 / 206"
const WHEEL_MULTIPLIER = 4.5
const SCROLL_REPEAT_MS = 60

export function ScrollRow({ count, renderPage, rowRef, lenisApiRef }: {
  count: number; renderPage: (i: number) => React.ReactNode
  rowRef: React.RefObject<HTMLDivElement | null>
  lenisApiRef?: React.MutableRefObject<{ lenis: Lenis; stepPx(): number } | null>
}) {
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)
  const heldKeyRef = useRef<string | null>(null)
  const lenisRef = useHorizontalScroll(rowRef, { wheelMultiplier: WHEEL_MULTIPLIER })
  const stepPx = useCallback(() => { const c = rowRef.current?.firstElementChild as HTMLElement | null; return c ? c.getBoundingClientRect().width + ROW_GAP_PX : 160 }, [rowRef])
  useEffect(() => { const l = lenisRef.current; if (l && lenisApiRef) lenisApiRef.current = { lenis: l, stepPx }; return () => { if (lenisApiRef) lenisApiRef.current = null } }, [])
  function start(d: 1 | -1) { stop(); const l = lenisRef.current; if (!l) return; l.scrollTo(l.targetScroll + d * stepPx()); intervalRef.current = setInterval(() => { const ln = lenisRef.current; ln?.scrollTo(ln.targetScroll + d * stepPx()) }, SCROLL_REPEAT_MS) }
  function stop() { if (intervalRef.current !== null) { clearInterval(intervalRef.current); intervalRef.current = null } }
  useEffect(() => stop, [])
  function kd(e: React.KeyboardEvent) { if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return; e.preventDefault(); if (heldKeyRef.current === e.key) return; heldKeyRef.current = e.key; start(e.key === "ArrowLeft" ? -1 : 1) }
  function ku(e: React.KeyboardEvent) { if (e.key !== heldKeyRef.current) return; heldKeyRef.current = null; stop() }
  const ap = (d: 1 | -1): React.HTMLAttributes<HTMLDivElement> => ({ onMouseDown: () => start(d), onMouseUp: stop, onMouseLeave: stop, onTouchStart: (e: React.TouchEvent) => { e.preventDefault(); start(d) }, onTouchEnd: stop, onTouchCancel: stop })
  const as = (dir: "left" | "right"): React.CSSProperties => ({ flexShrink: 0, width: 48, height: "100%", display: "flex", alignItems: "center", justifyContent: "center", cursor: "pointer", touchAction: "none", background: `linear-gradient(to ${dir}, transparent, rgba(0,0,0,0.6))` })
  return (
    <div style={{ flex: 1, minHeight: 0, display: "flex", gap: 4 }}>
      <div style={as("left")} {...ap(-1)}><i className="fa fa-2x fa-chevron-left" style={{ color: "white" }} aria-hidden="true" /></div>
      <div ref={rowRef} className="hide-scrollbar" tabIndex={0} onKeyDown={kd} onKeyUp={ku} style={{ display: "flex", gap: ROW_GAP_PX, overflowX: "auto", flex: 1, minHeight: 0, padding: "4px 0" }}>{Array.from({ length: count }, (_, i) => renderPage(i))}</div>
      <div style={as("right")} {...ap(1)}><i className="fa fa-2x fa-chevron-right" style={{ color: "white" }} aria-hidden="true" /></div>
    </div>
  )
}

/* ─── 原有 Upload 工具函数 ─── */

/** Fixed `plugin_namespace` every local-upload queue item is stored under — never a real
 * installed plugin, just this item type's own grouping key. */
export const LOCAL_UPLOAD_NAMESPACE = "local_upload"

const COMPOUND_EXTENSIONS = ["tar.gz", "tar.bz2", "tar.xz", "tar.zst"]

/** Splits a filename into stem/extension, treating `COMPOUND_EXTENSIONS` as one unit
 * (`archive.tar.gz` → `{stem: "archive", ext: "tar.gz"}`). */
export function splitFilenameStemAndExt(filename: string): { stem: string; ext: string } {
  for (const compound of COMPOUND_EXTENSIONS) {
    const suffix = `.${compound}`
    const start = filename.length - suffix.length
    if (start > 0 && filename.endsWith(suffix)) {
      return { stem: filename.slice(0, start), ext: compound }
    }
  }
  const lastDot = filename.lastIndexOf(".")
  if (lastDot <= 0) return { stem: filename, ext: "" }
  return { stem: filename.slice(0, lastDot), ext: filename.slice(lastDot + 1) }
}

/** Truncates a queue row's title/filename with an ellipsis, but only the *stem* — the extension
 * stays in its own `flexShrink: 0` span so `.zip`/`.pdf`/`.cbz` stays visible. */
export function TruncatedFilename({
  text,
  isFilename,
  style,
}: {
  text: string
  /** `true` only when `text` is a real filename, not a plugin-supplied display title. */
  isFilename: boolean
  style?: React.CSSProperties
}) {
  if (!isFilename) {
    return (
      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", ...style }}>{text}</span>
    )
  }
  const { stem, ext } = splitFilenameStemAndExt(text)
  return (
    <span style={{ display: "inline-flex", minWidth: 0, maxWidth: "100%", ...style }}>
      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", minWidth: 0 }}>{stem}</span>
      {ext && <span style={{ flexShrink: 0 }}>.{ext}</span>}
    </span>
  )
}

/** Matches `url` against `plugin.url_pattern` (a JS `RegExp` source, no delimiters), case-
 * insensitively. `null`/absent pattern never matches. */
export function matchesPattern(plugin: PluginInfo, url: string): boolean {
  if (!plugin.url_pattern) return false
  try {
    return new RegExp(plugin.url_pattern, "i").test(url)
  } catch {
    return false
  }
}

/** First match wins (plugin list is priority-sorted server-side) — the *precise trigger* check
 * for actual dispatch. Use `findPluginByDomain` for "does a plugin exist for this domain at all". */
export function findMatchingPlugin(plugins: PluginInfo[] | undefined, url: string): PluginInfo | null {
  return plugins?.find((p) => matchesPattern(p, url)) ?? null
}

/** Lowercases and strips a leading `www.` — mirrors the backend's own `normalize_domain`. */
function normalizeDomain(d: string): string {
  const lower = d.trim().replace(/\/+$/, "").toLowerCase()
  return lower.startsWith("www.") ? lower.slice(4) : lower
}

/** Extracts a bare hostname for domain-ownership comparisons; falls back to the input unchanged
 * if it doesn't parse as an absolute URL. */
function hostnameOf(url: string): string {
  try {
    return new URL(url).hostname
  } catch {
    return url
  }
}

/** Domain-ownership check — does `plugin.domain_match` (or, when empty, `url_pattern`) cover
 * `url`'s domain? For "fetch metadata" enablement, not actual fetch/download dispatch. */
export function matchesDomain(plugin: PluginInfo, url: string): boolean {
  const needle = normalizeDomain(hostnameOf(url))
  if (plugin.domain_match.length > 0) {
    return plugin.domain_match.some((d) => normalizeDomain(d) === needle)
  }
  return matchesPattern(plugin, url)
}

/** Domain-ownership counterpart to `findMatchingPlugin` — see `matchesDomain`'s docs. */
export function findPluginByDomain(plugins: PluginInfo[] | undefined, url: string): PluginInfo | null {
  return plugins?.find((p) => matchesDomain(p, url)) ?? null
}

/** A square, icon-only `.stdbtn` — fixed 24x24 box; `minWidth` override needed since `.stdbtn`'s
 * theme `min-width: 150px` is a hard floor a smaller inline `width` can't shrink below. */
export const ICON_BUTTON_STYLE: React.CSSProperties = {
  width: 24,
  minWidth: 24,
  height: 24,
  padding: 0,
  boxSizing: "border-box",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  fontSize: FONT_SIZE_XS,
}

/** Overrides `.stdbtn`'s theme `min-width: 150px` so the toolbar sizes each button to its own
 * text instead of wrapping. */
export const TOOLBAR_BUTTON_STYLE: React.CSSProperties = {
  minWidth: 0,
  width: "auto",
  flex: "0 1 auto",
  whiteSpace: "nowrap",
  fontSize: FONT_SIZE_XS,
  padding: "0 6px",
}

/** Renders a metadata plugin's `{tags?, title?, summary?}` response as a short tooltip body,
 * grouped by namespace. No separate "raw URL" line when a `source:` tag already covers it. */
export function MetadataPreviewTooltip({ preview, url }: { preview: Record<string, unknown>; url: string }) {
  const tags = typeof preview.tags === "string" ? preview.tags : ""
  const summary = typeof preview.summary === "string" ? preview.summary : undefined
  const hasSourceTag = "source" in splitTagsByNamespace(tags)

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
      <div style={{ fontWeight: "bold", wordBreak: "break-word" }}>
        {typeof preview.title === "string" ? preview.title : url}
      </div>
      {!hasSourceTag && <div style={{ wordBreak: "break-all", opacity: 0.8, fontSize: FONT_SIZE_XS }}>{url}</div>}
      <TagTable tags={tags} />
      {summary && <div style={{ opacity: 0.8 }}>{summary}</div>}
    </div>
  )
}

/** Wraps `children` in a `Tooltip` only when `preview` is present — wraps the bordered container
 * itself so the bubble's anchor lines up with the visible border, not the inner text. */
export function TooltipIfPresent({
  preview,
  url,
  children,
  wrapperStyle,
}: {
  preview: Record<string, unknown> | null
  url: string
  children: React.ReactNode
  wrapperStyle?: React.CSSProperties
}) {
  if (!preview) {
    // wrapperStyle still needs to land here or this branch's sizing won't match the tooltip case.
    return (
      <span style={{ position: "relative", display: "inline-flex", ...wrapperStyle }}>{children}</span>
    )
  }
  return (
    <Tooltip label={<MetadataPreviewTooltip preview={preview} url={url} />} wrapperStyle={wrapperStyle}>
      {children}
    </Tooltip>

  )
}
