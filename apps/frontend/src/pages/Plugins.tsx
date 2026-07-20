import { useQueryClient } from '@tanstack/react-query'
import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { usePluginOptions, usePlugins, usePluginSettings, useUpdatePluginSettings } from '../api/hooks'
import type { PluginInfo } from '../api/types'
import CollapsibleSection from '../components/CollapsibleSection'
import { useApplyTheme } from '../theme'
import { useDocumentTitle } from '../useDocumentTitle'
import PluginOptionsForm from './PluginOptionsForm'
import PluginParametersForm from './PluginParametersForm'

// Legacy's own left/right split (`~/LANraragi/templates/plugins.html.tt2`): left column is
// Login Plugins → Downloaders → Scripts (script-*type* plugins, not this app's own maintenance
// scripts below), right column is just Metadata Plugins.
const LEFT_GROUPS: Array<{ type: PluginInfo['type']; icon: string; label: string }> = [
  { type: 'login', icon: 'fa-plug', label: 'Login Plugins' },
  { type: 'download', icon: 'fa-cloud-download-alt', label: 'Downloaders' },
  { type: 'script', icon: 'fa-scroll', label: 'Scripts' },
]
const RIGHT_GROUPS: Array<{ type: PluginInfo['type']; icon: string; label: string }> = [
  { type: 'metadata', icon: 'fa-digital-tachograph', label: 'Metadata Plugins' },
]

