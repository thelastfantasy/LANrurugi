import { useQueryClient } from '@tanstack/react-query'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { fetchJson } from '../../api/client'
import { useAddToQueue, useCategories, usePlugins } from '../../api/hooks'
import type { ArchiveMetadata, PluginInfo } from '../../api/types'
import { STATE_COLOR } from '../../components/JobProgress'
import { routes } from '../../routes'
import { FONT_SIZE_8PT, useApplyTheme } from '../../theme'
import { useDocumentTitle } from '../../useDocumentTitle'
import DownloadQueuePanel from './DownloadQueuePanel'
import LocalUploadPanel, { type UploadRow } from './LocalUploadPanel'
import { findMatchingPlugin } from './shared'

// "Add from URL" stages matched URLs into a persistent, server-side queue (`useDownloadQueue`),
// grouped by which download plugin's `url_pattern` matched, so the queue survives a page refresh
// or a different browser tab. Manual file upload (left column) is unchanged — synchronous, no
// queue step.
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

  // Resolving each URL's checkbox defaults needs a per-plugin settings/options fetch, which can't
  // happen inside a plain callback (hooks can't be called conditionally/in a loop) — so defaults
  // are resolved via direct one-off fetches instead of hooks.
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
          <LocalUploadPanel rows={rows} />

          <DownloadQueuePanel downloadPlugins={downloadPlugins.data} metadataPlugins={metadataPlugins.data} />
        </div>
      </div>

      <br />
      <br />
      <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}
