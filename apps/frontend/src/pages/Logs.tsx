import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { LOG_CATEGORIES, type LogCategory, useLogLines } from '../api/hooks'
import { useApplyTheme } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'

const CATEGORY_LABELS: Record<LogCategory, string> = {
  general: 'General',
  shinobu: 'Shinobu',
  plugins: 'Plugins',
  redis: 'Redis',
  mojo: 'Web Server',
}

// Matches legacy's own button copy exactly (`logs.html.tt2`'s `#show-*` buttons), distinct from
// `CATEGORY_LABELS` above (which is just the bare category name, used for the "Currently Viewing:"
// indicator).
const CATEGORY_BUTTON_LABELS: Record<LogCategory, string> = {
  general: 'View LANrurugi Logs',
  shinobu: 'View Shinobu Logs',
  plugins: 'View Plugin Logs',
  redis: 'View Redis Logs',
  mojo: 'View Mojolicious Logs',
}

// Mirrors legacy's `~/LANraragi/templates/logs.html.tt2` — the intro paragraph + per-category
// bullet list, `.ih` heading with a floated refresh icon and a "Lines:" count input, and the log
// body itself is a one-row `table.itg` (`tr.gtr1 > td > pre.log-panel`), not a styled card.
// Doesn't reproduce the live-tailing/auto-refresh behavior (`logs.js`'s polling) — this refetches
// on category/line-count change and via the refresh icon only.
export default function Logs() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [category, setCategory] = useState<LogCategory>('general')
  const [lines, setLines] = useState(100)
  const logLines = useLogLines(category, lines)
  useApplyTheme()
  useDocumentTitle(t('Logs') ?? undefined)

  return (
    <div className="ido" style={{ textAlign: 'center' }}>
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Application Logs')}
      </h2>

      <br />
      {t('You can check LANrurugi logs here for debugging purposes.')}
      <br />
      {t('By default, this view only shows the last 100 lines of each logfile, newest lines last.')}
      <br />
      <br />
      <ul>
        <li>{t('General Logs pertain to the main application.')}</li>
        <li>{t('Shinobu Logs correspond to the Background Worker.')}</li>
        <li>{t('Plugin Logs are reserved for metadata plugins only.')}</li>
        <li>{t("Mojolicious logs won't tell much unless you're running Debug Mode.")}</li>
        <li>{t("Redis logs won't be available from here if you're running from source!")}</li>
      </ul>
      <br />
      <br />

      <h1 className="ih" style={{ float: 'left', marginLeft: '5%' }}>
        {t('Currently Viewing:')} <span id="indicator">{t(CATEGORY_LABELS[category])}</span>
      </h1>
      <div style={{ marginRight: '5%', float: 'right' }}>
        <a
          href="#"
          title="Refresh"
          onClick={(e) => {
            e.preventDefault()
            void logLines.refetch()
          }}
        >
          <i style={{ paddingRight: 10 }} className="fa fa-sync-alt fa-2x"></i>
        </a>
        {t('Lines:')}{' '}
        <input
          type="number"
          min={0}
          value={lines}
          onChange={(e) => setLines(Math.max(0, Number(e.target.value) || 0))}
          style={{ width: 60 }}
        />
      </div>
      <div style={{ clear: 'both' }} />

      <table className="itg" style={{ width: '100%', marginTop: 32 }}>
        <tbody>
          <tr className="gtr1">
            <td>
              <pre id="log-container" className="log-panel">
                {logLines.isLoading ? t('Loading library…') : logLines.data || t('No logs to be found here!')}
              </pre>
            </td>
          </tr>
        </tbody>
      </table>

      <span id="buttonstagging">
        {LOG_CATEGORIES.map((cat) => (
          <input
            key={cat}
            type="button"
            className="stdbtn"
            value={t(CATEGORY_BUTTON_LABELS[cat]) ?? undefined}
            onClick={() => setCategory(cat)}
          />
        ))}
      </span>
      <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />
    </div>
  )
}
