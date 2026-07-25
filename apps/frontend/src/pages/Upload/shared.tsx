import type { PluginInfo } from '../../api/types'
import TagTable from '../../components/TagTable'
import Tooltip from '../../components/Tooltip'
import { splitTagsByNamespace } from '../../lib/tagFormat'
import { FONT_SIZE_10PT } from '../../theme'

/** Matches `url` against `plugin.url_pattern` (a JS `RegExp` source, no delimiters), case-
 * insensitively. `null`/absent pattern never matches. */
export function matchesPattern(plugin: PluginInfo, url: string): boolean {
  if (!plugin.url_pattern) return false
  try {
    return new RegExp(plugin.url_pattern, 'i').test(url)
  } catch {
    return false
  }
}

/** First match wins when multiple plugins' `url_pattern` match the same URL. Relies on
 * `usePlugins(...)`'s list already being sorted by priority server-side
 * (`lanrurugi_api::plugins::list_plugins`), reflecting the Plugins page's drag-to-reorder. */
export function findMatchingPlugin(plugins: PluginInfo[] | undefined, url: string): PluginInfo | null {
  return plugins?.find((p) => matchesPattern(p, url)) ?? null
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
  boxSizing: 'border-box',
  display: 'inline-flex',
  alignItems: 'center',
  justifyContent: 'center',
  fontSize: FONT_SIZE_10PT,
}

/** Overrides `.stdbtn`'s theme `min-width: 150px` (5 buttons at that width, plus gaps, don't fit
 * most viewports) so the download-queue toolbar sizes each button to its own text instead of
 * wrapping. */
export const TOOLBAR_BUTTON_STYLE: React.CSSProperties = {
  minWidth: 0,
  width: 'auto',
  flex: '0 1 auto',
  whiteSpace: 'nowrap',
  fontSize: FONT_SIZE_10PT,
  padding: '0 6px',
}

/** Renders a metadata plugin's `{tags?, title?, summary?}` response as a short tooltip body —
 * schema-agnostic (see `DownloadQueueItem.metadata_preview`'s own docs), grouped by namespace via
 * the shared `TagTable`. No separate "raw URL" line: a `source:` tag, when present, already links
 * to the same URL, so showing both would be duplication. */
export function MetadataPreviewTooltip({ preview, url }: { preview: Record<string, unknown>; url: string }) {
  const tags = typeof preview.tags === 'string' ? preview.tags : ''
  const summary = typeof preview.summary === 'string' ? preview.summary : undefined
  const hasSourceTag = 'source' in splitTagsByNamespace(tags)

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
      <div style={{ fontWeight: 'bold', wordBreak: 'break-word' }}>
        {typeof preview.title === 'string' ? preview.title : url}
      </div>
      {!hasSourceTag && <div style={{ wordBreak: 'break-all', opacity: 0.8, fontSize: FONT_SIZE_10PT }}>{url}</div>}
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
      <span style={{ position: 'relative', display: 'inline-flex', ...wrapperStyle }}>{children}</span>
    )
  }
  return (
    <Tooltip label={<MetadataPreviewTooltip preview={preview} url={url} />} wrapperStyle={wrapperStyle}>
      {children}
    </Tooltip>
  )
}
