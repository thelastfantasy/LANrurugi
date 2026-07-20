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
  useDownloadQueue,
  useJobs,
  usePlugins,
  useStartAllQueue,
  useStartQueueItem,
  useStartSelectedQueue,
  useUpdateQueueItem,
} from '../api/hooks'
import type { ArchiveMetadata, DownloadQueueItem, JobRecord, PluginInfo } from '../api/types'
import CollapsibleSection from '../components/CollapsibleSection'
import { JobProgressBar, STATE_COLOR } from '../components/JobProgress'
import Tooltip from '../components/Tooltip'
import { FONT_SIZE_8PT, FONT_SIZE_10PT, useApplyTheme } from '../theme'
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

/** A bare `namespace:value` tag's value is a real absolute URL missing only its `https://`
 * scheme (real corpus evidence: E-Hentai's `source:` tag — `hashdata["tags"] += ", source:" +
 * (domain.split('://'))[1] + ...` in `plugins/metadata/ehentai.ts` — strips the scheme off
 * before appending). Adding it back is what turns `source` from inert text into a clickable
 * link without the tooltip needing to special-case *which* namespace means "this is a URL". */
function withScheme(value: string): string {
  return /^https?:\/\//i.test(value) ? value : `https://${value}`
}

/** Renders a metadata plugin's `{tags?, title?, summary?}` response as a short multi-line
 * tooltip body — deliberately schema-agnostic (see `DownloadQueueItem.metadata_preview`'s own
 * docs): `tags` is split on commas into `namespace: value` rows using whatever namespaces this
 * particular plugin happened to return (E-Hentai's `artist:`/`uploader:`/`category:`/
 * `timestamp:`/`source:`, or a completely different set from another site's plugin), not a fixed
 * allowlist. An untagged/non-namespaced entry (no `:`) renders as a bare value under a generic
 * "Tags" heading instead of being dropped.
 *
 * No separate "raw URL" line: a `source:` tag (when present) already carries the same URL — see
 * `withScheme`'s own docs — so showing both was pure duplication. `source` is instead rendered as
 * a real hyperlink (opened in a new tab) rather than plain text; every other namespace keeps its
 * original plain-text rendering. */
function MetadataPreviewTooltip({ preview, url }: { preview: Record<string, unknown>; url: string }) {
  const { t } = useTranslation()
  const tags = typeof preview.tags === 'string' ? preview.tags : ''
  const summary = typeof preview.summary === 'string' ? preview.summary : undefined

  const grouped = new Map<string, string[]>()
  const plain: string[] = []
  for (const raw of tags.split(',')) {
    const entry = raw.trim()
    if (!entry) continue
    const colonIndex = entry.indexOf(':')
    if (colonIndex === -1) {
      plain.push(entry)
      continue
    }
    const namespace = entry.slice(0, colonIndex).trim()
    const value = entry.slice(colonIndex + 1).trim()
    const list = grouped.get(namespace) ?? []
    list.push(value)
    grouped.set(namespace, list)
  }
  const hasSourceTag = grouped.has('source')

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
      <div style={{ fontWeight: 'bold', wordBreak: 'break-word' }}>
        {typeof preview.title === 'string' ? preview.title : url}
      </div>
      {!hasSourceTag && <div style={{ wordBreak: 'break-all', opacity: 0.8, fontSize: FONT_SIZE_10PT }}>{url}</div>}
      {[...grouped.entries()].map(([namespace, values]) => (
        <div key={namespace} style={{ wordBreak: 'break-all' }}>
          <strong>{namespace}:</strong>{' '}
          {namespace === 'source'
            ? values.map((value, i) => (
                <span key={value}>
                  {i > 0 && ', '}
                  <a href={withScheme(value)} target="_blank" rel="noreferrer">
                    {value}
                  </a>
                </span>
              ))
            : values.join(', ')}
        </div>
      ))}
      {plain.length > 0 && (
        <div>
          <strong>{t('Tags')}:</strong> {plain.join(', ')}
        </div>
      )}
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
                        <a href={`/reader/${row.archiveId}`} title={row.title ?? row.name}>
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
                          <a href={`/edit/${row.archiveId}`} target="_blank" rel="noreferrer">
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
      <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />
    </div>
  )
}

