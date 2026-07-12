import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import {
  useArchiveMetadata,
  useDeleteArchive,
  usePlugins,
  useUpdateArchiveMetadata,
} from '../api/hooks'
import type { ArchiveMetadata } from '../api/types'

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
        <button type="button" onClick={() => navigate('/')} className="self-start underline">
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
  const updateMetadata = useUpdateArchiveMetadata(archiveId)
  const deleteArchive = useDeleteArchive()

  const [title, setTitle] = useState(archive.title)
  const [summary, setSummary] = useState(archive.summary ?? '')
  const [tags, setTags] = useState(archive.tags)
  const [selectedPlugin, setSelectedPlugin] = useState('')
  const [pluginArg, setPluginArg] = useState('')
  const [pluginRunning, setPluginRunning] = useState(false)
  const [pluginResult, setPluginResult] = useState<string | null>(null)

  async function handleSave() {
    await updateMetadata.mutateAsync({ title, summary, tags })
  }

  async function handleDelete() {
    await deleteArchive.mutateAsync(archiveId)
    navigate('/')
  }

  async function runPlugin() {
    if (!selectedPlugin) return
    setPluginRunning(true)
    setPluginResult(null)
    try {
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
        if (result.tags) setTags((prev) => [prev, result.tags].filter(Boolean).join(', '))
        if (result.title) setTitle(result.title)
        if (result.summary) setSummary(result.summary)
        setPluginResult(t('This Plugin ran successfully!') ?? 'OK')
      } else {
        setPluginResult(data.error ?? t('unknown error') ?? '')
      }
    } finally {
      setPluginRunning(false)
    }
  }

  return (
    <div className="max-w-2xl mx-auto p-6 flex flex-col gap-4">
      <h1 className="text-2xl font-semibold">{t('Editing %1').replace('%1', archive.title)}</h1>

      <div className="flex flex-col gap-1">
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('Current File Name:')}
        </label>
        <input
          readOnly
          value={archive.filename}
          className="rounded border px-2 py-1 text-sm opacity-70"
          style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
        />
      </div>

      <div className="flex flex-col gap-1">
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('ID:')}
        </label>
        <input
          readOnly
          value={archiveId}
          className="rounded border px-2 py-1 text-sm opacity-70"
          style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
        />
      </div>

      <div className="flex flex-col gap-1">
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('Title:')}
        </label>
        <input
          value={title}
          onChange={(e) => setTitle(e.target.value)}
          className="rounded border px-2 py-1 text-sm"
          style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
        />
      </div>

      <div className="flex flex-col gap-1">
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('Summary:')}
        </label>
        <textarea
          value={summary}
          onChange={(e) => setSummary(e.target.value)}
          className="rounded border px-2 py-1 text-sm min-h-20"
          style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
        />
      </div>

      <div className="flex flex-col gap-1">
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('Tags')}{' '}
          <span className="opacity-70">{t('(separated by hyphens, i.e : tag1, tag2)')}</span>
        </label>
        <textarea
          value={tags}
          onChange={(e) => setTags(e.target.value)}
          className="rounded border px-2 py-1 text-sm min-h-24"
          style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
        />
      </div>

      <div
        className="rounded border p-3 flex flex-col gap-2"
        style={{ borderColor: 'var(--theme-border)' }}
      >
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('Import Tags from Plugin :')}
        </label>
        <div className="flex gap-2">
          <select
            value={selectedPlugin}
            onChange={(e) => setSelectedPlugin(e.target.value)}
            className="rounded border px-2 py-1 text-sm"
            style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
          >
            <option value="">{t(' -- No Category -- ')}</option>
            {plugins.data?.map((p) => (
              <option key={p.namespace} value={p.namespace}>
                {p.name}
              </option>
            ))}
          </select>
          <input
            value={pluginArg}
            onChange={(e) => setPluginArg(e.target.value)}
            placeholder={plugins.data?.find((p) => p.namespace === selectedPlugin)?.oneshot_arg ?? ''}
            className="flex-1 rounded border px-2 py-1 text-sm"
            style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
          />
          <button
            type="button"
            disabled={!selectedPlugin || pluginRunning}
            onClick={runPlugin}
            className="rounded px-3 py-1.5 text-sm disabled:opacity-50"
            style={{ backgroundColor: 'var(--theme-accent)', color: 'var(--theme-accent-fg)' }}
          >
            {t('Go!')}
          </button>
        </div>
        {pluginResult && <p className="text-xs">{pluginResult}</p>}
      </div>

      <div className="flex flex-wrap gap-2 justify-center pt-2">
        <button
          type="button"
          onClick={handleSave}
          className="rounded px-3 py-1.5 text-sm"
          style={{ backgroundColor: 'var(--theme-accent)', color: 'var(--theme-accent-fg)' }}
        >
          {t('Save Metadata')}
        </button>
        <button
          type="button"
          onClick={() => navigate(`/reader/${archiveId}`)}
          className="rounded border px-3 py-1.5 text-sm"
          style={{ borderColor: 'var(--theme-border)' }}
        >
          {t('Read Archive')}
        </button>
        <button
          type="button"
          onClick={handleDelete}
          className="rounded border px-3 py-1.5 text-sm text-red-500 border-red-500"
        >
          {t('Delete Archive')}
        </button>
        <button
          type="button"
          onClick={() => navigate('/')}
          className="rounded border px-3 py-1.5 text-sm"
          style={{ borderColor: 'var(--theme-border)' }}
        >
          {t('Return to Library')}
        </button>
      </div>
    </div>
  )
}
