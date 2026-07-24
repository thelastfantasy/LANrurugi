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
  const settings = useSettings()
  const updateSettings = useUpdateSettings()
  const queryClient = useQueryClient()
  const [running, setRunning] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)
  const [uploadStatus, setUploadStatus] = useState<string | null>(null)
  const fileInputRef = useRef<HTMLInputElement>(null)
  useApplyTheme()
  useDocumentTitle(t('Plugin Configuration') ?? undefined)

  // Real legacy also links `config.css` from `plugins.html.tt2` (not just `config.html.tt2`) —
  // same reasoning as `Settings.tsx`'s own identical block: it globally restyles every
  // `input[type=checkbox]` on the page into the real ON/OFF toggle look, which an SPA has to
  // link/unlink by hand per-page rather than getting "for free" from a full page load. Missing
  // here left every checkbox on this page (自动运行/允许插件替换档案标题/every bool-typed plugin
  // parameter) rendering as a bare, unstyled native checkbox instead of a switch.
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
              legacy template of its own, labeled distinctly from legacy's real "Scripts" flyout
              right above it (`LEFT_GROUPS`'s own `'script'`-type entry, per-plugin script
              execution) so the two aren't confused for one another. Only "Subfolders to
              Categories" lives here — it walks the entire archive directory tree, I/O-heavy enough
              to be worth a native Rust endpoint rather than a Deno-subprocess round trip. Source
              Finder / nHentai Source Converter are real `script`-type plugins now
              (`plugins/script/{sourcefinder,nhentaisourceconverter}.ts`) and render as ordinary
              cards in the "Scripts" flyout above, like any other script plugin. */}
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

      {/* `justifyContent`/`alignItems: center` (flex) instead of the plain `text-align: center`
          markup legacy uses — a `<span class="fileinput-button">` (text baseline) and an
          `<input type="button">` (treated as a replaced-ish element) align differently under
          `vertical-align: baseline` when sitting side by side inline, visibly offsetting the
          Upload Plugin button above the Return to Library button. `.stdbtn`'s own CSS (vendored
          verbatim from each theme) sets no `vertical-align` override, and the real demo's Plugins
          page is login-gated so this couldn't be confirmed against live legacy computed style —
          flex alignment sidesteps the baseline-vs-replaced-element quirk entirely without touching
          the vendored theme CSS or diverging from any confirmed real value. */}
      <h1 style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', gap: 8 }}>
        {/* Also flex (not the plain `display: inline-block` legacy's own `.fileinput-button` CSS
            provides): an `<input type="button">`'s value text is vertically centered in its
            content box by the browser's own native rendering, but a `<span>` wrapping plain text
            has no such behavior — it just follows normal inline flow, which left the label sitting
            close to the top of the 28px-tall box instead of centered. `inline-flex` here gives the
            label the same effective centering an `<input>` gets for free.

            `fontWeight: 'normal'` on the inner label: this `<h1>`'s own bold inherits down into a
            plain `<span>` child (verified via computed style — `700` on the label vs. the sibling
            `<input>`'s `400`), since form controls don't inherit a heading's bold by browser UA
            default the way ordinary text elements do. `.stdbtn` itself never sets `font-weight` at
            all, so this resets the span back to that same un-bolded baseline instead of silently
            inheriting the heading's weight. */}
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

/** One `type` group's drag-to-reorder plugin list (additive — no legacy equivalent; legacy has no
 * concept of plugin priority at all). Local `order` state is the array of namespaces actually
 * rendered — seeded from (and re-synced with) the server's own already-priority-sorted list, so a
 * drag reorders instantly without waiting on the round trip, and a background refetch (e.g. after
 * another browser tab's own reorder) doesn't fight the in-progress drag. On drop, persists the
 * complete new order via `useReorderPlugins` (see its own docs — full list, not a single-item
 * delta) — this matters for "why multiple metadata plugins can both claim the same
 * `url_pattern`": `findMatchingPlugin` (Upload.tsx) picks the first match in this exact order, so
 * dragging FAKKU above Chaika.moe here is what makes FAKKU the one actually used for a URL both
 * could handle. */
