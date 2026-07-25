import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import CollapsibleSection from '../../components/CollapsibleSection'
import { STATE_COLOR } from '../../components/JobProgress'
import { routes } from '../../routes'
import { FONT_SIZE_8PT, FONT_SIZE_10PT } from '../../theme'

export interface UploadRow {
  key: string
  name: string
  state: 'processing' | 'done' | 'error'
  archiveId?: string
  title?: string
  message?: string
}

/** Local-upload results, styled to match `DownloadQueuePanel`'s own grouped-row look (bordered
 * box per row, same `STATE_COLOR.active` green completed bar, same error-text styling) rather
 * than the plain two-column table this used to be — local uploads have no queue/plugin grouping
 * of their own, so this renders as a single `CollapsibleSection` instead of one per namespace. */
export default function LocalUploadPanel({ rows }: { rows: UploadRow[] }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  if (rows.length === 0) return null

  return (
    <div style={{ marginBottom: 16, textAlign: 'left' }}>
      <ul className="collapsible extensible with-right-caret" style={{ width: '100%' }}>
        <CollapsibleSection icon="fa-upload" title={`${t('From your computer')} (${rows.length})`} caretStyle="right-down" defaultOpen>
          {rows.map((row) => (
            <div
              key={row.key}
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 4,
                padding: '4px 2px',
                borderTop: '1px solid rgba(128,128,128,0.2)',
              }}
            >
              <div style={{ flex: '1 1 180px', minWidth: 0 }}>
                <div
                  style={{
                    width: '100%',
                    boxSizing: 'border-box',
                    border: '1px solid rgba(128,128,128,0.3)',
                    borderRadius: 4,
                    padding: '2px 6px',
                  }}
                >
                  {row.state === 'processing' && (
                    <span style={{ fontSize: FONT_SIZE_10PT, wordBreak: 'break-all' }} title={row.name}>
                      <i className="fas fa-spinner fa-spin"></i> {row.name}
                    </span>
                  )}
                  {row.state === 'done' && (
                    <div style={{ position: 'relative', height: 18, borderRadius: 4, overflow: 'hidden', background: STATE_COLOR.active }}>
                      <a
                        href={row.archiveId ? routes.reader(row.archiveId) : undefined}
                        onClick={(e) => {
                          if (!row.archiveId) return
                          e.preventDefault()
                          navigate(routes.reader(row.archiveId))
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
                          cursor: row.archiveId ? 'pointer' : 'default',
                        }}
                      >
                        {row.title ?? row.name}
                      </a>
                    </div>
                  )}
                  {row.state === 'error' && (
                    <span style={{ fontSize: FONT_SIZE_10PT, wordBreak: 'break-all' }} title={row.name}>
                      {row.name}
                    </span>
                  )}
                  {row.state === 'error' && row.message && (
                    <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>
                      <i className="fa fa-exclamation-circle"></i> {row.message}
                    </div>
                  )}
                </div>
              </div>
              {row.state === 'done' && row.archiveId && (
                <a href={routes.edit(row.archiveId)} target="_blank" rel="noreferrer" title={t('Click here to edit metadata.') ?? undefined}>
                  <i className="fa fa-pen" style={{ width: 18 }}></i>
                </a>
              )}
            </div>
          ))}
        </CollapsibleSection>
      </ul>
    </div>
  )
}
