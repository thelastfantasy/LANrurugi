import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { fetchJson, sendJson } from '../api/client'
import {
  useAddToQueue,
  useCategories,
  useClearCompletedQueue,
  useDeleteQueueItem,
  useDeleteSelectedQueue,
  useDownloadQueue,
  useJobs,
  useOverwriteQueueItem,
  usePlugins,
  useRenameQueueItem,
  useSettings,
  useStartQueueItem,
  useStartSelectedQueue,
  useStopQueueItem,
  useUpdateQueueItem,
} from '../api/hooks'
import type {
  ArchiveMetadata,
  DownloadQueueItem,
  JobRecord,
  PendingFilenameConflict,
  PluginInfo,
} from '../api/types'
import CollapsibleSection from '../components/CollapsibleSection'
import { formatBytes, JobProgressBar, STATE_COLOR } from '../components/JobProgress'
import { PopupMenu, PopupMenuItem, useMenuPalette } from '../components/PopupMenu'
import QueueErrorText from '../components/QueueErrorText'
import TagTable from '../components/TagTable'
import Tooltip from '../components/Tooltip'
import { splitTagsByNamespace } from '../lib/tagFormat'
import { routes } from '../routes'
import { FONT_SIZE_8PT, FONT_SIZE_10PT, useApplyTheme, Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'

interface UploadRow {
  key: string
  name: string
  state: 'processing' | 'done' | 'error'
  archiveId?: string
  title?: string
  message?: string
}

/** Matches `url` against `plugin.url_pattern` (a JS `RegExp` source, no delimiters — see the
 * field's own docs), case-insensitively. `null`/absent pattern never matches (a plugin with no
 * meaningful URL-based routing is never auto-selected). */
function matchesPattern(plugin: PluginInfo, url: string): boolean {
  if (!plugin.url_pattern) return false
  try {
    return new RegExp(plugin.url_pattern, 'i').test(url)
  } catch {
    return false
  }
}

/** When more than one plugin's `url_pattern` matches the same URL (a real, expected case — e.g.
 * multiple metadata plugins that can all handle a given site), the first match wins. This relies
 * on `usePlugins(...)`'s own list already being sorted by priority server-side
 * (`lanrurugi-api::plugins::list_plugins`), which in turn reflects the Plugins page's own
 * drag-to-reorder — so "first match" here means "highest-priority match", not "whichever
 * happened to be discovered first on disk". */
function findMatchingPlugin(plugins: PluginInfo[] | undefined, url: string): PluginInfo | null {
  return plugins?.find((p) => matchesPattern(p, url)) ?? null
}

/** Fetches `metadataPlugin`'s preview for `item.url` and persists it onto the queue item (title +
 * `metadata_preview`, via `update` — whichever `useUpdateQueueItem()` instance the caller already
 * has, since that hook's own `onSuccess` already invalidates the `download-queue` query, so
 * neither call site needs its own extra invalidation). Module-level (not a `QueueItemRow`-local
 * closure) so both the single-row Start button and the batch "Start (N)" button can call the exact
 * same logic — a batch start goes through `DownloadQueuePanel`, which has no per-row `item`/
 * `metadataPlugin` closure to reach into. */
async function fetchMetadataForItem(
  item: DownloadQueueItem,
  metadataPlugin: PluginInfo | null,
  update: ReturnType<typeof useUpdateQueueItem>,
) {
  if (!metadataPlugin || item.metadata_preview) return
  const result = await sendJson<{ success: number; data?: Record<string, unknown> }>(
    'POST',
    `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&arg=${encodeURIComponent(item.url)}`,
  ).catch(() => null)
  const data = result?.data
  const title = typeof data?.title === 'string' ? data.title : undefined
  if (data) await update.mutateAsync({ id: item.id, title, metadata_preview: data })
}

/** A square, icon-only `.stdbtn` — overrides its default padding/width so the icon is centered in
 * a fixed 24x24 box instead of stretching to fit label text (there is none). Two theme-specific
 * gotchas this accounts for:
 * - `.stdbtn`'s own `min-width: 150px` (themes/modern.css and others) is a hard floor on the
 *   computed width that a smaller inline `width` alone cannot shrink below — `minWidth` must be
 *   overridden too, or every one of these buttons renders 150px wide regardless of `width: 24`.
 * - Some themes (`ex.css`, `g.css`) give `.stdbtn` a real `border: 2px solid ...` rather than
 *   `border: 0`; content-box sizing (this app's default, matching every other unstyled element)
 *   would then render 4px *larger* than `width`/`height` (2px border on each side, added on top
 *   of the content box) — `boxSizing: 'border-box'` makes the border count toward the declared
 *   size instead, so the button is the same true footprint across every theme. */
const ICON_BUTTON_STYLE: React.CSSProperties = {
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

/** Overrides `.stdbtn`'s theme-provided `min-width: 150px` (5 of those, plus gaps, don't fit most
 * viewport widths — see the toolbar's own comment for the full story) so the 5-button download-
 * queue toolbar can size each button to its own text and genuinely never wrap, rather than just
 * being visually allowed to. */
const TOOLBAR_BUTTON_STYLE: React.CSSProperties = {
  minWidth: 0,
  width: 'auto',
  flex: '0 1 auto',
  whiteSpace: 'nowrap',
  fontSize: FONT_SIZE_10PT,
  padding: '0 6px',
}

/** Renders a metadata plugin's `{tags?, title?, summary?}` response as a short tooltip body —
 * deliberately schema-agnostic (see `DownloadQueueItem.metadata_preview`'s own docs): the tags
 * themselves render via the shared `TagTable` (same namespace-colored look the Library grid's own
 * tag tooltip uses), grouped by whatever namespaces this particular plugin happened to return
 * (E-Hentai's `artist:`/`uploader:`/`category:`/`timestamp:`/`source:`, or a completely different
 * set from another site's plugin) — an untagged/non-namespaced entry falls under `TagTable`'s own
 * `other` bucket rather than being dropped.
 *
 * No separate "raw URL" line: a `source:` tag (when present) already links out to the same URL
 * (`TagTable`'s own `source`-namespace handling) — showing both was pure duplication. */
function MetadataPreviewTooltip({ preview, url }: { preview: Record<string, unknown>; url: string }) {
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

// Full rewrite (Upload page redesign): "Add from URL" no longer starts a download immediately —
// it stages matched URLs into a persistent, server-side queue (`useDownloadQueue`), grouped by
// which download plugin's `url_pattern` matched, so the queue survives a page refresh or a
// different browser tab (the actual download itself already runs as shared server-side state).
// Manual file upload (left column) is unchanged — it's synchronous and has no equivalent
// review/queue step in legacy either.
export default function Upload() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const categories = useCategories()
  const downloadPlugins = usePlugins('download')
  const metadataPlugins = usePlugins('metadata')
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [category, setCategory] = useState('')
  const [rows, setRows] = useState<UploadRow[]>([])
  const [urls, setUrls] = useState('')
  const [unmatchedUrls, setUnmatchedUrls] = useState<string[]>([])
  const [uploadingCount, setUploadingCount] = useState(0)
  useApplyTheme()
  useDocumentTitle(t('Upload Center') ?? undefined)

  function upsertRow(key: string, patch: Partial<UploadRow>) {
    setRows((prev) => {
      const idx = prev.findIndex((r) => r.key === key)
      if (idx === -1) return [...prev, { key, name: key, state: 'processing', ...patch }]
      const next = [...prev]
      next[idx] = { ...next[idx], ...patch }
      return next
    })
  }

  async function handleUpload(toUpload: File) {
    const key = `upload-${Date.now()}-${toUpload.name}`
    upsertRow(key, { name: toUpload.name, state: 'processing' })
    setUploadingCount((n) => n + 1)
    try {
      const formData = new FormData()
      formData.append('file', toUpload)
      if (category) formData.append('catid', category)

      const response = await fetch('/api/archives/upload', { method: 'PUT', body: formData })
      const data = (await response.json()) as { success: number; error?: string; id?: string }

      if (data.success && data.id) {
        const meta = await fetchJson<ArchiveMetadata>(`/archives/${data.id}/metadata`).catch(() => null)
        upsertRow(key, { state: 'done', archiveId: data.id, title: meta?.title ?? toUpload.name })
        await queryClient.invalidateQueries({ queryKey: ['archives'] })
      } else {
        upsertRow(key, { state: 'error', message: data.error ?? t('unknown error') ?? '' })
      }
    } catch (e) {
      upsertRow(key, { state: 'error', message: String(e) })
    } finally {
      setUploadingCount((n) => n - 1)
      if (fileInputRef.current) fileInputRef.current.value = ''
    }
  }

  const addToQueue = useAddToQueue()

  async function handleAddToQueue() {
    const list = Array.from(new Set(urls.split('\n').map((u) => u.trim()).filter(Boolean)))
    if (list.length === 0) return

    const matched: { url: string; plugin: PluginInfo }[] = []
    const unmatched: string[] = []
    for (const url of list) {
      const plugin = findMatchingPlugin(downloadPlugins.data, url)
      if (plugin) matched.push({ url, plugin })
      else unmatched.push(url)
    }
    setUnmatchedUrls(unmatched)
    if (matched.length === 0) return

    const items = matched.map(({ url, plugin }) => ({
      url,
      plugin_namespace: plugin.namespace,
      category: category || undefined,
      metadataNamespace: findMatchingPlugin(metadataPlugins.data, url)?.namespace,
    }))
    await resolveDefaultsAndAdd(items)
    setUrls('')
  }

  // Resolving each URL's real checkbox defaults needs a per-plugin `usePluginOptions`/
  // `usePluginSettings` fetch, which can't happen inside a plain callback (hooks can't be called
  // conditionally/in a loop) — so defaults are resolved via direct one-off fetches here instead of
  // hooks, mirroring how the "Fetch metadata" preview button below also bypasses the query-hook
  // layer for a one-shot lookup.
  async function resolveDefaultsAndAdd(
    items: Array<{
      url: string
      plugin_namespace: string
      category?: string
      metadataNamespace?: string
    }>,
  ) {
    const resolved = await Promise.all(
      items.map(async (item) => {
        let autoFetch = false
        if (item.metadataNamespace) {
          const settings = await fetchJson<{ enabled: boolean }>(
            `/plugins/settings?namespace=${encodeURIComponent(item.metadataNamespace)}`,
          ).catch(() => null)
          autoFetch = settings?.enabled ?? false
        }
        const options = await fetchJson<{ overwrite_on_duplicate?: { value: boolean } }>(
          `/plugins/options?namespace=${encodeURIComponent(item.plugin_namespace)}`,
        ).catch(() => null)
        let overwrite: boolean
        if (options?.overwrite_on_duplicate) {
          overwrite = options.overwrite_on_duplicate.value
        } else {
          const settings = await fetchJson<{ replacedupe: boolean }>('/settings').catch(() => null)
          overwrite = settings?.replacedupe ?? false
        }
        return {
          url: item.url,
          plugin_namespace: item.plugin_namespace,
          category: item.category,
          auto_fetch_metadata: autoFetch,
          overwrite_on_duplicate: overwrite,
        }
      }),
    )
    await addToQueue.mutateAsync(resolved)
  }

  return (
    <div className="ido" style={{ textAlign: 'center', fontSize: FONT_SIZE_8PT }}>
      <h1 className="ih" style={{ textAlign: 'center' }}>
        {t('Adding Archives to the Library')}
      </h1>

      {t('Add files to your LANrurugi instance from your computer, or the Internet directly.')}
      <br />
      <br />

      <div style={{ marginLeft: 'auto', marginRight: 'auto' }}>
        <div className="left-column">
          {t('Add uploaded files to category:')}
          <select id="category" className="favtag-btn" value={category} onChange={(e) => setCategory(e.target.value)}>
            <option value="">{t(' -- No Category -- ')}</option>
            {categories.data?.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <br />
          <br />

          <h1 className="ih">{t('From your computer')}</h1>

          {t('You can drag and drop files into this window, or click the upload button.')}
          <br />
          <br />

          <span className="stdbtn fileinput-button" style={{ minHeight: 50, padding: '8px 12px' }}>
            <i className="fas fa-download fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
            <br />
            <span>{t('Add from your computer')}</span>
            <input
              ref={fileInputRef}
              type="file"
              name="file"
              accept=".zip,.cbz,.rar,.cbr,.7z,.cb7,.pdf,.epub"
              disabled={uploadingCount > 0}
              onChange={(e) => {
                const chosen = e.target.files?.[0] ?? null
                if (chosen) void handleUpload(chosen)
              }}
            />
          </span>

          <br />
          <br />
          <h1 className="ih">{t('From the Internet')}</h1>

          {t('You can download files from remote URLs directly into LANrurugi from here.')}
          <br />
          {t('Download jobs will keep going even if you close this window!')}
          <br />
          <br />

          {t('Type in your URLs (separated by a newline), and click the queue button.')}
          <br />
          {t("If a Downloader plugin is compatible with the URL, it'll be automatically used.")}
          <br />
          <br />

          <label htmlFor="urlForm">{t('URL(s) to download:')}</label>
          <br />
          <textarea
            id="urlForm"
            value={urls}
            onChange={(e) => setUrls(e.target.value)}
            style={{ width: 400, height: 100, whiteSpace: 'pre' }}
          />
          <br />
          <br />

          <span
            id="add-to-queue"
            className="stdbtn fileinput-button"
            style={{ minHeight: 50, padding: '8px 12px' }}
            onClick={() => !addToQueue.isPending && urls.trim() && void handleAddToQueue()}
          >
            <i className="fas fa-list fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
            <br />
            <span>{t('Add to Queue')}</span>
          </span>

          {unmatchedUrls.length > 0 && (
            <div style={{ marginTop: 12, textAlign: 'left', color: STATE_COLOR.failed }}>
              <i className="fa fa-exclamation-circle"></i>{' '}
              {t('No installed download plugin recognizes {{n}} URL(s):', { n: unmatchedUrls.length })}
              <ul style={{ margin: '4px 0 0', paddingLeft: 20 }}>
                {unmatchedUrls.map((u) => (
                  <li key={u} style={{ wordBreak: 'break-all' }}>
                    {u}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>

        <div className="right-column" style={{ paddingLeft: 24, boxSizing: 'border-box' }}>
          <h2 className="ih">
            {rows.length > 0 &&
              `🤔 ${t('Processing')}: ${rows.filter((r) => r.state === 'processing').length} 🙌 ${t('Completed')}: ${
                rows.filter((r) => r.state === 'done').length
              } 👹 ${t('Failed')}: ${rows.filter((r) => r.state === 'error').length}`}
          </h2>
          <table style={{ margin: 'auto', fontSize: FONT_SIZE_10PT, width: '80%', textAlign: 'center' }}>
            <tbody id="files">
              {rows.length === 0 ? (
                <tr>
                  <td colSpan={2}>
                    <div id="progress" style={{ paddingTop: 6, paddingBottom: 6 }}>
                      <div className="bar" style={{ width: '0%' }} />
                    </div>
                  </td>
                </tr>
              ) : (
                rows.map((row) => (
                  <tr key={row.key}>
                    <td style={{ maxWidth: 200, overflow: 'hidden', textOverflow: 'ellipsis' }}>
                      {row.state === 'done' && row.archiveId ? (
                        <a href={routes.reader(row.archiveId)} title={row.title ?? row.name}>
                          {row.title ?? row.name}
                        </a>
                      ) : (
                        <span title={row.name}>{row.name}</span>
                      )}
                    </td>
                    <td>
                      {row.state === 'processing' && (
                        <>
                          <i className="fas fa-spinner fa-spin"></i> {t('Processing your upload...')}
                        </>
                      )}
                      {row.state === 'done' && row.archiveId && (
                        <>
                          <i className="fas fa-check-circle"></i>{' '}
                          <a href={routes.edit(row.archiveId)} target="_blank" rel="noreferrer">
                            {t('Click here to edit metadata.')}
                          </a>
                        </>
                      )}
                      {row.state === 'error' && (
                        <>
                          <i className="fas fa-exclamation-circle"></i> {row.message}
                        </>
                      )}
                    </td>
                  </tr>
                ))
              )}
            </tbody>
          </table>

          <DownloadQueuePanel downloadPlugins={downloadPlugins.data} metadataPlugins={metadataPlugins.data} />
        </div>
      </div>

      <br />
      <br />
      <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}

/** Right-column panel: the persistent queue, grouped by `plugin_namespace` (one
 * `CollapsibleSection` per namespace present), with bulk Select All / Invert Selection / Start /
 * Clear Completed / Delete actions and per-item controls. A row is selectable while `queued` or
 * `error` — `error` had to become a startable state too (not just selectable-for-delete), so a
 * failed download (e.g. missing login credentials at the time it first ran) has a real way back to
 * running again once the underlying problem is fixed, instead of being permanently stuck. */
function DownloadQueuePanel({
  downloadPlugins,
  metadataPlugins,
}: {
  downloadPlugins: PluginInfo[] | undefined
  metadataPlugins: PluginInfo[] | undefined
}) {
  const { t } = useTranslation()
  const queue = useDownloadQueue()
  const jobs = useJobs()
  const settings = useSettings()
  const startSelected = useStartSelectedQueue()
  const deleteSelected = useDeleteSelectedQueue()
  const clearCompleted = useClearCompletedQueue()
  const updateForBatchMetadata = useUpdateQueueItem()
  const [selected, setSelected] = useState<Set<string>>(new Set())

  const items = useMemo(() => queue.data ?? [], [queue.data])
  const jobById = useMemo(() => {
    const map = new Map<string, JobRecord>()
    for (const j of jobs.data ?? []) map.set(j.id, j)
    return map
  }, [jobs.data])

  const grouped = useMemo(() => {
    const map = new Map<string, DownloadQueueItem[]>()
    for (const item of items) {
      const list = map.get(item.plugin_namespace) ?? []
      list.push(item)
      map.set(item.plugin_namespace, list)
    }
    return map
  }, [items])

  // Auto-fetch-metadata-on-completion: watches for a linked job reaching `finished` on an item
  // with `auto_fetch_metadata: true`, then fires the metadata preview call against the newly
  // created archive. Ref-guarded so this doesn't re-fire on every 3s/5s poll tick once triggered.
  const triggeredRef = useRef<Set<string>>(new Set())
  useEffect(() => {
    for (const item of items) {
      if (!item.auto_fetch_metadata || !item.job_id) continue
      if (triggeredRef.current.has(item.job_id)) continue
      const job = jobById.get(item.job_id)
      if (!job || job.state !== 'finished') continue
      const archiveIds = (job.result as { archive_ids?: string[] } | null)?.archive_ids
      const archiveId = archiveIds?.[0]
      if (!archiveId) continue
      const metadataPlugin = findMatchingPlugin(metadataPlugins, item.url)
      if (!metadataPlugin) continue
      triggeredRef.current.add(item.job_id)
      void (async () => {
        const result = await sendJson<{
          success: number
          data?: { tags?: string; title?: string; summary?: string }
        }>(
          'POST',
          `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&id=${encodeURIComponent(archiveId)}`,
        ).catch(() => null)
        // Mirrors `Controller::Batch::batch_plugin`'s persist-immediately semantics (no per-archive
        // staging step here either — this fires automatically post-download, with no form open for
        // the user to review through) — merges into whatever tags the fresh archive already has
        // (normally none yet) rather than assuming it's empty.
        if (!result?.success || !result.data) return
        const { tags: newTags, title, summary } = result.data
        if (!newTags && !(title && (settings.data?.replacetitles ?? true)) && !summary) return
        const archive = await fetchJson<ArchiveMetadata>(`/archives/${archiveId}/metadata`).catch(() => null)
        const mergedTags = newTags
          ? Array.from(
              new Set(
                [...(archive?.tags.split(',') ?? []), ...newTags.split(',')].map((tg) => tg.trim()).filter(Boolean),
              ),
            ).join(', ')
          : undefined
        await sendJson(
          'PUT',
          `/archives/${archiveId}/metadata?${new URLSearchParams({
            ...(mergedTags !== undefined && { tags: mergedTags }),
            ...(title && (settings.data?.replacetitles ?? true) && { title }),
            ...(summary && { summary }),
          })}`,
        )
      })()
    }
  }, [items, jobById, metadataPlugins, settings.data?.replacetitles])

  if (items.length === 0) return null

  // `error` is included alongside `queued` — both are valid starting states now (retrying a failed
  // download, e.g. after fixing a login plugin's saved credentials, reuses the exact same
  // start/select flow as a first attempt; see `start_one`'s own docs on the Rust side for why
  // `error`/`cancelled` had to become startable at all). `downloading`/`starting`/`done` are never
  // selectable.
  const selectableIds = items
    .filter((i) => i.state === 'queued' || i.state === 'error' || i.state === 'cancelled')
    .map((i) => i.id)
  // Matches the backend `clear_completed` handler's own filter exactly — `error` is deliberately
  // excluded (it's a restartable, still-actionable state, not "completed" work), so this button's
  // enabled/disabled state and displayed count don't imply it would also sweep up failed items.
  const cleared = items.filter((i) => i.state === 'done').length

  function selectAll() {
    setSelected(new Set(selectableIds))
  }

  function invertSelection() {
    setSelected((prev) => new Set(selectableIds.filter((id) => !prev.has(id))))
  }

  return (
    <div style={{ marginTop: 16, textAlign: 'left' }}>
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Download Queue')}
      </h2>

      {/* `.control-btn-group` (also used by Jobs.tsx/Duplicates.tsx) carries no actual CSS from
          either this app's own stylesheet or any of the 5 legacy theme files — it's a plain,
          unstyled class name, so `justifyContent`/`gap` here need `display: 'flex'` alongside them
          explicitly, or the group falls back to block layout. `flexWrap: 'wrap'` alone isn't
          enough to satisfy "never wrap": `.stdbtn` carries a theme `min-width: 150px`, and 5 of
          those (750px) plus gaps don't fit most viewports, so wrap was still happening — the real
          fix is overriding that min-width down to `0` and letting each button size to its own
          text (`width: 'auto'`) so all 5 actually fit on one row instead of merely being allowed
          to wrap when they don't. */}
      <div
        className="control-btn-group"
        style={{ display: 'flex', flexWrap: 'nowrap', justifyContent: 'center', gap: 4, marginBottom: 6 }}
      >
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selectableIds.length === 0}
          onClick={selectAll}
        >
          {t('Select All')}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selectableIds.length === 0}
          onClick={invertSelection}
        >
          {t('Invert Selection')}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selected.size === 0 || startSelected.isPending}
          onClick={async () => {
            const selectedIds = [...selected]
            await startSelected.mutateAsync(selectedIds)
            setSelected(new Set())
            // Same "fire alongside the real download, don't block it" behavior as the single-row
            // Start button's own `fetchMetadataOnStart` (see that function's own docs) — one call
            // per selected item, each independently fire-and-forget so a slow/failed metadata
            // fetch for one item never delays or breaks the others.
            for (const id of selectedIds) {
              const item = items.find((i) => i.id === id)
              if (!item) continue
              void fetchMetadataForItem(item, findMatchingPlugin(metadataPlugins, item.url), updateForBatchMetadata)
            }
          }}
        >
          {t('Start ({{n}})', { n: selected.size })}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={cleared === 0 || clearCompleted.isPending}
          onClick={() => void clearCompleted.mutateAsync()}
        >
          {t('Clear Completed')}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selected.size === 0 || deleteSelected.isPending}
          onClick={async () => {
            await deleteSelected.mutateAsync([...selected])
            setSelected(new Set())
          }}
        >
          {t('Delete ({{n}})', { n: selected.size })}
        </button>
      </div>

      <ul className="collapsible extensible with-right-caret" style={{ width: '100%' }}>
        {[...grouped.entries()].map(([namespace, groupItems]) => {
          const plugin = downloadPlugins?.find((p) => p.namespace === namespace)
          return (
            <CollapsibleSection
              key={namespace}
              icon="fa-cloud-download-alt"
              title={`${plugin?.name ?? namespace} (${groupItems.length})`}
              caretStyle="right-down"
              defaultOpen
            >
              {groupItems.map((item) => (
                <QueueItemRow
                  key={item.id}
                  item={item}
                  job={item.job_id ? jobById.get(item.job_id) : undefined}
                  selected={selected.has(item.id)}
                  onToggleSelect={() => {
                    if (item.state !== 'queued' && item.state !== 'error' && item.state !== 'cancelled')
                      return
                    setSelected((prev) => {
                      const next = new Set(prev)
                      if (next.has(item.id)) next.delete(item.id)
                      else next.add(item.id)
                      return next
                    })
                  }}
                  metadataPlugin={findMatchingPlugin(metadataPlugins, item.url)}
                />
              ))}
            </CollapsibleSection>
          )
        })}
      </ul>
    </div>
  )
}

function QueueItemRow({
  item,
  job,
  selected,
  onToggleSelect,
  metadataPlugin,
}: {
  item: DownloadQueueItem
  job: JobRecord | undefined
  selected: boolean
  onToggleSelect: () => void
  metadataPlugin: PluginInfo | null
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const update = useUpdateQueueItem()
  const start = useStartQueueItem()
  const stop = useStopQueueItem()
  const del = useDeleteQueueItem()
  const overwriteConflict = useOverwriteQueueItem()
  const renameConflict = useRenameQueueItem()
  // Holds the trigger button's own `getBoundingClientRect()`, captured at click time (an
  // event-handler read, not a render-time `ref.current` read — this app's ESLint config forbids
  // the latter, `react-hooks/refs`) — `null` means closed.
  const [conflictMenuAnchor, setConflictMenuAnchor] = useState<DOMRect | null>(null)
  const [renamePopover, setRenamePopover] = useState<{ x: number; y: number } | null>(null)
  const [fetchingMetadata, setFetchingMetadata] = useState(false)
  // Same shape the auto-fetch-metadata effect above already reads from a finished job's result
  // (`{archive_ids?: string[]}`) — here just to make a `done` item's title clickable through to
  // the archive it actually became, matching every other archive-title link in this app.
  const archiveId = (job?.result as { archive_ids?: string[] } | null)?.archive_ids?.[0]
  // `cancelled` is a real, persisted queue state (survives a page refresh) — `useStopQueueItem`'s
  // own optimistic `onMutate` write already sets this in the cache the instant Stop is clicked,
  // before the request even resolves, so this reads as immediate despite being server state.
  const wasCancelled = item.state === 'cancelled'

  async function handleFetchMetadata() {
    if (!metadataPlugin) return
    setFetchingMetadata(true)
    try {
      await fetchMetadataForItem(item, metadataPlugin, update)
    } finally {
      setFetchingMetadata(false)
    }
  }

  // Fired alongside Start/Retry (not awaited — runs concurrently with the real download, not
  // before/blocking it) so the row's title/tooltip fill in with real metadata while the transfer
  // is still in progress, instead of showing only the bare URL for the download's entire duration.
  // A no-op when there's no metadata plugin for this URL, or metadata was already fetched (either
  // manually via the tags button beforehand, or by an earlier start-triggered call — e.g. after a
  // Stop + restart cycle).
  function fetchMetadataOnStart() {
    if (!metadataPlugin || item.metadata_preview || fetchingMetadata) return
    void handleFetchMetadata()
  }

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 4,
        padding: '4px 2px',
        borderTop: '1px solid rgba(128,128,128,0.2)',
        flexWrap: 'wrap',
      }}
    >
      <input
        type="checkbox"
        checked={selected}
        disabled={item.state !== 'queued' && item.state !== 'error' && item.state !== 'cancelled'}
        onChange={onToggleSelect}
      />

      <TooltipIfPresent preview={item.metadata_preview} url={item.url} wrapperStyle={{ flex: '1 1 180px', minWidth: 0 }}>
        <div
          style={{
            width: '100%',
            boxSizing: 'border-box',
            border: '1px solid rgba(128,128,128,0.3)',
            borderRadius: 4,
            padding: '2px 6px',
          }}
        >
          {item.state === 'downloading' || item.state === 'starting' ? (
            <>
              {/* Title/URL stays visible above the progress bar (previously fully replaced by
                  `JobProgressBar`, which only ever renders byte-progress — losing sight of *which*
                  item is downloading, and burying the `TooltipIfPresent` trigger since the title
                  text it wraps was gone from the DOM). Matches the plain-text rendering used for
                  every other state below. */}
              <span style={{ fontSize: FONT_SIZE_10PT, wordBreak: 'break-all', display: 'block' }} title={item.metadata_preview ? undefined : item.url}>
                {item.title ?? item.url}
              </span>
              {job ? (
                <JobProgressBar job={job} />
              ) : (
                <span style={{ fontSize: FONT_SIZE_10PT }}>{t('Starting…')}</span>
              )}
            </>
          ) : item.state === 'done' ? (
            // A plain full green bar with the title overlaid on top (not beside it, which felt
            // cramped — explicit user call), not `JobProgressBar` (which always renders the
            // byte-count/speed detail line) — once a download is actually done, that detail is
            // just noise: no one needs to see "50.9 MB / 101.3 MB (100%) · 0 B/s" for a finished
            // item. Earlier versions tried reusing `JobProgressBar` here (first grey via
            // `STATE_COLOR.finished`, then green, then a same-row bar+text pair) before landing on
            // this overlay treatment.
            <div style={{ position: 'relative', height: 18, borderRadius: 4, overflow: 'hidden', background: STATE_COLOR.active }}>
              {/* Clickable through to the reader once cataloged (explicit user call) — matches
                  every other archive-title link in this app (`Library.tsx`'s cards, `Upload.tsx`'s
                  own file-upload rows below). `archiveId` can be absent (job result not yet
                  polled-in, or a `file_path` fallback result with no `archive_ids`), in which case
                  this stays plain text rather than a dead link. */}
              <a
                href={archiveId ? routes.reader(archiveId) : undefined}
                onClick={(e) => {
                  if (!archiveId) return
                  e.preventDefault()
                  navigate(routes.reader(archiveId))
                }}
                style={{
                  position: 'absolute',
                  inset: 0,
                  display: 'flex',
                  alignItems: 'center',
                  padding: '0 6px',
                  fontSize: FONT_SIZE_10PT,
                  color: '#fff',
                  textShadow: '0 1px 2px rgba(0,0,0,0.6)',
                  whiteSpace: 'nowrap',
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                  cursor: archiveId ? 'pointer' : 'default',
                }}
              >
                {item.title ?? item.url}
                {job?.total_bytes != null && (
                  <span style={{ marginLeft: 6, opacity: 0.85, flexShrink: 0 }}>({formatBytes(job.total_bytes)})</span>
                )}
              </a>
            </div>
          ) : (
            <span style={{ fontSize: FONT_SIZE_10PT, wordBreak: 'break-all' }} title={item.metadata_preview ? undefined : item.url}>
              {item.title ?? item.url}
            </span>
          )}
          {item.state === 'error' && item.error && (
            <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>
              <QueueErrorText error={item.error} />
            </div>
          )}
          {/* A stopped download gets its own real, persisted state (`cancelled`, distinct from
              `queued`) precisely so this label — and the Retry-button treatment below — survive a
              page refresh instead of only living in this mutation's own transient local state. */}
          {wasCancelled && (
            <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>{t('Cancelled')}</div>
          )}
        </div>
      </TooltipIfPresent>

      <Tooltip label={t('Auto Fetch Metadata') ?? ''}>
        <input
          type="checkbox"
          checked={item.auto_fetch_metadata}
          disabled={item.state !== 'queued'}
          onChange={(e) => void update.mutateAsync({ id: item.id, auto_fetch_metadata: e.target.checked })}
        />
      </Tooltip>

      <Tooltip label={t('Overwrite Duplicate') ?? ''}>
        <input
          type="checkbox"
          checked={item.overwrite_on_duplicate}
          disabled={item.state !== 'queued'}
          onChange={(e) => void update.mutateAsync({ id: item.id, overwrite_on_duplicate: e.target.checked })}
        />
      </Tooltip>

      {item.pending_filename_conflict ? (
        // A `Filename` collision (content is genuinely new, only the resolved filename collides —
        // see `PendingFilenameConflict`'s own docs) has a real choice, unlike a `ContentHash`
        // collision (`QueueError::DuplicateArchive`, unconditionally rejected — no buttons render
        // for that case at all, just the plain error text below). The already-downloaded bytes are
        // staged server-side either way, so neither action re-downloads anything. Both resolutions
        // share one trigger button (a dropdown, not two separate icon buttons) — the same visual
        // slot the row's other single-purpose action buttons occupy.
        <>
          <Tooltip label={t('Resolve Conflict') ?? ''}>
            <button
              type="button"
              className="stdbtn"
              style={ICON_BUTTON_STYLE}
              disabled={overwriteConflict.isPending || renameConflict.isPending}
              onClick={(e) => {
                // `e.currentTarget` must be read synchronously here, not inside the updater
                // function below — React nulls it out by the time that callback runs.
                const rect = e.currentTarget.getBoundingClientRect()
                setConflictMenuAnchor((open) => (open ? null : rect))
              }}
            >
              <i className="fa fa-clone" aria-hidden="true"></i>
            </button>
          </Tooltip>
          {conflictMenuAnchor && (
            <>
              <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={() => setConflictMenuAnchor(null)} />
              <ConflictMenu
                anchor={conflictMenuAnchor}
                onOverwrite={() => {
                  setConflictMenuAnchor(null)
                  void overwriteConflict.mutateAsync(item.id)
                }}
                onRename={() => {
                  // Anchored off the trigger button's own rect (already captured above), not the
                  // menu item that was clicked — that item disappears the instant `ConflictMenu`
                  // closes, so anchoring off it made the popover feel like it drifted from
                  // whatever position the (now-gone) item happened to occupy.
                  setRenamePopover({ x: conflictMenuAnchor.left, y: conflictMenuAnchor.bottom })
                  setConflictMenuAnchor(null)
                }}
              />
            </>
          )}
          {renamePopover && (
            <RenamePopover
              anchor={renamePopover}
              conflict={item.pending_filename_conflict}
              itemTitle={item.title}
              itemNamespace={item.plugin_namespace}
              pending={renameConflict.isPending}
              onCancel={() => setRenamePopover(null)}
              onConfirm={(filename) => {
                setRenamePopover(null)
                void renameConflict.mutateAsync({ id: item.id, filename })
              }}
            />
          )}
        </>
      ) : item.state === 'starting' || item.state === 'downloading' ? (
        <Tooltip label={t('Stop') ?? ''}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={stop.isPending}
            onClick={() => void stop.mutateAsync(item.id)}
          >
            <i className="fa fa-stop" aria-hidden="true"></i>
          </button>
        </Tooltip>
      ) : (
        // `cancelled` is its own restartable state (see `wasCancelled`'s own docs) — treated as a
        // restart (Retry wording/icon), same as `error`, rather than a fresh download.
        <Tooltip label={(item.state === 'error' || wasCancelled ? t('Retry') : t('Download')) ?? ''}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={
              (item.state !== 'queued' && item.state !== 'error' && item.state !== 'cancelled') ||
              start.isPending
            }
            onClick={() => {
              void start.mutateAsync(item.id)
              fetchMetadataOnStart()
            }}
          >
            <i
              className={`fa ${item.state === 'error' || wasCancelled ? 'fa-redo' : 'fa-download'}`}
              aria-hidden="true"
            ></i>
          </button>
        </Tooltip>
      )}

      <Tooltip
        label={
          metadataPlugin
            ? `${t('Fetch Metadata')} (${metadataPlugin.name})`
            : (t('Fetch Metadata') ?? '')
        }
      >
        <button
          type="button"
          className="stdbtn"
          style={ICON_BUTTON_STYLE}
          disabled={!metadataPlugin || item.state === 'done' || fetchingMetadata}
          onClick={() => void handleFetchMetadata()}
        >
          <i className={`fa ${fetchingMetadata ? 'fa-spinner fa-spin' : 'fa-tags'}`} aria-hidden="true"></i>
        </button>
      </Tooltip>

      {/* `DELETE /download_queue/{id}` only ever removes this queue-history row — it never touches
          the actual cataloged archive a `done` item became (that's `DELETE /archives/{id}`, a
          completely separate, unrelated endpoint). "Delete"/`fa-times` on a `done` row reads as
          "delete the archive I just downloaded", which it explicitly is not — relabeled "Remove"
          with a distinct icon for that one state (matching this page's own "清除已完成" bulk
          action's own real semantics) so the wording doesn't imply a destructive action this
          button was never capable of. */}
      <Tooltip label={(item.state === 'done' ? t('Remove') : t('Delete')) ?? ''}>
        <button
          type="button"
          className="stdbtn"
          style={ICON_BUTTON_STYLE}
          // Deleting an in-flight item would rip its Redis entry out from under the still-running
          // download task (`update_queue_item_state`'s writes afterward silently no-op against a
          // gone item — see that function's own docs) with no way to stop the transfer or see its
          // progress afterward. Must Stop first, matching the same restartable-states set the
          // Download/Retry button itself allows starting from.
          disabled={
            del.isPending || item.state === 'starting' || item.state === 'downloading'
          }
          onClick={() => void del.mutateAsync(item.id)}
        >
          <i className={`fa ${item.state === 'done' ? 'fa-eraser' : 'fa-times'}`} aria-hidden="true"></i>
        </button>
      </Tooltip>
    </div>
  )
}

/** Common compound extensions hardcoded, not user-configurable — real occurrences of this in this
 * app's actual corpus (archive-download plugin output) are rare enough that a whole settings
 * surface for it would be over-engineering. Checked longest-first purely so a filename that
 * somehow matched more than one entry always takes the more specific/longer one (not currently
 * possible with this exact list, but keeps the ordering intentional rather than incidental). */
const COMPOUND_EXTENSIONS = ['tar.gz', 'tar.bz2', 'tar.xz', 'tar.zst']

/** Splits a filename into its stem and extension — matches Rust's `Path::file_stem()`/
 * `extension()` for an ordinary single-segment extension, but recognizes `COMPOUND_EXTENSIONS` as
 * one whole unit (`archive.tar.gz` → `{stem: "archive", ext: "tar.gz"}`, not `{stem:
 * "archive.tar", ext: "gz"}`) — otherwise `{filename}`/`{ext}` (the template variables this feeds)
 * would silently mangle a `.tar.gz` name, leaving half the real suffix stuck onto `{filename}`
 * instead of `{ext}`. `{filename}` (the template variable) is deliberately the stem only, not the
 * full original name, since the default template `{filename}_{crc}.{ext}` already appends the
 * extension back on separately (explicit user call: this way the default never accidentally
 * duplicates it, e.g. `archive.zip_a1b2c3d4.zip`). */
function splitFilenameStemAndExt(filename: string): { stem: string; ext: string } {
  for (const compound of COMPOUND_EXTENSIONS) {
    const suffix = `.${compound}`
    const start = filename.length - suffix.length
    if (start > 0 && filename.endsWith(suffix)) {
      return { stem: filename.slice(0, start), ext: compound }
    }
  }
  const lastDot = filename.lastIndexOf('.')
  if (lastDot <= 0) return { stem: filename, ext: '' }
  return { stem: filename.slice(0, lastDot), ext: filename.slice(lastDot + 1) }
}

const TEMPLATE_VARS = [
  'filename',
  'crc',
  'title',
  'ext',
  'date-yyyymmdd',
  'date-yyyy-mm-dd',
  'namespace',
] as const

/** `YYYYMMDD`/`YYYY-MM-DD` (local time, computed fresh each render — this is the moment the
 * rename is happening, not the original download's own timestamp) — the padStart calls are
 * needed since `getMonth()`/`getDate()` return unpadded single digits for January-September/the
 * 1st-9th. Keyed by the exact `date-*` template-variable suffix so `substituteFilenameTemplate`'s
 * lookup is a plain object-key hit, no per-call parsing of the suffix needed. */
function dateTemplateValues(): Record<string, string> {
  const now = new Date()
  const yyyy = String(now.getFullYear())
  const mm = String(now.getMonth() + 1).padStart(2, '0')
  const dd = String(now.getDate()).padStart(2, '0')
  return {
    'date-yyyymmdd': `${yyyy}${mm}${dd}`,
    'date-yyyy-mm-dd': `${yyyy}-${mm}-${dd}`,
  }
}

/** Replaces every `{filename}`/`{crc}`/`{title}`/`{ext}`/`{date-*}`/`{namespace}` occurrence in
 * `template` with its real value — an unrecognized `{...}` placeholder is left as-is rather than
 * silently dropped, so a typo reads as "this literally didn't get replaced" instead of vanishing
 * without a trace. Matches `[\w-]+` (not just `\w+`) so the hyphenated `date-yyyymmdd`/
 * `date-yyyy-mm-dd` variable names are recognized at all. */
function substituteFilenameTemplate(template: string, vars: Record<string, string>): string {
  return template.replace(/\{([\w-]+)\}/g, (match, key: string) => vars[key] ?? match)
}

/** What a Shift-click on a template-variable button inserts — plain-wrapped-in-parentheses
 * (`({var})`) for every variable except `ext`: a file extension is never meaningfully wrapped in
 * parentheses (nobody names a file `archive(zip)`), but it does need its own leading `.` when
 * inserted this way, since a plain click already omits one (`{filename}_{crc}.{ext}`'s own literal
 * `.` between `{crc}` and `{ext}` is typed by hand, not part of either variable) — `.{ext}` is the
 * one insertion this button can make that's actually useful on its own, appended directly after
 * whatever the user already typed. */
function shiftClickInsertion(key: string): string {
  return key === 'ext' ? '.{ext}' : `({${key}})`
}

/** The dropdown that opens off the filename-conflict row's single trigger button, offering both
 * resolutions for a `PendingFilenameConflict` (see that type's own docs) — "Overwrite" runs
 * immediately, "Rename and Catalog" just signals the caller to open {@link RenamePopover} (the
 * caller anchors it off `anchor` itself — the trigger button's own rect, captured once when this
 * menu was opened — rather than off whichever menu item was clicked, since that item disappears
 * the instant this menu closes and would make the popover feel like it drifted from a
 * now-nonexistent position). Positioned off `anchor` the same viewport-aware way `RenamePopover`
 * itself is — see that component's own docs for why a plain `rect.bottom`/`rect.left` isn't
 * enough. */
function ConflictMenu({
  anchor,
  onOverwrite,
  onRename,
}: {
  anchor: DOMRect
  onOverwrite: () => void
  onRename: () => void
}) {
  const { t } = useTranslation()
  const pos = anchoredPosition(anchor, 160)
  return (
    <PopupMenu style={{ position: 'fixed', top: pos.top, left: pos.left, zIndex: Z_OVERLAY_CONTENT }}>
      <PopupMenuItem onClick={onOverwrite}>
        <i className="fa fa-clone" aria-hidden="true" style={{ marginRight: 6 }}></i>
        {t('Overwrite')}
      </PopupMenuItem>
      <PopupMenuItem onClick={onRename}>
        <i className="fa fa-i-cursor" aria-hidden="true" style={{ marginRight: 6 }}></i>
        {t('Rename and Catalog')}
      </PopupMenuItem>
    </PopupMenu>
  )
}

/** Picks a `{top, left}` for a `width`-wide floating panel anchored below `rect` (its trigger's
 * own `getBoundingClientRect()`).
 *
 * `preferCenter: false` (the default — regular dropdown-menu behavior, used by `ConflictMenu`):
 * always left-aligns with the trigger, flipping to right-aligned only when left-aligned genuinely
 * doesn't fit. A small menu anchored right at its trigger reads as "opened from this button"; the
 * moment it drifts toward the window's center instead, it stops looking anchored to anything.
 *
 * `preferCenter: true` (used by `RenamePopover`, a much wider standalone form panel, not a menu
 * hugging its trigger): picks whichever side actually pulls the panel closer to the *center* of
 * the viewport when both directions have room — a trigger that's already past the horizontal
 * midpoint (routine here: these triggers live inside a right-aligned-icon download-queue row)
 * opens left-of-button, not right-of-button, since a right-aligned trigger opening rightward still
 * visually hugs the window edge even when it technically "fits". Only falls back to whichever side
 * doesn't fit when the preferred side genuinely has no room at all.
 *
 * Both modes flip above the trigger when there isn't enough room below, the same way. */
function anchoredPosition(
  rect: DOMRect,
  width: number,
  preferCenter = false,
): { top: number; left: number } {
  const margin = 8
  const estimatedHeight = 220
  const fitsLeftAligned = rect.left + width + margin <= window.innerWidth
  const fitsRightAligned = rect.right - width >= margin
  const preferLeftAligned = preferCenter
    ? (rect.left + rect.right) / 2 <= window.innerWidth / 2
    : true
  const useLeftAligned = preferLeftAligned ? fitsLeftAligned || !fitsRightAligned : fitsLeftAligned && !fitsRightAligned
  const left = useLeftAligned ? rect.left : Math.max(rect.right - width, margin)
  const spaceBelow = window.innerHeight - rect.bottom
  const top =
    spaceBelow >= estimatedHeight || spaceBelow >= rect.top
      ? rect.bottom + 4
      : Math.max(rect.top - estimatedHeight - 4, margin)
  return { top, left }
}

/** One piece of a parsed template string — either literal, freely-editable text, or a `{var}`/
 * `({var})`/`.{var}` token rendered as an atomic, non-editable chip (with its own `×` remove
 * button). Mirrors how a real mail-merge/mention editor represents a mixed text+placeholder
 * string internally, rather than trying to treat the whole thing as one opaque string the way the
 * plain-`<input>` version of this popover used to. */
type TemplateSegment = { type: 'text'; value: string } | { type: 'token'; value: string }

/** A real, zero-visual-width but genuinely-present text-node character — inserted immediately
 * before and after every chip `<span>` in `renderSegments` so a native browser caret has an actual
 * text-node anchor to land in/near a chip boundary. Two `contentEditable={false}` chip `<span>`s
 * placed directly adjacent (or a chip at the very start/end of the editor, adjacent only to the
 * container edge) give the browser's native caret-placement logic no text-node landing spot at
 * all — confirmed live: clicking directly between two adjacent chips (or before the first/after
 * the last) either silently fails to place a visible caret, or places one that's effectively
 * invisible in the ~2px gap chip margins leave (a real, reported bug — widening that margin alone
 * wouldn't fix the underlying "no text node here" cause, only make the same invisible-caret gap
 * wider). Filtered back out in `extractTemplateFromDom`/skipped in `setCursorAtOffset`'s length
 * accounting, so it never leaks into the real template string this editor represents. */
const CURSOR_ANCHOR = '\u200b'

/** Splits a template string into alternating text/token segments — `token` segments capture the
 * optional wrapping `(`/`)` and leading `.` a Shift-click insertion can add (see
 * `shiftClickInsertion`), so the whole thing (`({crc})`, `.{ext}`, `{filename}`) round-trips back
 * into `substituteFilenameTemplate` unchanged as one atomic unit. */
function parseTemplateSegments(template: string): TemplateSegment[] {
  const segments: TemplateSegment[] = []
  let lastIndex = 0
  for (const match of template.matchAll(/\.?\(?\{[\w-]+\}\)?/g)) {
    const start = match.index
    if (start > lastIndex) {
      segments.push({ type: 'text', value: template.slice(lastIndex, start) })
    }
    segments.push({ type: 'token', value: match[0] })
    lastIndex = start + match[0].length
  }
  if (lastIndex < template.length) {
    segments.push({ type: 'text', value: template.slice(lastIndex) })
  }
  return segments
}

/** A real mixed text/chip editor for a filename template string — a `contentEditable` `<div>`
 * (not a plain `<input>`) since a token needs to render as an atomic, visually distinct chip with
 * its own `×` button sitting *inline* among freely-typed text on either side of it, which a plain
 * `<input>`'s single text node fundamentally can't represent. Each chip is
 * `contentEditable={false}` (so the browser's own cursor/selection engine treats it as one
 * indivisible unit — arrow keys skip over it in one step, Backspace at its edge deletes the whole
 * thing, exactly like a real mention/tag in Gmail or Notion) sitting inside the outer
 * `contentEditable="true"` container, which handles everything else (typing, arrow keys, text
 * selection) via native browser behavior — no custom Selection/Range bookkeeping for the
 * plain-text parts at all.
 *
 * Re-parses `template` into segments on every render (`parseTemplateSegments`) and re-renders the
 * whole DOM subtree from that — a controlled `contentEditable` in the same sense a controlled
 * `<input>` is, just with a heavier DOM diff. `onInput` reads the live DOM back out
 * (`extractTemplateFromDom`) rather than trying to track incremental text-node edits. */
function TemplateInput({
  template,
  onChange,
  onInsert,
}: {
  template: string
  onChange: (next: string) => void
  /** Called once (on mount) with this editor's real `insert(text)` function — the caller stashes
   * it (e.g. in a ref) and calls it later from its own template-variable buttons. */
  onInsert: (insert: (text: string) => void) => void
}) {
  const palette = useMenuPalette()
  const editorRef = useRef<HTMLDivElement>(null)
  // Sidesteps a real feedback loop: `onInput` calls `onChange`, which flows back down as a new
  // `template` prop — without this flag, the effect that re-renders the DOM from `template` would
  // then stomp on the very keystroke that triggered it (the DOM already reflects the typed
  // character; re-rendering from `template` at that point is redundant at best, and can steal
  // focus/collapse the selection mid-composition at worst). Set right before `onChange` fires,
  // cleared once the resulting re-render's effect has run.
  const skipNextSyncRef = useRef(false)
  // Set by the `×` remove button (to the deleted chip's own former character offset in the flat
  // template string) right before it calls `onChange` — consumed by the `renderSegments` effect
  // right after it rebuilds the DOM, so the cursor lands exactly where the chip used to be rather
  // than wherever the browser happens to default to after an `innerHTML` rebuild (its start, in
  // practice). Placing the cursor there — not e.g. at the end — is what makes "delete a chip, type
  // or pick a replacement variable right where it was" a smooth one-motion "replace" gesture
  // instead of "delete, then hunt for where the cursor ended up."
  const pendingCursorOffsetRef = useRef<number | null>(null)

  /** Places the caret at flat character offset `offset` (as `extractTemplateFromDom` would count
   * it) by walking the editor's direct children, which are always either text nodes or one flat
   * level of chip `<span>`s (never nested) — a plain linear scan tracking how many flat characters
   * each child accounts for. Landing exactly on a chip boundary collapses to just after it (chips
   * are `contentEditable={false}`, so a native caret can never be placed *inside* one anyway). */
  function setCursorAtOffset(offset: number) {
    const root = editorRef.current
    if (!root) return
    const selection = window.getSelection()
    if (!selection) return
    let remaining = offset
    for (const node of root.childNodes) {
      // A pure `CURSOR_ANCHOR` text node (see that constant's own docs) contributes zero real
      // characters — it's a rendering-only caret landing spot, not part of the flat template
      // string `offset` is measured against, so it's skipped entirely rather than treated as a
      // normal zero/one-character text node candidate for the caret to land in.
      if (node.nodeType === Node.TEXT_NODE && node.textContent === CURSOR_ANCHOR) continue
      const length =
        node.nodeType === Node.TEXT_NODE
          ? (node.textContent?.length ?? 0)
          : node instanceof HTMLElement
            ? (node.dataset.token?.length ?? 0)
            : 0
      if (remaining <= length) {
        const range = document.createRange()
        if (node.nodeType === Node.TEXT_NODE) {
          range.setStart(node, remaining)
        } else {
          // A chip boundary (`remaining` is 0 or equals this chip's own length) — position the
          // range immediately before/after the chip node itself, since text offsets don't apply
          // inside an atomic, non-text chip.
          const parent = node.parentNode
          if (!parent) return
          const index = Array.prototype.indexOf.call(parent.childNodes, node)
          range.setStart(parent, remaining === 0 ? index : index + 1)
        }
        range.collapse(true)
        selection.removeAllRanges()
        selection.addRange(range)
        return
      }
      remaining -= length
    }
    // `offset` was past the end of every child (e.g. the deleted chip was the last segment) —
    // collapse to the very end of the editor instead.
    const range = document.createRange()
    range.selectNodeContents(root)
    range.collapse(false)
    selection.removeAllRanges()
    selection.addRange(range)
  }

  /** Reads the live DOM back into a flat template string — chips carry their own literal token
   * text on a `data-token` attribute (rather than reading `textContent`, which would include the
   * `×` button's own label) added by `renderSegments` below. Strips out every `CURSOR_ANCHOR`
   * character (the zero-width text nodes `renderSegments` plants around each chip purely so the
   * browser has somewhere to put a real caret) — those are a rendering-layer implementation
   * detail, never part of the actual template content this editor represents. */
  function extractTemplateFromDom(): string {
    const root = editorRef.current
    if (!root) return template
    let result = ''
    for (const node of root.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        result += node.textContent ?? ''
      } else if (node instanceof HTMLElement && node.dataset.token) {
        result += node.dataset.token
      }
    }
    return result.split(CURSOR_ANCHOR).join('')
  }

  function renderSegments() {
    const root = editorRef.current
    if (!root) return
    root.innerHTML = ''
    for (const segment of parseTemplateSegments(template)) {
      if (segment.type === 'text') {
        root.appendChild(document.createTextNode(segment.value))
        continue
      }
      // A real, empty-looking text node immediately before this chip — see `CURSOR_ANCHOR`'s own
      // docs for why a chip needs one on both sides. Only actually needed when there's no real
      // text node already adjacent (two chips back-to-back, or a chip at the very start/end of
      // the editor) — but adding it unconditionally is simpler and harmless: an extra zero-width
      // anchor next to a real text node costs nothing visually or functionally.
      root.appendChild(document.createTextNode(CURSOR_ANCHOR))
      const chip = document.createElement('span')
      chip.contentEditable = 'false'
      chip.dataset.token = segment.value
      chip.style.display = 'inline-flex'
      chip.style.alignItems = 'center'
      chip.style.gap = '3px'
      chip.style.padding = '0 2px 0 5px'
      // Wider than the original `1px` — the cursor-anchor text node between two adjacent chips
      // (see `CURSOR_ANCHOR`'s own docs) now makes the caret placeable there at all, but a mouse
      // still has to physically land a click within that gap, and `1px` on either side left only
      // ~2px of real clickable width between two chips — reported as still awkward to hit even
      // after the caret-placement fix itself. `3px` roughly triples that usable gap.
      chip.style.margin = '0 3px'
      chip.style.borderRadius = '3px'
      chip.style.background = 'rgba(0,0,0,0.08)'
      // A neutral, low-opacity grey — not `palette.border`/`palette.text` (this app's own themed
      // accent colors, e.g. `g.css`'s dark red `#5C0D11`), which reads too close to the browser's
      // own (usually near-black/dark) text-cursor color and made a chip's edge easy to mistake
      // for the caret sitting right next to it. Matches the template-variable insertion buttons'
      // own already-established border color just below.
      chip.style.border = '1px solid rgba(128,128,128,0.4)'
      chip.style.fontFamily = 'monospace'
      chip.style.userSelect = 'none'
      chip.style.whiteSpace = 'nowrap'
      chip.onmouseenter = () => {
        chip.style.background = 'rgba(0,0,0,0.16)'
      }
      chip.onmouseleave = () => {
        chip.style.background = 'rgba(0,0,0,0.08)'
      }

      const label = document.createElement('span')
      label.textContent = segment.value
      chip.appendChild(label)

      // A real Font Awesome icon (`fa-times`, this app's own established "remove/delete" icon —
      // see e.g. the queue row's own delete button) rather than a plain `×` character glyph:
      // reported as easy to mistake for literal typed text sitting inside the chip rather than a
      // clickable remove control. Colored with the theme's own destructive-looking accent
      // (`palette.border`, falling back to `palette.text` the same way the chip's own border
      // already does on the 2 themes where `border` is `transparent`) instead of `currentColor`,
      // so it visually reads as "a distinct button" rather than blending into the chip's own text.
      const removeBtn = document.createElement('button')
      removeBtn.type = 'button'
      removeBtn.innerHTML = '<i class="fa fa-times" aria-hidden="true"></i>'
      removeBtn.setAttribute('aria-label', 'Remove')
      removeBtn.style.border = 'none'
      removeBtn.style.background = 'none'
      removeBtn.style.padding = '0'
      removeBtn.style.margin = '0'
      removeBtn.style.cursor = 'pointer'
      removeBtn.style.color = palette.border === 'transparent' ? palette.text : palette.border
      removeBtn.style.fontSize = '0.85em'
      removeBtn.style.lineHeight = '1'
      // Deliberately does NOT set `skipNextSyncRef` — unlike a normal keystroke (where the DOM
      // already reflects the edit and a re-render would be redundant/disruptive), removing a chip
      // needs the DOM to actually change: the chip's own DOM node must be un-rendered, which only
      // the `template`-driven effect below (calling `renderSegments`) does.
      removeBtn.onclick = (e) => {
        e.preventDefault()
        e.stopPropagation()
        const tokenStart = template.indexOf(segment.value)
        pendingCursorOffsetRef.current = tokenStart
        onChange(template.slice(0, tokenStart) + template.slice(tokenStart + segment.value.length))
      }
      chip.appendChild(removeBtn)
      root.appendChild(chip)
      root.appendChild(document.createTextNode(CURSOR_ANCHOR))
    }
  }

  useEffect(() => {
    if (skipNextSyncRef.current) {
      skipNextSyncRef.current = false
      return
    }
    renderSegments()
    if (pendingCursorOffsetRef.current !== null) {
      editorRef.current?.focus()
      setCursorAtOffset(pendingCursorOffsetRef.current)
      pendingCursorOffsetRef.current = null
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [template])

  useEffect(() => {
    renderSegments()
    editorRef.current?.focus()
    // Places the initial cursor right before the extension (the `.` of `.{ext}`), not wherever
    // the browser defaults an `innerHTML`-rebuilt `contentEditable` to (its very start, in
    // practice) — the filename stem/CRC portion is what a user typically wants to actually look
    // at/edit first, with the extension itself rarely touched. Only correct because the default
    // template's own shape (`{filename}_{crc}.{ext}`) is fixed, not derived per-file — the offset
    // right before its own trailing `.{ext}` token is always the same regardless of what the real
    // extension turns out to be, so this doesn't need to know anything about the actual filename
    // (compound extensions like `tar.gz` are already handled by `splitFilenameStemAndExt` itself,
    // which feeds the whole `{ext}` value as one atomic token either way).
    setCursorAtOffset(template.length - '.{ext}'.length)
    // Runs once on mount only — subsequent re-renders are handled by the `template`-keyed effect
    // above; re-running this on every render would double-render the same segments.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    // Registers an `insert(text)` function the parent (the template-variable buttons) calls —
    // inserts at the current DOM selection (falling back to the end when the editor doesn't have
    // focus/a real selection inside it, e.g. right after this popover first opens and a button is
    // clicked before ever touching the editor itself).
    onInsert((text) => {
      const root = editorRef.current
      if (!root) return
      const selection = window.getSelection()
      const range =
        selection && selection.rangeCount > 0 && root.contains(selection.anchorNode)
          ? selection.getRangeAt(0)
          : null
      if (range) {
        range.deleteContents()
        range.insertNode(document.createTextNode(text))
        range.collapse(false)
      } else {
        root.appendChild(document.createTextNode(text))
      }
      // Deliberately does NOT set `skipNextSyncRef` (unlike a plain keystroke's own `onInput`
      // below) — a real, confirmed-live bug this fixes: the text just inserted above is a plain
      // text node, not yet a chip. Skipping the sync here (as an earlier version of this function
      // did) left `{crc}`/`({crc})` etc. sitting in the DOM as bare, unstyled text forever — the
      // `template`-driven `renderSegments` re-render below is exactly what turns that plain text
      // into a real chip, so this insertion path needs it to actually run, not skip it.
      onChange(extractTemplateFromDom())
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div
      ref={editorRef}
      contentEditable
      suppressContentEditableWarning
      role="textbox"
      aria-multiline="false"
      className="stdinput"
      style={{
        width: '100%',
        boxSizing: 'border-box',
        background: '#e8e8e8',
        color: '#000',
        // Chips now use a neutral grey border specifically so they don't get mistaken for the
        // text caret sitting next to them (see that change's own docs) — giving the caret itself
        // a real, distinct color (the theme's own accent, falling back to `palette.text` on the 2
        // themes where `border` is `transparent`) instead of the browser default (usually
        // near-black, easy to conflate with dark chip borders/text) reinforces that same
        // distinction from the other direction.
        caretColor: palette.border === 'transparent' ? palette.text : palette.border,
        minHeight: '1.6em',
        outline: 'none',
        wordBreak: 'break-all',
      }}
      onInput={() => {
        skipNextSyncRef.current = true
        onChange(extractTemplateFromDom())
      }}
      onKeyDown={(e) => {
        // A plain `Enter` inside a single-line field should submit the form, not insert a
        // newline `contentEditable` would otherwise happily create.
        if (e.key === 'Enter') e.preventDefault()
      }}
    />
  )
}

/** The "重命名并入库" popover — lets the user resolve a `PendingFilenameConflict` by cataloguing
 * the already-downloaded, already-staged bytes under a new filename instead of overwriting the
 * archive that owns the original one. The input holds a *template string* (starting from the
 * default `{filename}_{crc}.{ext}`, literal placeholder syntax, not yet substituted) rather than
 * an already-resolved filename — the four small buttons insert more literal `{var}` placeholders
 * at the input's current cursor position (explicit user call, mirroring how a mail-merge/snippet
 * tool's own "insert field" buttons work), and only `onConfirm` substitutes the real values, once,
 * at submit time. Styled as a single custom `PopupMenu` item (this app's existing from-scratch
 * menu primitive, `components/PopupMenu.tsx`) rather than a whole new popover component, since a
 * `PopupMenu` accepts arbitrary children — its own `<PopupMenuItem>` row abstraction just isn't
 * used here, since a form doesn't fit that component's plain-clickable-row model. */
function RenamePopover({
  anchor,
  conflict,
  itemTitle,
  itemNamespace,
  pending,
  onCancel,
  onConfirm,
}: {
  anchor: { x: number; y: number }
  conflict: PendingFilenameConflict
  itemTitle: string | null
  itemNamespace: string
  pending: boolean
  onCancel: () => void
  onConfirm: (filename: string) => void
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const { stem, ext } = splitFilenameStemAndExt(conflict.original_filename)
  const [template, setTemplate] = useState('{filename}_{crc}.{ext}')
  // 220px (the plain-`<input>` era's width) wraps the default `{filename}_{crc}.{ext}` template
  // onto two lines once each token became its own bordered/padded chip with a `×` button — real
  // screen-space cost a bare text token didn't have. 280px keeps the default template on one line
  // in the common case.
  const width = 280
  const { top, left } = anchoredPosition(
    new DOMRect(anchor.x, anchor.y, 0, 0),
    width,
    true,
  )

  const vars: Record<string, string> = {
    filename: stem,
    crc: conflict.crc32,
    title: itemTitle ?? '',
    ext,
    ...dateTemplateValues(),
    // Uppercased per explicit request — every other template variable here is either already
    // lowercase content (`filename`/`crc`/`title`) or a fixed-case file extension, but a plugin
    // namespace (e.g. `ehdl`) reads more like a stable identifier/tag when shouted, similar to how
    // `source:` tag values elsewhere in this app are left as-is rather than case-normalized.
    namespace: itemNamespace.toUpperCase(),
  }
  const resolved = substituteFilenameTemplate(template, vars)

  // `TemplateInput` registers its own real `insert(text)` function here once mounted (it needs
  // access to the live DOM selection inside its own `contentEditable`, which this parent
  // component has no direct way to reach) — the template-variable buttons below call whatever's
  // currently registered rather than manipulating `template`/cursor position themselves.
  const insertRef = useRef<(text: string) => void>(() => {})

  return (
    <>
      <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={onCancel} />
      <PopupMenu style={{ position: 'fixed', top, left, zIndex: Z_OVERLAY_CONTENT }}>
        <li style={{ listStyle: 'none', padding: '6px 10px', width }}>
          <div style={{ fontSize: FONT_SIZE_10PT, marginBottom: 4 }}>{t('New filename')}</div>
          <TemplateInput
            template={template}
            onChange={setTemplate}
            onInsert={(insert) => {
              insertRef.current = insert
            }}
          />
          {/* `.stdbtn`'s theme default carries `min-width: 150px` — fine for a normal action
              button, wildly oversized for a small "insert this token" chip, so these deliberately
              don't use that class at all (plain themed-border buttons instead). */}
          <div style={{ display: 'flex', gap: 4, marginTop: 6, flexWrap: 'wrap' }}>
            {TEMPLATE_VARS.map((key) => (
              <button
                key={key}
                type="button"
                style={{
                  fontSize: FONT_SIZE_8PT,
                  padding: '1px 5px',
                  minWidth: 0,
                  width: 'auto',
                  border: '1px solid rgba(128,128,128,0.4)',
                  borderRadius: 3,
                  background: 'transparent',
                  cursor: 'pointer',
                }}
                // Inline `style` can't express `:hover` — this app's own theme classes
                // (`.stdbtn`/`.stdinput`) get their hover state from real CSS rules, but these
                // buttons deliberately don't use `.stdbtn` (its 150px min-width is wildly oversized
                // here), so the hover affordance has to be applied by hand, on the actual events,
                // the same way `PopupMenuItem` already does for its own row-hover highlight.
                // `palette.border` is `transparent` on 2 of this app's 5 themes (`modern_red.css`/
                // `modern_clear.css`) — an outline in that color would be invisible on hover.
                // `palette.text` is a real, non-transparent color on every theme.
                onMouseEnter={(e) => {
                  e.currentTarget.style.outline = `1px solid ${palette.text}`
                  e.currentTarget.style.outlineOffset = '1px'
                }}
                onMouseLeave={(e) => {
                  e.currentTarget.style.outline = 'none'
                }}
                onClick={(e) => insertRef.current(e.shiftKey ? shiftClickInsertion(key) : `{${key}}`)}
              >
                {`{${key}}`}
              </button>
            ))}
          </div>
          {/* Explains the shift-click insertion mode right below the buttons that trigger it —
              easy to miss otherwise, since holding shift changes what a plain click inserts with
              no other visual cue on the buttons themselves. Two lines, not one: `{ext}` is a real
              exception to the general "wrapped in parentheses" rule (see `shiftClickInsertion`'s
              own docs for why), so the general explanation on its own would be actively wrong
              about that one button. */}
          <div style={{ fontSize: FONT_SIZE_8PT, opacity: 0.6, marginTop: 4 }}>
            <div>{t('Shift-click to insert wrapped in parentheses')}</div>
            <div>{t('Shift-click {{ext}} to insert with a leading dot instead', { ext: '{ext}' })}</div>
          </div>
          {/* A `<code>` block (not a plain `<div>`) — this text is a literal, exact filename the
              user is about to commit to, not prose, so it reads better set in a monospace/code
              typeface with its own subtle background, same convention as an inline code snippet
              elsewhere in a document. */}
          <code
            style={{
              display: 'block',
              fontSize: FONT_SIZE_8PT,
              marginTop: 6,
              padding: '3px 5px',
              background: 'rgba(0,0,0,0.06)',
              borderRadius: 3,
              wordBreak: 'break-all',
              minHeight: '1.2em',
            }}
          >
            {resolved}
          </code>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 6, marginTop: 8 }}>
            <button
              type="button"
              className="stdbtn"
              style={{ minWidth: 0, width: 'auto', padding: '2px 8px' }}
              onClick={onCancel}
            >
              {t('Cancel')}
            </button>
            <button
              type="button"
              className="stdbtn"
              style={{ minWidth: 0, width: 'auto', padding: '2px 8px' }}
              disabled={pending || !resolved.trim()}
              onClick={() => onConfirm(resolved)}
            >
              {t('Rename and Catalog')}
            </button>
          </div>
        </li>
      </PopupMenu>
    </>
  )
}

/** Wraps `children` in a `Tooltip` only when `preview` is actually present — extracted so the
 * `Tooltip` itself (and therefore its anchor measurement) wraps the *bordered* container, not
 * just the inner text span, so the bubble's left edge lines up with the border the user actually
 * sees, not with the unbordered text a few pixels further in. */
function TooltipIfPresent({
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
    // No `Tooltip` wrapper needed, but `wrapperStyle` (the flex sizing the caller's layout
    // depends on) still has to land somewhere, or this branch's element shrinks to its content
    // width instead of matching the tooltip-present branch's sizing — see `Tooltip`'s own
    // `wrapperStyle` docs for why a real inline-flex `<span>` (not just spreading the style onto
    // `children` directly, which isn't an option when `children` is arbitrary/opaque JSX) is what
    // the tooltip-present branch actually renders under the hood.
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
