import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import {
  useAddToTankoubon,
  useDeleteTankoubon,
  useTankoubon,
  useUpdateTankoubon,
} from '../api/hooks'
import type { TankoubonMetadata } from '../api/types'
import { routes } from '../routes'
import { useDocumentTitle } from '../useDocumentTitle'

export default function TankoubonEdit() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { tankId = '' } = useParams<{ tankId: string }>()
  const tankoubon = useTankoubon(tankId)

  if (tankoubon.isLoading) {
    return (
      <div className="p-6" style={{ color: 'var(--theme-muted)' }}>
        {t('Loading library…')}
      </div>
    )
  }

  if (tankoubon.isError || !tankoubon.data) {
    return (
      <div className="p-6 flex flex-col gap-3">
        <p className="text-red-500">
          {t('Failed to load archives: {{error}}', { error: String(tankoubon.error) })}
        </p>
        <button type="button" onClick={() => navigate(routes.library())} className="self-start underline">
          {t('Return to Library')}
        </button>
      </div>
    )
  }

  // Keyed by tankId so navigating between two different tankoubons' edit pages remounts this
  // form with fresh initial state, rather than needing an effect to re-sync it.
  return <TankoubonForm key={tankId} tankId={tankId} tankoubon={tankoubon.data} />
}

function TankoubonForm({ tankId, tankoubon }: { tankId: string; tankoubon: TankoubonMetadata }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  // Matches this page's own real heading text below ("Editing %1 (Tankoubon)") — no legacy
  // equivalent to cross-check against (Tankoubon editing is additive to this rewrite), so this
  // just keeps the tab title and the on-page heading in sync with each other.
  useDocumentTitle(t('Editing %1 (Tankoubon)').replace('%1', tankoubon.name))
  const updateTankoubon = useUpdateTankoubon(tankId)
  const deleteTankoubon = useDeleteTankoubon()
  const addToTankoubon = useAddToTankoubon(tankId)

  const [name, setName] = useState(tankoubon.name)
  const [summary, setSummary] = useState(tankoubon.summary)
  const [tags, setTags] = useState(tankoubon.tags)
  const [archives, setArchives] = useState(tankoubon.archives)
  const [newArchiveId, setNewArchiveId] = useState('')

  async function handleSave() {
    await updateTankoubon.mutateAsync({ metadata: { name, summary, tags } })
  }

  function moveArchive(index: number, direction: -1 | 1) {
    const next = [...archives]
    const target = index + direction
    if (target < 0 || target >= next.length) return
    ;[next[index], next[target]] = [next[target], next[index]]
    setArchives(next)
    updateTankoubon.mutate({ archives: next })
  }

  function removeArchive(id: string) {
    const next = archives.filter((a) => a !== id)
    setArchives(next)
    updateTankoubon.mutate({ archives: next })
  }

  async function handleDelete() {
    await deleteTankoubon.mutateAsync(tankId)
    navigate(routes.library())
  }

  async function handleAddArchive() {
    if (!newArchiveId.trim()) return
    await addToTankoubon.mutateAsync(newArchiveId.trim())
    setArchives((prev) => [...prev, newArchiveId.trim()])
    setNewArchiveId('')
  }

  return (
    <div className="max-w-2xl mx-auto p-6 flex flex-col gap-4">
      <h1 className="text-2xl font-semibold">
        {t('Editing %1 (Tankoubon)').replace('%1', tankoubon.name)}
      </h1>

      <div className="flex flex-col gap-1">
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('Title:')}
        </label>
        <input
          value={name}
          onChange={(e) => setName(e.target.value)}
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
          {t('Tags')}
        </label>
        <textarea
          value={tags}
          onChange={(e) => setTags(e.target.value)}
          className="rounded border px-2 py-1 text-sm min-h-16"
          style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
        />
      </div>

      <div className="flex flex-col gap-2">
        <label className="text-xs" style={{ color: 'var(--theme-muted)' }}>
          {t('Archives:')}
        </label>
        <ul className="flex flex-col gap-1">
          {archives.map((archiveId, index) => (
            <li
              key={archiveId}
              className="rounded border px-2 py-1 flex items-center justify-between gap-2 text-sm"
              style={{ borderColor: 'var(--theme-border)' }}
            >
              <span className="truncate">{archiveId}</span>
              <div className="flex gap-1 shrink-0">
                <button
                  type="button"
                  disabled={index === 0}
                  onClick={() => moveArchive(index, -1)}
                  className="disabled:opacity-30"
                >
                  ↑
                </button>
                <button
                  type="button"
                  disabled={index === archives.length - 1}
                  onClick={() => moveArchive(index, 1)}
                  className="disabled:opacity-30"
                >
                  ↓
                </button>
                <button
                  type="button"
                  onClick={() => navigate(routes.edit(archiveId))}
                  className="underline"
                  style={{ color: 'var(--theme-muted)' }}
                >
                  {t('Edit')}
                </button>
                <button
                  type="button"
                  onClick={() => removeArchive(archiveId)}
                  className="text-red-500"
                  title={t('Remove from Tankoubon') ?? undefined}
                >
                  ✕
                </button>
              </div>
            </li>
          ))}
        </ul>

        <div className="flex gap-2">
          <input
            value={newArchiveId}
            onChange={(e) => setNewArchiveId(e.target.value)}
            placeholder={t('Archive ID (40-character long)') ?? undefined}
            className="flex-1 rounded border px-2 py-1 text-sm"
            style={{ borderColor: 'var(--theme-border)', backgroundColor: 'var(--theme-surface)' }}
          />
          <button
            type="button"
            onClick={handleAddArchive}
            className="rounded px-3 py-1.5 text-sm"
            style={{ backgroundColor: 'var(--theme-accent)', color: 'var(--theme-accent-fg)' }}
          >
            {t('Add')}
          </button>
        </div>
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
        {archives[0] && (
          <button
            type="button"
            onClick={() => navigate(routes.reader(archives[0]))}
            className="rounded border px-3 py-1.5 text-sm"
            style={{ borderColor: 'var(--theme-border)' }}
          >
            {t('Read Archive')}
          </button>
        )}
        <button
          type="button"
          onClick={handleDelete}
          className="rounded border px-3 py-1.5 text-sm text-red-500 border-red-500"
        >
          {t('Delete Tankoubon')}
        </button>
        <button
          type="button"
          onClick={() => navigate(routes.library())}
          className="rounded border px-3 py-1.5 text-sm"
          style={{ borderColor: 'var(--theme-border)' }}
        >
          {t('Return to Library')}
        </button>
      </div>
    </div>
  )
}
