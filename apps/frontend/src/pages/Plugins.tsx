import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { useArchives, usePlugins } from '../api/hooks'
import type { PluginInfo } from '../api/types'
import CollapsibleSection from '../components/CollapsibleSection'
import { useApplyTheme } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'

// Legacy's own left/right split (`~/LANraragi/templates/plugins.html.tt2`): left column is
// Login Plugins → Downloaders → Scripts (script-*type* plugins, not this app's own maintenance
// scripts below), right column is just Metadata Plugins.
const LEFT_GROUPS: Array<{ type: PluginInfo['type']; icon: string; label: string }> = [
  { type: 'login', icon: 'fa-plug', label: 'Login Plugins' },
  { type: 'download', icon: 'fa-cloud-download-alt', label: 'Downloaders' },
]
const RIGHT_GROUPS: Array<{ type: PluginInfo['type']; icon: string; label: string }> = [
  { type: 'metadata', icon: 'fa-digital-tachograph', label: 'Metadata Plugins' },
]

// Mirrors legacy's `~/LANraragi/templates/plugins.html.tt2` — plugins grouped into
// `.collapsible.extensible.with-right-caret` > `.option-flyout` flyouts by type, each plugin a
// card with icon/name/version/author/description + a `.stdbtn` action. Doesn't reproduce
// allcollapsible.js's expand/collapse (flyouts render always-open) or per-plugin parameter-arg
// editing (this app's plugin-settings API isn't wired into this page yet) — script-type
// "Library-wide maintenance scripts" have no legacy equivalent template at all (a genuinely new
// feature area), kept in its own plain section rather than forced into legacy's per-plugin card
// shape.
export default function Plugins() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const plugins = usePlugins('all')
  const archives = useArchives()
  const queryClient = useQueryClient()
  const [selectedArchive, setSelectedArchive] = useState('')
  const [running, setRunning] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)
  const [sourceUrl, setSourceUrl] = useState('')
  useApplyTheme()
  useDocumentTitle(t('Plugin Configuration') ?? undefined)

  async function runPlugin(namespace: string) {
    if (!selectedArchive) return
    setRunning(namespace)
    setResult(null)
    try {
      const response = await fetch(
        `/api/plugins/use?plugin=${encodeURIComponent(namespace)}&id=${encodeURIComponent(selectedArchive)}`,
        { method: 'POST' },
      )
      const data = (await response.json()) as { success: number; data?: unknown; error?: string }
      setResult(
        data.success
          ? JSON.stringify(data.data, null, 2)
          : (data.error ?? t('unknown error') ?? ''),
      )
    } finally {
      setRunning(null)
    }
  }

  async function runScript(path: string, params?: Record<string, string>) {
    setRunning(path)
    setResult(null)
    try {
      const query = params ? `?${new URLSearchParams(params)}` : ''
      const response = await fetch(`/api/database/scripts/${path}${query}`, { method: 'POST' })
      const data = (await response.json()) as Record<string, unknown>
      setResult(JSON.stringify(data, null, 2))
      await queryClient.invalidateQueries({ queryKey: ['archives'] })
      await queryClient.invalidateQueries({ queryKey: ['categories'] })
    } finally {
      setRunning(null)
    }
  }

  function renderGroupFlyout(group: { type: PluginInfo['type']; icon: string; label: string }) {
    const groupPlugins = plugins.data?.filter((p) => p.type === group.type) ?? []
    return (
      <CollapsibleSection icon={group.icon} title={t(group.label)} key={group.type}>
        {groupPlugins.length === 0 && <p>{t('No plugins installed.')}</p>}
        {groupPlugins.map((p) => (
          <div key={p.namespace} style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, padding: '4px 0' }}>
            <span>
              {p.icon ? <img src={p.icon} alt="" style={{ height: 24, verticalAlign: 'middle' }} /> : <i className="fa fa-puzzle-piece"></i>}{' '}
              <span className="ih" style={{ fontWeight: 'bold' }}>
                {p.name} v{p.version}
              </span>
              <br />
              <span className="ih">{p.author}</span>
              <br />
              {p.description}
            </span>
            <input
              type="button"
              className="stdbtn"
              disabled={!selectedArchive || running === p.namespace}
              onClick={() => void runPlugin(p.namespace)}
              value={(running === p.namespace ? t('Loading library…') : t('Use Plugin')) ?? undefined}
            />
          </div>
        ))}
      </CollapsibleSection>
    )
  }

  return (
    <div className="ido">
      <h1 className="ih">{t('Plugins')}</h1>
      <p style={{ textAlign: 'center' }}>
        <a href="/docs/" target="_blank" rel="noopener noreferrer">
          <i className="fa fa-book"></i> {t('Plugin SDK Documentation')}
        </a>
      </p>

      <table style={{ margin: 'auto' }}>
        <tbody>
          <tr>
            <td>{t('Target archive')}</td>
            <td>
              <select value={selectedArchive} onChange={(e) => setSelectedArchive(e.target.value)} className="favtag-btn">
                <option value="">{t(' -- No Category -- ')}</option>
                {archives.data?.map((a) => (
                  <option key={a.arcid} value={a.arcid}>
                    {a.title}
                  </option>
                ))}
              </select>
            </td>
          </tr>
        </tbody>
      </table>

      <form name="editPluginForm">
        <div className="left-column" style={{ width: '49%' }}>
          <ul className="collapsible extensible with-right-caret">
            {LEFT_GROUPS.map(renderGroupFlyout)}

            {/* This app's own library-wide maintenance scripts — a genuinely new feature with no
                legacy template of its own (legacy's *own* "Scripts" flyout here is for
                script-*type* plugins instead, which `renderGroupFlyout` already covers if/when
                `PluginInfo['type']` grows a `'script'` variant). */}
            <CollapsibleSection icon="fa-scroll" title={t('Scripts')}>
                <p>{t('Library-wide maintenance scripts (operate on the whole database, not one archive).')}</p>

                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, padding: '4px 0' }}>
                  <span>
                    <b>{t('Subfolders to Categories')}</b>
                    <br />
                    {t('Scan your Content Folder and automatically create Static Categories for each subfolder.')}
                  </span>
                  <input
                    type="button"
                    className="stdbtn"
                    disabled={running === 'subfolders-to-categories'}
                    onClick={() => void runScript('subfolders-to-categories')}
                    value={t('Run') ?? undefined}
                  />
                </div>

                <div style={{ padding: '4px 0' }}>
                  <b>{t('Source Finder')}</b>
                  <br />
                  {t("Looks in the database if an archive has a 'source:' tag matching the given URL.")}
                  <br />
                  <input value={sourceUrl} onChange={(e) => setSourceUrl(e.target.value)} placeholder={t('URL to search.') ?? undefined} className="stdinput" />
                  <input
                    type="button"
                    className="stdbtn"
                    disabled={running === 'source-finder' || !sourceUrl.trim()}
                    onClick={() => void runScript('source-finder', { url: sourceUrl })}
                    value={t('Run') ?? undefined}
                  />
                </div>

                <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, padding: '4px 0' }}>
                  <span>
                    <b>{t('nHentai Source Converter')}</b>
                    <br />
                    {t('Converts "source:{id}" tags with 6 or less digits into "source:nhentai.net/g/{id}"')}
                  </span>
                  <input
                    type="button"
                    className="stdbtn"
                    disabled={running === 'nhentai-source-converter'}
                    onClick={() => void runScript('nhentai-source-converter')}
                    value={t('Run') ?? undefined}
                  />
                </div>
            </CollapsibleSection>
          </ul>
        </div>

        <div className="right-column" style={{ width: '50%' }}>
          <ul className="collapsible extensible with-right-caret">{RIGHT_GROUPS.map(renderGroupFlyout)}</ul>
        </div>
      </form>

      {result && (
        <table className="itg">
          <tbody>
            <tr className="gtr1">
              <td>
                <pre className="log-panel">{result}</pre>
              </td>
            </tr>
          </tbody>
        </table>
      )}

      <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />
    </div>
  )
}