function SortablePluginGroup({ type, plugins }: { type: PluginInfo['type']; plugins: PluginInfo[] }) {
  const reorder = useReorderPlugins()
  const serverOrder = plugins.map((p) => p.namespace)
  const serverOrderKey = serverOrder.join(',')

  // Local `order` state only exists to reflect an in-progress/just-finished drag ahead of the
  // server round trip — reset during render (React's own documented pattern for "adjust state
  // when a prop changes", not a `useEffect`, which would cause an extra render pass here) whenever
  // the server's own list changes for a reason other than this component's own drag (a different
  // plugin installed/removed, another tab's reorder, etc.). Skipped while a drag-triggered
  // mutation is still in flight, so the just-dropped order doesn't visibly snap back to the
  // pre-drag server value for the moment before the mutation's own refetch lands.
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
 * click-target padding), not a full flex `gap`-separated column sized by its own content, which
 * is what made an earlier version's handle column read as an oversized empty gutter next to
 * `PluginCard`'s own `width: 80%` inset. */
const DRAG_HANDLE_COLUMN_WIDTH = 18

// Per-plugin card, mirroring legacy's exact `pluginlist` markup (`plugins.html.tt2` lines
// 121-215): an inline-block `<span>` at 80% width with a bottom-border separator between entries,
// name/version/author inline, "Run Automatically"/"depends on login plugin" floated right, then
// description, then (in order) the script-trigger table or "Plugin Settings" accordion. Plus this
// app's own additive "Download Settings" (concurrency/rate-limit/bundling,
// specs/005-download-plugin-progress) for download plugins whose `pluginOptions()` resolves —
// deliberately distinctly labeled from "Plugin Settings" so the two aren't confused for one
// another, rendered inside the same floated-right corner legacy uses for its own toggles.
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
        // Legacy's own markup puts two trailing `<br />` tags after each card's closing `</span>`
        // (`plugins.html.tt2:213-214`) to create the gap before the next card — real vertical space
        // in legacy's non-flex layout, since each `<br>` there is a genuine block-level line break.
        // Inside *this* flex row, though, a `<br>` is just another flex item sitting side-by-side
        // with zero width, contributing nothing to the row's height (confirmed live: two `<br>`s
        // measured 14px tall each but 0 additional row height) — so the gap collapsed to 0 and the
        // border-bottom separator line sat flush against the very next card. `marginBottom: 28`
        // (2 × the 14px a `<br>` actually rendered at) reproduces the same visual gap explicitly,
        // applies uniformly across every theme (a real CSS property, not theme-dependent whitespace
        // rendering), and the two now-inert trailing `<br />`s below are removed rather than kept
        // as dead markup.
        marginBottom: 28,
        // Only ever true inside `SortableList`'s own `DragOverlay` (see that component's docs) —
        // this element is genuinely detached from the page's normal layout flow at that point, so
        // a lift shadow/scale-up/raised z-index here can't squish or overlap any other row the
        // way it did before the overlay refactor.
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
          // Legacy's own template hardcodes `width:80%` here (`plugins.html.tt2:124`) — a fixed
          // proportion of whatever containing block happens to be available, which is why real
          // legacy's own right-floated "Run Automatically" checkbox also stops short of the
          // panel's actual right edge (verified against the live reference instance) rather than
          // reaching it. `flex: 1` instead fills 100% of the row's remaining width (after the drag
          // handle column), removing that dead gutter entirely — a deliberate improvement over
          // legacy's own layout, not a parity target.
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
        {/* Plugin-declared static HTML (`<br/>`, `<i class="fa ...">`, etc. — verified against
            every shipped plugin's own `description` literal, not user input) — legacy's own
            template (`plugins.html.tt2`) renders this the same way, unescaped. */}
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
