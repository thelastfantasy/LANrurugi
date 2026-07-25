import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import {
  useArchiveMetadata,
  useDeleteArchive,
  usePlugins,
  useSettings,
  useStats,
  useUpdateArchiveMetadata,
} from '../api/hooks'
import type { ArchiveMetadata } from '../api/types'
import TagInput from '../components/TagInput'
import { confirmDialog } from '../dialog'
import { routes } from '../routes'
import { toast } from '../toast'

export default function Edit() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { archiveId = '' } = useParams<{ archiveId: string }>()
  const metadata = useArchiveMetadata(archiveId)

  if (metadata.isLoading) {
    return (
      <div className="p-6" style={{ color: 'var(--theme-muted)' }}>
        {t('Loading library…')}
      </div>
    )
  }

  if (metadata.isError || !metadata.data) {
    return (
      <div className="p-6 flex flex-col gap-3">
        <p className="text-red-500">
          {t('Failed to load archives: {{error}}', { error: String(metadata.error) })}
        </p>
        <button type="button" onClick={() => navigate(routes.library())} className="self-start underline">
          {t('Return to Library')}
        </button>
      </div>
    )
  }

  // Keyed by archiveId so navigating directly from one archive's edit page to another's (e.g.
  // via a Tankoubon's member list) remounts this form with fresh initial state, rather than
  // needing an effect to re-sync it.
  return <EditForm key={archiveId} archiveId={archiveId} archive={metadata.data} />
}

