import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { waitForJob } from '../api/client'
import { useClearDuplicates, useDeleteArchive, useDuplicates, useScanDuplicates } from '../api/hooks'
import { useApplyTheme } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'

// Mirrors legacy's `~/LANraragi/templates/duplicates.html.tt2` — empty/start state with a
// `.stdbtn.find-duplicates` button, results state as a `table#ds.ds.itg` (the same "index table
// grid" class the Library page's table view would use). Doesn't reproduce DataTables
// sorting/paging or Tippy.js tag tooltips (both separate JS features) — this is a plain static
// table, still fully functional for scan/clear/delete.
export default function Duplicates() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const duplicates = useDuplicates()
  const scanDuplicates = useScanDuplicates()
  const clearDuplicates = useClearDuplicates()
  const deleteArchive = useDeleteArchive()
  useApplyTheme()
  useDocumentTitle(t('Duplicate Detection') ?? undefined)

  const [threshold, setThreshold] = useState(5)
  const [scanning, setScanning] = useState(false)

  async function handleScan() {
    setScanning(true)
    try {
      const { job } = await scanDuplicates.mutateAsync(threshold)
      await waitForJob(job)
      await duplicates.refetch()
    } finally {
      setScanning(false)
    }
  }

  async function handleDelete(arcid: string) {
    await deleteArchive.mutateAsync(arcid)
    await duplicates.refetch()
  }

  const hasResults = duplicates.data && duplicates.data.length > 0

  return (
    <div className="ido">
      <h1 className="ih">{t('Duplicate Detection')}</h1>
      <p>
        {t(
          'This feature looks at archives across your database to try and find duplicates by comparing cover thumbnail hashes.',
        )}
      </p>

      {duplicates.isLoading && (
        <div id="processing">
          <i className="fa fa-3x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {!hasResults && !duplicates.isLoading && (
        <div id="nodupes">
          <i className="fa fa-3x fa-check-circle"></i>
          <p>{t('No duplicates found!')}</p>
        </div>
      )}

      <div className="control-btn-group">
        <label>
          {t('Threshold:')}
          <input
            type="number"
            min={0}
            max={40}
            value={threshold}
            onChange={(e) => setThreshold(Number(e.target.value))}
            className="stdinput"
            style={{ width: 60 }}
          />
        </label>
        <button type="button" className="stdbtn find-duplicates" disabled={scanning} onClick={() => void handleScan()}>
          {scanning ? t('Rescanning...') : t('Search for duplicates')}
        </button>
        <button type="button" className="stdbtn clear-duplicates" onClick={() => clearDuplicates.mutate()}>
          {t('Clear Results')}
        </button>
      </div>

      {hasResults && (
        <table id="ds" className="ds itg">
          <thead>
            <tr>
              <th>{t('Title')}</th>
              <th>{t('Filesize')}</th>
              <th>{t('Action')}</th>
            </tr>
          </thead>
          <tbody>
            {(duplicates.data ?? []).map((group, index) =>
              group.map((archive, i) => (
                <tr key={archive.arcid} className={i === 0 ? 'duplicate-group' : undefined}>
                  <td>
                    <div className="thumbnail-wrapper" style={{ display: 'inline-block', marginRight: 8 }}>
                      <a href={`#/edit/${archive.arcid}`} onClick={(e) => { e.preventDefault(); navigate(`/edit/${archive.arcid}`) }}>
                        <img
                          src={`/api/archives/${archive.arcid}/thumbnail`}
                          alt={archive.title}
                          style={{ height: 60 }}
                        />
                      </a>
                    </div>
                    <a href={`#/edit/${archive.arcid}`} onClick={(e) => { e.preventDefault(); navigate(`/edit/${archive.arcid}`) }}>
                      {archive.title}
                    </a>
                  </td>
                  <td>{(archive.size / 1e6).toFixed(1)} MB</td>
                  <td>
                    <button type="button" className="stdbtn delete-archive action-button" onClick={() => void handleDelete(archive.arcid)}>
                      {t('Delete')}
                    </button>
                  </td>
                </tr>
              )).concat(
                index < (duplicates.data?.length ?? 0) - 1
                  ? [
                      <tr key={`sep-${group[0]?.group_key ?? index}`}>
                        <td colSpan={3}>&nbsp;</td>
                      </tr>,
                    ]
                  : [],
              ),
            )}
          </tbody>
        </table>
      )}

      <input type="button" id="goback" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />
    </div>
  )
}
