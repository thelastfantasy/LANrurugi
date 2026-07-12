import { useQueryClient } from '@tanstack/react-query'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { fetchJson } from '../api/client'
import { useCategories } from '../api/hooks'
import type { ArchiveMetadata } from '../api/types'
import { useApplyTheme } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'

interface UploadRow {
  key: string
  name: string
  // 'processing' while polling a download_url job; upload is synchronous so it goes straight to
  // 'done'/'error'.
  state: 'processing' | 'done' | 'error'
  archiveId?: string
  title?: string
  message?: string
  // Legacy's `I18N.UploadProcessing`/`DownloadProcessing` interpolate a real Minion job id — our
  // upload endpoint is fully synchronous (no job id exists yet when the row is created), so this
  // holds whichever identifier is actually available at that point (the download_url job id, or
  // just the filename for a synchronous upload) rather than fabricating a fake job number.
  jobLabel: string
}

// Mirrors legacy's `~/LANraragi/templates/upload.html.tt2` + `public/js/upload.js`: centered
// `.ido`, category select, a "From your computer" section (icon-over-label
// `.stdbtn.fileinput-button`, stacked not inline) and a "From the Internet" section (URL textarea
// + its own download button, wired to the real `POST /download_url` endpoint) in `.left-column`.
// `.right-column` is a per-file `#files` table (legacy's `#${job}-name`/`#${job}-icon`/
// `#${job}-link` row-mutation pattern) rather than one shared status line — each upload/download
// gets its own row that starts as a spinner + "Processing your upload...", then flips in place to
// either the archive's title linking to `/reader/{id}` with a "Click here to edit metadata." link
// to `/edit/{id}`, or a red error icon + message. Legacy's own upload page has no title/tags
// fields at all — those only exist on the Edit page, added after upload — so this doesn't invent
// any either.
export default function Upload() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const categories = useCategories()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [category, setCategory] = useState('')
  const [rows, setRows] = useState<UploadRow[]>([])
  const [urls, setUrls] = useState('')
  const [downloading, setDownloading] = useState(false)
  const [uploadingCount, setUploadingCount] = useState(0)
  useApplyTheme()
  useDocumentTitle(t('Upload Center') ?? undefined)

  function upsertRow(key: string, patch: Partial<UploadRow>) {
    setRows((prev) => {
      const idx = prev.findIndex((r) => r.key === key)
      if (idx === -1) return [...prev, { key, name: key, state: 'processing', jobLabel: key, ...patch }]
      const next = [...prev]
      next[idx] = { ...next[idx], ...patch }
      return next
    })
  }

  async function handleUpload(toUpload: File) {
    const key = `upload-${Date.now()}-${toUpload.name}`
    upsertRow(key, { name: toUpload.name, state: 'processing', jobLabel: toUpload.name })
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

  async function pollDownloadJob(key: string, jobId: string) {
    // Mirrors legacy's `checkJobStatus` polling loop (`~/LANraragi/public/js/mod/server.js`) —
    // repeatedly hits the job-detail endpoint until it leaves the active/inactive states.
    for (;;) {
      await new Promise((resolve) => setTimeout(resolve, 1500))
      const detail = await fetchJson<{
        state: string
        result: { id?: string; title?: string; success?: number; message?: string } | null
      }>(`/minion/${jobId}/detail`).catch(() => null)
      if (!detail) continue
      if (detail.state === 'finished') {
        const result = detail.result
        if (result?.id) {
          const meta = await fetchJson<ArchiveMetadata>(`/archives/${result.id}/metadata`).catch(() => null)
          upsertRow(key, { state: 'done', archiveId: result.id, title: meta?.title ?? result.title })
          await queryClient.invalidateQueries({ queryKey: ['archives'] })
        } else {
          upsertRow(key, { state: 'error', message: result?.message ?? t('Error while downloading file.') ?? '' })
        }
        return
      }
      if (detail.state === 'failed') {
        upsertRow(key, { state: 'error', message: t('Error while downloading file.') ?? '' })
        return
      }
    }
  }

  async function handleDownloadUrls() {
    const list = urls
      .split('\n')
      .map((u) => u.trim())
      .filter(Boolean)
    if (list.length === 0) return
    setDownloading(true)
    try {
      for (const url of list) {
        const key = `download-${Date.now()}-${url}`
        upsertRow(key, { name: url, state: 'processing', jobLabel: url })
        const params = new URLSearchParams({ url })
        if (category) params.set('catid', category)
        try {
          const response = await fetch(`/api/download_url?${params}`, { method: 'POST' })
          const data = (await response.json()) as { success: number; job?: string; error?: string }
          if (data.success && data.job) {
            upsertRow(key, { jobLabel: data.job })
            void pollDownloadJob(key, data.job)
          } else {
            upsertRow(key, { state: 'error', message: data.error ?? t('Error while downloading file.') ?? '' })
          }
        } catch (e) {
          upsertRow(key, { state: 'error', message: String(e) })
        }
      }
      setUrls('')
    } finally {
      setDownloading(false)
    }
  }

  return (
    <div className="ido" style={{ textAlign: 'center', fontSize: '8pt' }}>
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

          <span className="stdbtn fileinput-button" style={{ height: 50 }}>
            <i className="fas fa-download fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
            <br />
            <span>{t('Add from your computer')}</span>
            <input
              ref={fileInputRef}
              type="file"
              name="file"
              accept=".zip,.cbz,.rar,.cbr,.7z,.cb7,.pdf,.epub"
              disabled={uploadingCount > 0}
              // Auto-uploads on selection, matching legacy's own jQuery File Upload behavior
              // (there's no separate "confirm" step — choosing a file starts the upload).
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

          {t('Type in your URLs (separated by a newline), and click the download button.')}
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
            id="download-url"
            className="stdbtn fileinput-button"
            style={{ height: 50 }}
            onClick={() => !downloading && urls.trim() && void handleDownloadUrls()}
          >
            <i className="fas fa-cloud-download-alt fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
            <br />
            <span>{t('Add from URL(s)')}</span>
          </span>
        </div>

        <div className="right-column">
          <h2 className="ih">
            {rows.length > 0 &&
              `🤔 ${t('Processing')}: ${rows.filter((r) => r.state === 'processing').length} 🙌 ${t('Completed')}: ${
                rows.filter((r) => r.state === 'done').length
              } 👹 ${t('Failed')}: ${rows.filter((r) => r.state === 'error').length}`}
          </h2>
          <table style={{ margin: 'auto', fontSize: '9pt', width: '80%', textAlign: 'center' }}>
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
                          <i className="fas fa-spinner fa-spin"></i>{' '}
                          {(row.key.startsWith('download-')
                            ? t('Downloading file... (Job #\\${jobid})')
                            : t('Processing your upload... (Job #\\${jobid})')
                          )?.replace('${jobid}', row.jobLabel)}
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
        </div>
      </div>

      <br />
      <br />
      <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />
    </div>
  )
}
