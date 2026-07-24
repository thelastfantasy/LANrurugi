import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { waitForJob } from '../../api/client'
import { routes } from '../../routes'
import { FONT_SIZE_10PT, useApplyTheme } from '../../theme'
import { useDocumentTitle } from '../../useDocumentTitle'

// Mirrors legacy's `~/LANraragi/templates/backup.html.tt2` — one `table > tbody#files > tr` with
// two `.stdbtn.fileinput-button` spans (backup/restore), a processing spinner, and a return
// button. Doesn't reproduce the upload-plugin progress bar (`backup.js`) — status is a plain
// text line instead.
export default function Backup() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [status, setStatus] = useState<string>('')
  const [busy, setBusy] = useState(false)

  async function handleBackup() {
    setBusy(true)
    setStatus(t('Backup generation in progress...'))
    try {
      const response = await fetch('/api/database/backup', { method: 'POST' })
      const { job } = (await response.json()) as { job: string }
      const jobStatus = await waitForJob(job)

      if (jobStatus.state === 'failed') {
        setStatus(t('Backup failed: {{error}}', { error: jobStatus.error ?? t('unknown error') }))
        return
      }

      const blob = new Blob([JSON.stringify(jobStatus.notes, null, 2)], {
        type: 'application/json',
      })
      const url = URL.createObjectURL(blob)
      const a = document.createElement('a')
      a.href = url
      a.download = `lanrurugi-backup-${new Date().toISOString()}.json`
      a.click()
      URL.revokeObjectURL(url)
      setStatus(t('Backup complete! Download will start automatically.'))
    } catch (e) {
      setStatus(t('Backup failed: {{error}}', { error: String(e) }))
    } finally {
      setBusy(false)
    }
  }

  async function handleRestore(file: File) {
    setBusy(true)
    setStatus(t('Uploading file...'))
    try {
      const formData = new FormData()
      formData.append('file', file)
      const response = await fetch('/api/database/restore', {
        method: 'POST',
        body: formData,
      })
      const { job } = (await response.json()) as { job: string }
      const jobStatus = await waitForJob(job)

      if (jobStatus.state === 'failed') {
        setStatus(
          t('Restore failed: {{error}}', { error: jobStatus.error ?? t('unknown error') }),
        )
      } else {
        setStatus(t('Backup restored!'))
      }
    } catch (e) {
      setStatus(t('Restore failed: {{error}}', { error: String(e) }))
    } finally {
      setBusy(false)
      if (fileInputRef.current) fileInputRef.current.value = ''
    }
  }

  useApplyTheme()
  useDocumentTitle(t('Database Backup/Restore') ?? undefined)

  return (
    <div className="ido" style={{ textAlign: 'center' }}>
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Database Backup/Restore')}
      </h2>

      <br />
      {t('You can backup your existing database here, or restore an existing backup.')}
      <br />
      <br />
      {t('Backuping allows you to download a JSON file containing all your categories and archive IDs, and their matching metadata.')}
      <br />
      <span
        dangerouslySetInnerHTML={{
          __html: t(
            'Restoring from a backup will restore this metadata, <b>for IDs which already exist in your database.</b>',
          ),
        }}
      />
      <br />
      {t('(Categories will always be restored)')}

      <table style={{ margin: 'auto', fontSize: FONT_SIZE_10PT, marginTop: 25, textAlign: 'center' }}>
        <tbody id="files">
          <tr>
            <td>
              <span
                id="do-backup"
                className="stdbtn"
                style={{ height: 50, display: 'inline-block' }}
                onClick={() => !busy && void handleBackup()}
              >
                <i className="fa fa-download fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
                <br />
                <span>{t('Backup Database')}</span>
              </span>
            </td>
            <td>
              <span className="stdbtn fileinput-button" style={{ height: 50, display: 'inline-block' }}>
                <i className="fa fa-upload fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
                <br />
                <span>{t('Restore Backup')}</span>
                <input
                  ref={fileInputRef}
                  type="file"
                  name="file"
                  multiple
                  disabled={busy}
                  onChange={(e) => {
                    const file = e.target.files?.[0]
                    if (file) void handleRestore(file)
                  }}
                />
              </span>
            </td>
          </tr>
        </tbody>
      </table>
      <br />
      <br />

      <span style={{ margin: 'auto', fontSize: FONT_SIZE_10PT, width: '80%', textAlign: 'center' }}>
        {busy && (
          <div id="processing">
            <i className="fa fa-3x fa-compact-disc fa-spin" style={{ marginTop: 20 }}></i>
            <h3 id="processing-status">{t('Processing')}</h3>
          </div>
        )}

        <h3 id="result">{status}</h3>
      </span>

      <br />
      <br />
      <br />
      <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}