function EditForm({ archiveId, archive }: { archiveId: string; archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const plugins = usePlugins('metadata')
  const settings = useSettings()
  const stats = useStats(2)
  const updateMetadata = useUpdateArchiveMetadata(archiveId)
  const deleteArchive = useDeleteArchive()

  const [title, setTitle] = useState(archive.title)
  const [summary, setSummary] = useState(archive.summary ?? '')
  const [tags, setTags] = useState(archive.tags)
  const [selectedPlugin, setSelectedPlugin] = useState('')
  const [pluginArg, setPluginArg] = useState('')
  const [pluginRunning, setPluginRunning] = useState(false)

  // Real legacy's own suggestion list (`Edit.suggestions`, `edit.js:49-54`): every tag used at
  // least twice across the library (`useStats(2)`), rendered as `namespace:text` when namespaced.
  const tagSuggestions = (stats.data ?? []).map((s) => (s.namespace ? `${s.namespace}:${s.text}` : s.text))

  async function handleSave() {
    await updateMetadata.mutateAsync({ title, summary, tags })
  }

  async function handleDelete() {
    if (!(await confirmDialog(t('Are you sure you want to delete this archive?') ?? ''))) return
    await deleteArchive.mutateAsync(archiveId)
    navigate('/')
  }

  // Real legacy's `Edit.runPlugin` always saves current form state FIRST, then fetches+merges
  // plugin tags (`edit.js:343-345`: `Edit.saveMetadata().then(() => Edit.getTags())`) — it's not a
  // preview, it unconditionally persists whatever's in the fields right now, matching the warning
  // text shown below the plugin row.
  async function runPlugin() {
    if (!selectedPlugin) return
    setPluginRunning(true)
    try {
      await updateMetadata.mutateAsync({ title, summary, tags })
      const response = await fetch(
        `/api/plugins/use?plugin=${encodeURIComponent(selectedPlugin)}&id=${encodeURIComponent(archiveId)}&arg=${encodeURIComponent(pluginArg)}`,
        { method: 'POST' },
      )
      const data = (await response.json()) as {
        success: number
        data?: { tags?: string; title?: string; summary?: string }
        error?: string
      }
      if (data.success && data.data) {
        const result = data.data
        // Mirrors legacy's own `Edit.getTags` exactly (`edit.js:293-337`): a toast per changed
        // field, plus always one tags-outcome toast (added vs. none) whether or not new tags
        // came back.
        // Gated on the global "Allow Plugins to replace archive titles" toggle
        // (`Model/Config.pm::can_replacetitles`, defaults to true) — mirrors legacy's own
        // `Model::Plugins::exec_metadata_plugin` gate exactly (tags/summary are never gated,
        // only title).
        if (result.title && (settings.data?.replacetitles ?? true)) {
          setTitle(result.title)
          toast({ heading: t('Archive title changed to') ?? undefined, text: result.title, icon: 'info' })
        }
        if (result.summary) {
          setSummary(result.summary)
          toast({ heading: t('Archive summary updated!') ?? undefined, icon: 'info' })
        }
        if (result.tags) {
          setTags((prev) => [prev, result.tags].filter(Boolean).join(', '))
          toast({ heading: t('Added the following tags') ?? undefined, text: result.tags, icon: 'info', hideAfter: 7000 })
        } else {
          toast({ heading: t('No new tags added!') ?? undefined, icon: 'info' })
        }
      } else {
        toast({ text: data.error ?? t('unknown error') ?? undefined, icon: 'error' })
      }
    } finally {
      setPluginRunning(false)
    }
  }

  const selectedPluginData = plugins.data?.find((p) => p.namespace === selectedPlugin)

  return (
    <div className="ido" style={{ textAlign: 'center', maxWidth: 800, margin: '10px auto' }}>
      {/* Real legacy's `Edit.pm::index` (the plain-archive, non-Tankoubon branch this page covers)
          never passes an `artist` template var at all — only `edit_tankoubon` does — so the
          "Editing %1 by %2" heading variant never actually fires for a plain archive; always the
          plain "Editing %1" form. */}
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Editing %1').replace('%1', archive.title)}
      </h2>

      <form
        autoComplete="off"
        style={{ width: '98%', maxWidth: 700, margin: '0 auto', fontSize: '8pt' }}
        onSubmit={(e) => e.preventDefault()}
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10, textAlign: 'left' }}>
          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'center', gap: 6 }}>
            <span>{t('Current File Name:')}</span>
            <input readOnly className="stdinput" type="text" style={{ width: '100%' }} value={archive.filename} />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'center', gap: 6 }}>
            <span>{t('ID:')}</span>
            <input readOnly className="stdinput" type="text" style={{ width: '100%' }} maxLength={255} value={archiveId} />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'center', gap: 6 }}>
            <span>{t('Title:')}</span>
            <input
              id="title"
              className="stdinput"
              type="text"
              style={{ width: '100%' }}
              maxLength={255}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'start', gap: 6 }}>
            <span>{t('Summary:')}</span>
            <textarea
              id="summary"
              className="stdinput"
              style={{ width: '100%', minHeight: 72, boxSizing: 'border-box' }}
              value={summary}
              onChange={(e) => setSummary(e.target.value)}
            />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'start', gap: 6 }}>
            <span>
              {t('Tags')} <span style={{ fontSize: '6pt' }}>{t('(separated by hyphens, i.e : tag1, tag2)')}</span> :
            </span>
            <TagInput value={tags} onChange={setTags} suggestions={tagSuggestions} />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'start', gap: 6 }}>
            <span>{t('Import Tags from Plugin :')}</span>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 6, alignItems: 'flex-start', textAlign: 'left' }}>
              <div style={{ display: 'flex', gap: 6 }}>
                <select
                  className="favtag-btn"
                  value={selectedPlugin}
                  onChange={(e) => setSelectedPlugin(e.target.value)}
                >
                  <option value="">{t(' -- No Category -- ')}</option>
                  {plugins.data?.map((p) => (
                    <option key={p.namespace} value={p.namespace}>
                      {p.name}
                    </option>
                  ))}
                </select>

                <input
                  className="stdbtn"
                  type="button"
                  style={{ minWidth: 90 }}
                  disabled={!selectedPlugin || pluginRunning}
                  onClick={() => void runPlugin()}
                  value={t('Go!') ?? undefined}
                />
              </div>

              {selectedPluginData?.oneshot_arg && (
                <div style={{ display: 'flex', flexDirection: 'column', gap: 4, width: '100%' }}>
                  <span>{selectedPluginData.oneshot_arg} :</span>
                  <input
                    className="stdinput"
                    type="text"
                    style={{ width: '100%', boxSizing: 'border-box' }}
                    value={pluginArg}
                    onChange={(e) => setPluginArg(e.target.value)}
                  />
                </div>
              )}

              <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <i className="fa fa-2x fa-exclamation-circle"></i>
                {t('Using a Plugin will save any modifications to archive metadata you might have made !')}
              </div>
            </div>
          </div>

          <div style={{ display: 'flex', flexWrap: 'wrap', justifyContent: 'center', gap: 8, marginTop: 10 }}>
            <input
              className="stdbtn"
              type="button"
              value={t('Save Metadata') ?? undefined}
              onClick={() => void handleSave()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t('Delete Archive') ?? undefined}
              onClick={() => void handleDelete()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t('Read Archive') ?? undefined}
              onClick={() => navigate(routes.reader(archiveId))}
            />
            <input
              className="stdbtn"
              type="button"
              value={t('Return to Library') ?? undefined}
              onClick={() => navigate(routes.library())}
            />
          </div>
        </div>
      </form>
    </div>
  )
}
