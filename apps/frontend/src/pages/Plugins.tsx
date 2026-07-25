import { useQueryClient } from '@tanstack/react-query'
import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import {
  usePluginOptions,
  usePlugins,
  usePluginSettings,
  useReorderPlugins,
  useSettings,
  useUpdatePluginSettings,
  useUpdateSettings,
} from '../api/hooks'
import type { PluginInfo } from '../api/types'
import CollapsibleSection from '../components/CollapsibleSection'
import SortableList, { type DragHandleProps } from '../components/SortableList'
import { routes } from '../routes'
import { ensureLink, removeLink, useApplyTheme } from '../theme'
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
// against an archive" affordance here — legacy has none either: a metadata plugin's "Run
// Automatically" checkbox controls whether it fires on new archives, a script-type plugin has its
// own "Trigger Script" button, and a download plugin is only invoked via the upload page's
// URL-download form. Manually running one plugin against one existing archive is `Edit.tsx`'s job
// — script-type "Library-wide maintenance scripts" below have no legacy equivalent at all.
export default function Plugins() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const plugins = usePlugins('all')
  const settings = useSettings()
  const updateSettings = useUpdateSettings()
  const queryClient = useQueryClient()
  const [running, setRunning] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)
  const [uploadStatus, setUploadStatus] = useState<string | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  useApplyTheme()
  useDocumentTitle(t('Plugin Configuration') ?? undefined)

  // Real legacy also links `config.css` from `plugins.html.tt2`, same as `Settings.tsx`'s own
  // identical block — it globally restyles every `input[type=checkbox]` into the real ON/OFF
  // toggle look, which an SPA has to link/unlink by hand rather than getting for free.
  useEffect(() => {
    ensureLink('legacy-config-css', '/legacy/config.css')
    return () => removeLink('legacy-config-css')
  }, [])

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
        {/* Legacy's own `plugins.html.tt2:84-91` — sits at the top of the Metadata Plugins flyout
            body, above the plugin list itself. Gates whether a metadata plugin's returned `title`
            is actually applied to an archive (`Edit.tsx`'s own `handleSave`/`runPlugin`); tags and
            summary are never gated. */}
        {group.type === 'metadata' && (
          <div style={{ padding: '4px 0 8px 0' }}>
            <h1 className="ih" style={{ display: 'inline' }}>
              {t('Allow Plugins to replace archive titles:')}{' '}
            </h1>
            <input
              id="replacetitles"
              className="fa"
              type="checkbox"
              checked={settings.data?.replacetitles ?? true}
              onChange={(e) => updateSettings.mutate({ replacetitles: e.target.checked })}
            />
            <label htmlFor="replacetitles">
              <br />
              {t(
                'If enabled, metadata plugins will be able to change the title of your archives alongside adding tags to them.',
              )}
            </label>
          </div>
        )}
        {groupPlugins.length === 0 ? (
          <p>{t('No plugins installed.')}</p>
        ) : (
          <SortablePluginGroup type={group.type} plugins={groupPlugins} />
        )}
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
              legacy template, labeled distinctly from legacy's real "Scripts" flyout above
              (per-plugin script execution) so the two aren't confused. Only "Subfolders to
              Categories" lives here — it walks the entire archive directory tree, I/O-heavy enough
              to warrant a native Rust endpoint rather than a Deno-subprocess round trip. Source
              Finder / nHentai Source Converter are real `script`-type plugins and render as
              ordinary cards in the "Scripts" flyout above. */}
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

      {/* `justifyContent`/`alignItems: center` (flex) instead of plain `text-align: center` —
          a `<span class="fileinput-button">` (text baseline) and an `<input type="button">`
          (replaced-ish element) align differently under `vertical-align: baseline` when sitting
          side by side inline, offsetting the Upload Plugin button above the Return to Library
          button. Flex sidesteps the baseline-vs-replaced-element quirk entirely. */}
      <h1 style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', gap: 8 }}>
        {/* Also flex: an `<input type="button">`'s value text is vertically centered in its
            content box natively, but a `<span>` wrapping plain text just follows normal inline
            flow, leaving the label sitting near the top of the box instead of centered.
            `fontWeight: 'normal'` on the inner label: this `<h1>`'s own bold inherits into a plain
            `<span>` child, since form controls don't inherit a heading's bold by browser UA
            default the way ordinary text elements do. */}
        <span className="stdbtn fileinput-button" style={{ display: 'inline-flex', alignItems: 'center', justifyContent: 'center' }}>
          <span style={{ fontWeight: 'normal' }}>{t('Upload Plugin')}</span>
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
        </span>
        <input type="button" id="return" className="stdbtn" value={t('Return to Library') ?? undefined} onClick={() => navigate(routes.library())} />
      </h1>
    </div>
  )
}