/** Right-column panel: the persistent queue, grouped by `plugin_namespace` (one
 * `CollapsibleSection` per namespace present), with bulk Start All / Start Selected / Clear
 * Completed actions and per-item controls. */
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
  const startAll = useStartAllQueue()
  const startSelected = useStartSelectedQueue()
  const clearCompleted = useClearCompletedQueue()
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
      void sendJson('POST', `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&id=${encodeURIComponent(archiveId)}`)
    }
  }, [items, jobById, metadataPlugins])

  if (items.length === 0) return null

  const selectableIds = items.filter((i) => i.state === 'queued').map((i) => i.id)
  const cleared = items.filter((i) => i.state === 'done' || i.state === 'error').length

  return (
    <div style={{ marginTop: 16, textAlign: 'left' }}>
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Download Queue')}
      </h2>

      <div className="control-btn-group" style={{ justifyContent: 'center', gap: 6, marginBottom: 6 }}>
        <button
          type="button"
          className="stdbtn"
          disabled={selectableIds.length === 0 || startAll.isPending}
          onClick={() => void startAll.mutateAsync()}
        >
          {t('Start All')}
        </button>
        <button
          type="button"
          className="stdbtn"
          disabled={selected.size === 0 || startSelected.isPending}
          onClick={async () => {
            await startSelected.mutateAsync([...selected])
            setSelected(new Set())
          }}
        >
          {t('Start Selected ({{n}})', { n: selected.size })}
        </button>
        <button
          type="button"
          className="stdbtn"
          disabled={cleared === 0 || clearCompleted.isPending}
          onClick={() => void clearCompleted.mutateAsync()}
        >
          {t('Clear Completed')}
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
                    if (item.state !== 'queued') return
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
  const update = useUpdateQueueItem()
  const start = useStartQueueItem()
  const del = useDeleteQueueItem()
  const [fetchingMetadata, setFetchingMetadata] = useState(false)

  async function handleFetchMetadata() {
    if (!metadataPlugin) return
    setFetchingMetadata(true)
    try {
      const result = await sendJson<{ success: number; data?: Record<string, unknown> }>(
        'POST',
        `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&arg=${encodeURIComponent(item.url)}`,
      ).catch(() => null)
      const data = result?.data
      const title = typeof data?.title === 'string' ? data.title : undefined
      if (data) await update.mutateAsync({ id: item.id, title, metadata_preview: data })
    } finally {
      setFetchingMetadata(false)
    }
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
      <input type="checkbox" checked={selected} disabled={item.state !== 'queued'} onChange={onToggleSelect} />

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
            job ? (
              <JobProgressBar job={job} />
            ) : (
              <span style={{ fontSize: FONT_SIZE_10PT }}>{t('Starting…')}</span>
            )
          ) : (
            <span style={{ fontSize: FONT_SIZE_10PT, wordBreak: 'break-all' }} title={item.metadata_preview ? undefined : item.url}>
              {item.title ?? item.url}
            </span>
          )}
          {item.state === 'error' && item.error && (
            <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>{item.error}</div>
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

      <Tooltip label={t('Download') ?? ''}>
        <button
          type="button"
          className="stdbtn"
          style={ICON_BUTTON_STYLE}
          disabled={item.state !== 'queued' || start.isPending}
          onClick={() => void start.mutateAsync(item.id)}
        >
          <i className="fa fa-download" aria-hidden="true"></i>
        </button>
      </Tooltip>

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
          disabled={!metadataPlugin || item.state !== 'queued' || fetchingMetadata}
          onClick={() => void handleFetchMetadata()}
        >
          <i className={`fa ${fetchingMetadata ? 'fa-spinner fa-spin' : 'fa-tags'}`} aria-hidden="true"></i>
        </button>
      </Tooltip>

      <Tooltip label={t('Delete') ?? ''}>
        <button
          type="button"
          className="stdbtn"
          style={ICON_BUTTON_STYLE}
          disabled={item.state === 'downloading' || item.state === 'starting' || del.isPending}
          onClick={() => void del.mutateAsync(item.id)}
        >
          <i className="fa fa-times" aria-hidden="true"></i>
        </button>
      </Tooltip>
    </div>
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
