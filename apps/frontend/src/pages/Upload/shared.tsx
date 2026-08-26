import type Lenis from "lenis"
import { useCallback, useEffect, useRef } from "react"

import type { PluginInfo } from "@/api/types"
import { TagTable, Tooltip } from "@/components/Display"
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

/** The fixed `plugin_namespace` every local-upload queue item is stored under
 * (`crates/lanrurugi-api/src/upload.rs`'s own constant of the same name) — never a real installed
 * plugin, just this item type's own grouping key on the Upload page. */
export const LOCAL_UPLOAD_NAMESPACE = "local_upload"

// Common compound extensions hardcoded, not user-configurable — rare enough in this app's actual
// corpus that a settings surface would be over-engineering.
const COMPOUND_EXTENSIONS = ["tar.gz", "tar.bz2", "tar.xz", "tar.zst"]

/** Splits a filename into stem and extension, recognizing `COMPOUND_EXTENSIONS` as one unit
 * (`archive.tar.gz` → `{stem: "archive", ext: "tar.gz"}`, not `{stem: "archive.tar", ext: "gz"}`)
 * — otherwise the `{filename}`/`{ext}` template variables `FilenameTemplateEditor` feeds from
 * this, and `TruncatedFilename`'s own truncation below, would both mangle a `.tar.gz` name. */
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

/** A queue row's title/filename text, truncated with an ellipsis when too long for its row —
 * but truncating the *stem* only, never the extension: `splitFilenameStemAndExt` splits the two
 * apart so the stem sits in a `min-width: 0` flex-shrinking span (the part that actually
 * truncates) while the extension sits in its own `flexShrink: 0` span at the end, always fully
 * visible. A plain single-span `text-overflow: ellipsis` would truncate from the end, silently
 * eating the extension along with however much of the stem doesn't fit — hiding exactly the part
 * (`.zip` vs. `.pdf` vs. `.cbz`) most useful for telling two similarly-named rows apart at a
 * glance. Skips the split entirely (renders `text` as one plain truncating span) when `text` is
 * `item.title` rather than a real filename — a plugin-supplied title has no meaningful
 * "extension" to preserve. */
export function TruncatedFilename({
  text,
  isFilename,
  style,
}: {
  text: string
  /** `true` only when `text` is actually `item.url`/a filename — `item.title` (a plugin- or
   * metadata-supplied display name) has no extension worth carving out. */
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

/** First match wins when multiple plugins' `url_pattern` match the same URL. Relies on
 * `usePlugins(...)`'s list already being sorted by priority server-side
 * (`lanrurugi_api::plugins::list_plugins`), reflecting the Plugins page's drag-to-reorder.
 *
 * This is the *precise trigger* check — real dispatch (which download plugin actually fetches
 * this URL). For "is there a plugin that could handle this domain at all" questions (the
 * "fetch metadata" button's enablement), use `findPluginByDomain` below instead — using this one
 * for that purpose was a real, confirmed bug (2026-08-26): a plugin's `url_pattern` can be
 * intentionally narrower than its domain (e.g. requiring a specific path), which is correct for
 * deciding when to actually fire a fetch but wrong for "does a plugin exist for this domain". */
export function findMatchingPlugin(plugins: PluginInfo[] | undefined, url: string): PluginInfo | null {
  return plugins?.find((p) => matchesPattern(p, url)) ?? null
}

/** Lowercases and strips a leading `www.` — mirrors the backend's own `normalize_domain`
 * (`plugins.rs`), the only two normalizations any real plugin declaration or user-typed domain
 * actually needs. */
function normalizeDomain(d: string): string {
  const lower = d.trim().replace(/\/+$/, "").toLowerCase()
  return lower.startsWith("www.") ? lower.slice(4) : lower
}

/** Extracts a bare hostname out of a full URL for domain-ownership comparisons; falls back to the
 * input unchanged if it doesn't parse as an absolute URL (already a bare domain). */
function hostnameOf(url: string): string {
  try {
    return new URL(url).hostname
  } catch {
    return url
  }
}

/** Domain-ownership check — does `plugin.domain_match` (or, when empty, a loose `url_pattern`-as-
 * domain-containment fallback) consider `url`'s domain covered? Use this for "is there a plugin
 * that could handle this domain at all" questions (the Upload page's "fetch metadata" button
 * enablement) — `matchesPattern`/`findMatchingPlugin` above stay the precise trigger check for an
 * actual fetch/download call; do not replace those call sites with this. */
export function matchesDomain(plugin: PluginInfo, url: string): boolean {
  const needle = normalizeDomain(hostnameOf(url))
  if (plugin.domain_match.length > 0) {
    return plugin.domain_match.some((d) => normalizeDomain(d) === needle)
  }
  return matchesPattern(plugin, url)
}

/** Domain-ownership counterpart to `findMatchingPlugin` — see `matchesDomain`'s own docs on when
 * to use which. */
export function findPluginByDomain(plugins: PluginInfo[] | undefined, url: string): PluginInfo | null {
  return plugins?.find((p) => matchesDomain(p, url)) ?? null
}

/** A square, icon-only `.stdbtn` — overrides its default padding/width so the icon is centered in
 * a fixed 24x24 box instead of stretching to fit label text (there is none). `minWidth` must be
 * overridden too since `.stdbtn`'s theme `min-width: 150px` is a hard floor a smaller inline
 * `width` alone can't shrink below. `boxSizing: 'border-box'` keeps the button's true footprint
 * consistent across themes where `.stdbtn` has a real `border` (`ex.css`, `g.css`) vs. `border: 0`. */
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

/** Overrides `.stdbtn`'s theme `min-width: 150px` (5 buttons at that width, plus gaps, don't fit
 * most viewports) so the download-queue toolbar sizes each button to its own text instead of
 * wrapping. */
export const TOOLBAR_BUTTON_STYLE: React.CSSProperties = {
  minWidth: 0,
  width: "auto",
  flex: "0 1 auto",
  whiteSpace: "nowrap",
  fontSize: FONT_SIZE_XS,
  padding: "0 6px",
}

/** Renders a metadata plugin's `{tags?, title?, summary?}` response as a short tooltip body —
 * schema-agnostic (see `DownloadQueueItem.metadata_preview`'s own docs), grouped by namespace via
 * the shared `TagTable`. No separate "raw URL" line: a `source:` tag, when present, already links
 * to the same URL, so showing both would be duplication. */
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

/** Wraps `children` in a `Tooltip` only when `preview` is actually present — extracted so the
 * `Tooltip` itself (and therefore its anchor measurement) wraps the *bordered* container, not
 * just the inner text span, so the bubble's left edge lines up with the border the user actually
 * sees, not with the unbordered text a few pixels further in. */
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
    // No `Tooltip` wrapper needed, but `wrapperStyle` still has to land somewhere, or this
    // branch's element shrinks to its content width instead of matching the tooltip-present
    // branch's sizing.
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