/** One `type` group's drag-to-reorder plugin list (additive — legacy has no concept of plugin
 * priority at all). Local `order` state is seeded from (and re-synced with) the server's already
 * priority-sorted list, so a drag reorders instantly without waiting on the round trip, and a
 * background refetch doesn't fight the in-progress drag. On drop, persists the complete new order
 * via `useReorderPlugins` — this matters because `findMatchingPlugin` (Upload.tsx) picks the first
 * URL-pattern match in this exact order, so dragging one plugin above another is what makes it the
 * one actually used for a URL both could handle. */
function SortablePluginGroup({ type, plugins }: { type: PluginInfo['type']; plugins: PluginInfo[] }) {
  const reorder = useReorderPlugins()
  const serverOrder = plugins.map((p) => p.namespace)
  const serverOrderKey = serverOrder.join(',')

  // Local `order` state only reflects an in-progress/just-finished drag ahead of the server round
  // trip — reset during render (React's pattern for "adjust state when a prop changes", not a
  // `useEffect`) whenever the server's list changes for a reason other than this component's own
  // drag. Skipped while a drag-triggered mutation is still in flight, so the just-dropped order
  // doesn't visibly snap back to the pre-drag server value.
  const [order, setOrder] = useState(serverOrder)
  const [syncedKey, setSyncedKey] = useState(serverOrderKey)
  if (serverOrderKey !== syncedKey && !reorder.isPending) {
    setOrder(serverOrder)
    setSyncedKey(serverOrderKey)
  }

  const byNamespace = new Map(plugins.map((p) => [p.namespace, p]))
  const orderedPlugins = order.map((namespace) => byNamespace.get(namespace)).filter((p): p is PluginInfo => !!p)

  return (
    <SortableList
      items={orderedPlugins}
      getId={(p) => p.namespace}
      onReorder={(next) => {
        setOrder(next)
        reorder.mutate({ type, order: next })
      }}
      renderItem={(plugin, dragHandleProps) => <PluginCard plugin={plugin} dragHandleProps={dragHandleProps} />}
    />
  )
}

/** Width of the drag-handle column — narrow (just enough for the grip glyph plus a little
 * click-target padding), not a content-sized column, which would read as an oversized empty
 * gutter next to `PluginCard`'s own `width: 80%` inset. */
const DRAG_HANDLE_COLUMN_WIDTH = 18

// Per-plugin card, mirroring legacy's exact `pluginlist` markup: an inline-block `<span>` at 80%
// width with a bottom-border separator, name/version/author inline, "Run Automatically"/"depends
// on login plugin" floated right, then description, then the script-trigger table or "Plugin
// Settings" accordion. Plus this app's own additive "Download Settings" for download plugins
// whose `pluginOptions()` resolves — distinctly labeled from "Plugin Settings" so the two aren't
// confused.
function PluginCard({
  plugin,
  dragHandleProps,
}: {
  plugin: PluginInfo
  dragHandleProps: DragHandleProps
}) {
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

  const { attributes, listeners, isDragging } = dragHandleProps

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'stretch',
        // Legacy's own markup uses two trailing `<br />` tags after each card to create the gap
        // before the next card — real vertical space in legacy's non-flex layout. Inside this flex
        // row, a `<br>` is just a zero-width flex item contributing nothing to row height, so
        // `marginBottom: 28` (2x the 14px a `<br>` renders at) reproduces the same visual gap
        // explicitly, applying uniformly across every theme.
        marginBottom: 28,
        // Only ever true inside `SortableList`'s own `DragOverlay` — this element is genuinely
        // detached from the page's normal layout flow at that point, so a lift shadow/scale-up/
        // raised z-index here can't squish or overlap any other row.
        ...(isDragging && {
          zIndex: 1,
          position: 'relative',
          boxShadow: '0 8px 16px rgba(0, 0, 0, 0.35)',
          scale: '1.02',
        }),
      }}
    >
      <span
        {...attributes}
        {...listeners}
        style={{
          width: DRAG_HANDLE_COLUMN_WIDTH,
          flexShrink: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          cursor: isDragging ? 'grabbing' : 'grab',
          touchAction: 'none',
          fontSize: '0.9em',
          opacity: 0.5,
        }}
      >
        <i className="fa fa-grip-vertical" aria-hidden="true"></i>
      </span>
      <span
        style={{
          display: 'inline-block',
          textAlign: 'left',
          // Legacy hardcodes `width: 80%` here, a fixed proportion that leaves its right-floated
          // "Run Automatically" checkbox stopping short of the panel's actual right edge. `flex: 1`
          // instead fills 100% of the row's remaining width, a deliberate improvement over
          // legacy's layout, not a parity target.
          flex: 1,
          minWidth: 0,
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
        {/* Plugin-declared static HTML (`<br/>`, `<i class="fa ...">`, etc., not user input) —
            legacy's own template renders this the same way, unescaped. */}
        <span dangerouslySetInnerHTML={{ __html: plugin.description }} />
        <br />

        {plugin.type === 'script' && (
          <table>
            <tbody>
              {plugin.oneshot_arg && (
                <tr>
                  <td style={{ verticalAlign: 'middle' }}>
                    <b dangerouslySetInnerHTML={{ __html: `${t(plugin.oneshot_arg)} :` }} />
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
    </div>
  )
}
