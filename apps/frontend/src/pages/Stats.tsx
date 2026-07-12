import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { useArchives, useServerInfo, useStats } from '../api/hooks'
import CollapsibleSection from '../components/CollapsibleSection'
import { useApplyTheme } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'

// Mirrors legacy's `~/LANraragi/templates/stats.html.tt2` — `#stats` icon+counter lines, then a
// `.collapsible.extensible.with-right-caret` > `.option-flyout` flyout for the detailed tag list
// (same accordion classes as the nav/carousel). Doesn't reproduce jqCloud's tag-cloud rendering.
export default function Stats() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const stats = useStats(2)
  const archives = useArchives()
  const info = useServerInfo()
  useApplyTheme()
  useDocumentTitle(t('Library Statistics') ?? undefined)

  const sorted = [...(stats.data ?? [])].sort((a, b) => b.weight - a.weight)

  const archiveCount = archives.data?.length ?? 0
  const contentSizeGb = (archives.data ?? []).reduce((sum, a) => sum + a.size, 0) / 1e9
  const tagCount = sorted.length
  const pagesRead = info.data?.total_pages_read ?? 0

  return (
    <div className="ido">
      <h1 className="ih">{t('Library Statistics')}</h1>

      <div id="stats">
        <p>
          <i className="fa fa-book fa-2x"></i> <span style={{ fontSize: 20 }}>{archiveCount}</span>{' '}
          {t('Archives on record')}
        </p>
        <p>
          <i className="fa fa-tags fa-2x"></i> <span style={{ fontSize: 20 }}>{tagCount}</span>{' '}
          {t('Different tags existing')}
        </p>
        <p>
          <i className="fa fa-folder-open fa-2x"></i>{' '}
          <span style={{ fontSize: 20 }}>{contentSizeGb.toFixed(2)} GB</span> {t('in content folder')}
        </p>
        <p>
          <i className="fa fa-book-reader fa-2x"></i> <span style={{ fontSize: 20 }}>{pagesRead}</span>{' '}
          {t('pages read')}
        </p>
      </div>

      {stats.isLoading && (
        <div id="statsLoading" style={{ width: '80%', marginLeft: 'auto', marginRight: 'auto' }}>
          <i className="fa fa-dharmachakra fa-4x fa-spin"></i>
        </div>
      )}

      <ul
        className="collapsible extensible with-right-caret"
        id="detailedStats"
        style={{ width: '80%', marginLeft: 'auto', marginRight: 'auto' }}
      >
        <CollapsibleSection icon="fa-chart-bar" title={t('Tag Cloud')}>
          <div id="tagList">
            {sorted.map((tag) => (
              <span key={`${tag.namespace ?? ''}:${tag.text}`} style={{ marginRight: 8 }}>
                {tag.namespace ? `${tag.namespace}:` : ''}
                {tag.text} ({tag.weight})
              </span>
            ))}
            {sorted.length === 0 && !stats.isLoading && <span>{t('No tags found.')}</span>}
          </div>
        </CollapsibleSection>
      </ul>

      <p style={{ fontSize: '9pt' }}>
        {t('(These statistics only show tags that appear at least twice in your database.)')}
      </p>

      <input type="button" id="goback" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />
    </div>
  )
}