// Mirrors legacy's `~/LANraragi/templates/plugins.html.tt2` — plugins grouped into
// `.collapsible.extensible.with-right-caret` > `.option-flyout` flyouts by type, each plugin a
// card with icon/name/version/author/description. There is deliberately no "run this plugin
// against an archive" affordance here — legacy has none either (verified against the real
// template): a metadata plugin's "Run Automatically" checkbox controls whether it fires on new
// archives, a script-type plugin has its own "Trigger Script" button, and a download plugin is
// only ever invoked via the upload page's URL-download form. Manually running one plugin against
// one *existing* archive is `Edit.tsx`'s job (legacy's own `edit.html.tt2` has that entry point,
// not this page) — script-type "Library-wide maintenance scripts" below have no legacy equivalent
// template at all (a genuinely new feature area), kept in its own plain section rather than
// forced into legacy's per-plugin card shape.
export default function Plugins() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const plugins = usePlugins('all')
  const queryClient = useQueryClient()
  const [running, setRunning] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)
  const [sourceUrl, setSourceUrl] = useState('')
  const [uploadStatus, setUploadStatus] = useState<string | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  useApplyTheme()
  useDocumentTitle(t('Plugin Configuration') ?? undefined)

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

  async function uploadPlugin(file: File) {
    setUploadStatus(null)
    const body = new FormData()
    body.set('file', file)
    try {
      const response = await fetch('/api/plugins/upload', { method: 'POST', body })
      const data = (await response.json()) as { success: number; name?: string; error?: string }
      setUploadStatus(
        data.success ? t('Plugin uploaded: {{name}}', { name: data.name }) : (data.error ?? t('Upload failed.') ?? ''),
      )
      if (data.success) await queryClient.invalidateQueries({ queryKey: ['plugins'] })
    } finally {
      if (fileInputRef.current) fileInputRef.current.value = ''
    }
  }

  function renderGroupFlyout(group: { type: PluginInfo['type']; icon: string; label: string }) {
    const groupPlugins = plugins.data?.filter((p) => p.type === group.type) ?? []
    return (
      <CollapsibleSection icon={group.icon} title={t(group.label)} key={group.type}>
        {groupPlugins.length === 0 && <p>{t('No plugins installed.')}</p>}
        {groupPlugins.map((p) => (
          <PluginCard key={p.namespace} plugin={p} />
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

      <div className="left-column" style={{ width: '49%' }}>
        <ul className="collapsible extensible with-right-caret">
          {LEFT_GROUPS.map(renderGroupFlyout)}

          {/* This app's own library-wide maintenance scripts — a genuinely new feature with no
              legacy template of its own, labeled distinctly from legacy's real "Scripts" flyout
              right above it (`LEFT_GROUPS`'s own `'script'`-type entry, per-plugin script
              execution) so the two aren't confused for one another. */}
          <CollapsibleSection icon="fa-scroll" title={t('Maintenance Scripts')}>
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

      {uploadStatus && <p style={{ textAlign: 'center' }}>{uploadStatus}</p>}

      <h1 style={{ textAlign: 'center' }}>
        <span className="stdbtn fileinput-button" style={{ display: 'inline-block' }}>
          <span>{t('Upload Plugin')}</span>
          <input
            ref={fileInputRef}
            type="file"
            accept=".ts"
            style={{ display: 'none' }}
            onChange={(e) => {
              const file = e.target.files?.[0]
              if (file) void uploadPlugin(file)
            }}
          />
        </span>{' '}
        <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate('/')} />
      </h1>
    </div>
  )
}

// Per-plugin card, mirroring legacy's exact `pluginlist` markup (`plugins.html.tt2` lines
// 121-215): an inline-block `<span>` at 80% width with a bottom-border separator between entries,
// name/version/author inline, "Run Automatically"/"depends on login plugin" floated right, then
// description, then (in order) the script-trigger table or "Plugin Settings" accordion. Plus this
// app's own additive "Download Settings" (concurrency/rate-limit/bundling,
// specs/005-download-plugin-progress) for download plugins whose `pluginOptions()` resolves —
// deliberately distinctly labeled from "Plugin Settings" so the two aren't confused for one
// another, rendered inside the same floated-right corner legacy uses for its own toggles.
function PluginCard({ plugin }: { plugin: PluginInfo }) {
  const { t } = useTranslation()
  const [downloadSettingsOpen, setDownloadSettingsOpen] = useState(false)
  const [scriptArg, setScriptArg] = useState('')
  const [scriptRunning, setScriptRunning] = useState(false)

  const options = usePluginOptions(plugin.type === 'download' ? plugin.namespace : '')
  const hasDownloadOptions = plugin.type === 'download' && Boolean(options.data)
  const hasParameters = plugin.parameters.length > 0

  const settings = usePluginSettings(plugin.type === 'metadata' ? plugin.namespace : '')
  const updateSettings = useUpdatePluginSettings(plugin.namespace)

  async function triggerScript() {
    setScriptRunning(true)
    try {
      const query = new URLSearchParams({ plugin: plugin.namespace })
      if (scriptArg) query.set('arg', scriptArg)
      await fetch(`/api/plugins/queue?${query}`, { method: 'POST' })
    } finally {
      setScriptRunning(false)
    }
  }

  return (
    <>
      <span
        style={{
          display: 'inline-block',
          textAlign: 'left',
          width: '80%',
          borderBottomWidth: 1,
          borderBottomStyle: 'solid',
        }}
      >
        {plugin.icon ? (
          <img height={20} width={20} src={plugin.icon} alt="" />
        ) : (
          <i className="fa fa-puzzle-piece" style={{ fontSize: 20 }}></i>
        )}
        <h2 className="ih" style={{ display: 'inline' }}>
          {' '}
          {plugin.name} v.{plugin.version}
        </h2>
        <h1 className="ih" style={{ display: 'inline' }}>
          {' '}
          by {plugin.author}{' '}
        </h1>

        <div style={{ float: 'right', textAlign: 'right' }}>
          {plugin.type === 'metadata' && settings.data && (
            <>
              <h1 className="ih" style={{ display: 'inline' }}>
                {' '}
                {t('Run Automatically')}:{' '}
              </h1>
              <input
                type="checkbox"
                className="fa"
                checked={settings.data.enabled}
                onChange={(e) => void updateSettings.mutateAsync({ enabled: e.target.checked })}
              />
              <br />
            </>
          )}
          {hasDownloadOptions && (
            <input
              type="button"
              className="stdbtn"
              value={t('Download Settings') ?? undefined}
              onClick={() => setDownloadSettingsOpen((o) => !o)}
            />
          )}
          {plugin.login_from && (
            <>
              <i className="fa fa-plug" aria-hidden="true"></i>{' '}
              {t('This plugin depends on the login plugin')} "{plugin.login_from}".
            </>
          )}
        </div>

        <br />
        {plugin.description}
        <br />

        {plugin.type === 'script' && (
          <table>
            <tbody>
              {plugin.oneshot_arg && (
                <tr>
                  <td style={{ verticalAlign: 'middle' }}>
                    <b>{t(plugin.oneshot_arg)} :</b>
                  </td>
                  <td>
                    <input style={{ maxWidth: 200 }} size={20} value={scriptArg} onChange={(e) => setScriptArg(e.target.value)} />
                  </td>
                </tr>
              )}
              <tr>
                <td colSpan={2}>
                  <input
                    type="button"
                    className="stdbtn"
                    disabled={scriptRunning}
                    onClick={() => void triggerScript()}
                    value={t('Trigger Script') ?? undefined}
                  />
                </td>
              </tr>
            </tbody>
          </table>
        )}

        {hasParameters && <PluginParametersForm namespace={plugin.namespace} parameters={plugin.parameters} />}
        {downloadSettingsOpen && hasDownloadOptions && (
          <PluginOptionsForm namespace={plugin.namespace} onClose={() => setDownloadSettingsOpen(false)} />
        )}

        <br />
      </span>
      <br />
      <br />
    </>
  )
}
