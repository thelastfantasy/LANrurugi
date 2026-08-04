import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { fetchJson, sendJson } from '../../api/client'
import {
  useClearCompletedQueue,
  useDeleteSelectedQueue,
  useDownloadQueue,
  useJobs,
  useSettings,
  useStartSelectedQueue,
  useUpdateQueueItem,
} from '../../api/hooks'
import type { ArchiveMetadata, DownloadQueueItem, JobRecord, PluginInfo } from '../../api/types'
import CollapsibleSection from '../../components/CollapsibleSection'
import { fetchMetadataForItem,QueueItemRow } from './QueueItemRow'
import {
  findMatchingPlugin,
  LOCAL_UPLOAD_NAMESPACE,
  TOOLBAR_BUTTON_STYLE,
} from './shared'

/** Right-column panel: the persistent queue, grouped by `plugin_namespace` (one
 * `CollapsibleSection` per namespace present), with bulk Select All / Invert Selection / Start /
 * Clear Completed / Delete actions and per-item controls. A row is selectable while `queued` or
 * `error` — `error` is a startable state too, so a failed download has a way back to running again
 * once the underlying problem (e.g. missing login credentials) is fixed. */
export function DownloadQueuePanel({
  downloadPlugins,
  metadataPlugins,
}: {
  downloadPlugins: PluginInfo[] | undefined
  metadataPlugins: PluginInfo[] | undefined
}) {
  const { t } = useTranslation()
  const queue = useDownloadQueue()
  const jobs = useJobs()
  const settings = useSettings()
  const startSelected = useStartSelectedQueue()
  const deleteSelected = useDeleteSelectedQueue()
  const clearCompleted = useClearCompletedQueue()
  const updateForBatchMetadata = useUpdateQueueItem()
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const items = useMemo(() => queue.data ?? [], [queue.data])
  // Auto-select freshly-added queue items ("添加到队列" then straight to "开始" without having to
  // tick every checkbox). `seenRef` starts null — the first *non-empty* `items` snapshot is the
  // pre-existing queue and must NOT be selected (the empty first frame from the still-loading
  // query is deliberately skipped, or every item would look "fresh" the moment data arrives and
  // the whole queue would get auto-checked, inflating 开始/删除's counts); only ids appearing
  // after that snapshot (a new "添加到队列" click) get auto-checked.
  const seenRef = useRef<Set<string> | null>(null)
  useEffect(() => {
    let seen = seenRef.current
    if (seen === null) {
      if (items.length === 0) return
      seen = new Set(items.map((i) => i.id))
      seenRef.current = seen
      return
    }
    const fresh = items.filter((i) => !seen.has(i.id))
    if (fresh.length === 0) return
    for (const i of fresh) seen.add(i.id)
    setSelected((prev) => {
      const next = new Set(prev)
      for (const i of fresh) next.add(i.id)
      return next
    })
  }, [items])
  const jobById = useMemo(() => {
    const map = new Map<string, JobRecord>()
    for (const j of jobs.data ?? []) map.set(j.id, j)
    return map
  }, [jobs.data])

  const grouped = useMemo(() => {
    const map = new Map<string, DownloadQueueItem[]>()
    for (const item of items) {
      const list = map.get(item.plugin_namespace) ?? []
      list.push(item)
      map.set(item.plugin_namespace, list)
    }
    return map
  }, [items])

  // Auto-fetch-metadata-on-completion: watches for a linked job reaching `finished` on an item
  // with `auto_fetch_metadata: true`, then fires the metadata preview call. Ref-guarded so this
  // doesn't re-fire on every poll tick once triggered.
  const triggeredRef = useRef<Set<string>>(new Set())
  useEffect(() => {
    for (const item of items) {
      if (!item.auto_fetch_metadata || !item.job_id) continue
      if (triggeredRef.current.has(item.job_id)) continue
      const job = jobById.get(item.job_id)
      if (!job || job.state !== 'finished') continue
      const archiveIds = (job.result as { archive_ids?: string[] } | null)?.archive_ids
      const archiveId = archiveIds?.[0]
      if (!archiveId) continue
      const metadataPlugin = findMatchingPlugin(metadataPlugins, item.url)
      if (!metadataPlugin) continue
      triggeredRef.current.add(item.job_id)
      void (async () => {
        const result = await sendJson<{
          success: number
          data?: { tags?: string; title?: string; summary?: string }
        }>(
          'POST',
          `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&id=${encodeURIComponent(archiveId)}`,
        ).catch(() => null)
        // Merges into whatever tags the fresh archive already has (normally none yet) rather than
        // assuming it's empty.
        if (!result?.success || !result.data) return
        const { tags: newTags, title, summary } = result.data
        if (!newTags && !(title && (settings.data?.replacetitles ?? true)) && !summary) return
        const archive = await fetchJson<ArchiveMetadata>(`/archives/${archiveId}/metadata`).catch(() => null)
        const mergedTags = newTags
          ? Array.from(
              new Set(
                [...(archive?.tags.split(',') ?? []), ...newTags.split(',')].map((tg) => tg.trim()).filter(Boolean),
              ),
            ).join(', ')
          : undefined
        await sendJson(
          'PUT',
          `/archives/${archiveId}/metadata?${new URLSearchParams({
            ...(mergedTags !== undefined && { tags: mergedTags }),
            ...(title && (settings.data?.replacetitles ?? true) && { title }),
            ...(summary && { summary }),
          })}`,
        )
      })()
    }
  }, [items, jobById, metadataPlugins, settings.data?.replacetitles])

  if (items.length === 0) return null

  // `error`/`cancelled` are startable states too (retry reuses the same start/select flow as a
  // first attempt; see `start_one`'s own docs on the Rust side). `downloading`/`starting`/`done`
  // are never selectable.
  const selectableIds = items
    .filter((i) => i.state === 'queued' || i.state === 'error' || i.state === 'cancelled')
    .map((i) => i.id)
  // Matches the backend `clear_completed` handler's filter exactly — `error` is excluded since
  // it's still-actionable, not "completed".
  const cleared = items.filter((i) => i.state === 'done').length

  function selectAll() {
    setSelected(new Set(selectableIds))
  }

  function invertSelection() {
    setSelected((prev) => new Set(selectableIds.filter((id) => !prev.has(id))))
  }

  return (
    <div style={{ marginTop: 16, textAlign: 'left' }}>
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Download Queue')}
      </h2>

      {/* `.control-btn-group` carries no actual CSS from any theme — a plain unstyled class name —
          so `display: 'flex'` must be set explicitly here. `.stdbtn`'s theme `min-width: 150px`
          (5 buttons = 750px, doesn't fit most viewports) is overridden to `0` per-button via
          `TOOLBAR_BUTTON_STYLE` so all 5 fit on one row instead of wrapping. */}
      <div
        className="control-btn-group"
        style={{ display: 'flex', flexWrap: 'nowrap', justifyContent: 'center', gap: 4, marginBottom: 6 }}
      >
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selectableIds.length === 0}
          onClick={selectAll}
        >
          {t('Select All')}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selectableIds.length === 0}
          onClick={invertSelection}
        >
          {t('Invert Selection')}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selected.size === 0 || startSelected.isPending}
          onClick={async () => {
            const selectedIds = [...selected]
            await startSelected.mutateAsync(selectedIds)
            setSelected(new Set())
            // Same fire-alongside-the-download behavior as the single-row Start button's
            // `fetchMetadataOnStart` — fire-and-forget per item so one slow/failed fetch never
            // delays the others.
            for (const id of selectedIds) {
              const item = items.find((i) => i.id === id)
              if (!item) continue
              void fetchMetadataForItem(item, findMatchingPlugin(metadataPlugins, item.url), updateForBatchMetadata)
            }
          }}
        >
          {t('Start ({{n}})', { n: selected.size })}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={cleared === 0 || clearCompleted.isPending}
          onClick={() => void clearCompleted.mutateAsync()}
        >
          {t('Clear Completed')}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selected.size === 0 || deleteSelected.isPending}
          onClick={async () => {
            await deleteSelected.mutateAsync([...selected])
            setSelected(new Set())
          }}
        >
          {t('Delete ({{n}})', { n: selected.size })}
        </button>
      </div>

      <ul className="collapsible extensible with-right-caret" style={{ width: '100%' }}>
        {/* Local uploads always pinned to the top, ahead of every download-plugin group —
            `grouped`'s own iteration order otherwise just follows whichever namespace's first
            item happened to appear earliest in `items` (queue insertion order), which puts local
            uploads wherever they landed rather than somewhere a user can reliably expect. */}
        {[...grouped.entries()]
          .sort(([a], [b]) => {
            if (a === LOCAL_UPLOAD_NAMESPACE) return -1
            if (b === LOCAL_UPLOAD_NAMESPACE) return 1
            return 0
          })
          .map(([namespace, groupItems]) => {
            const isLocalUpload = namespace === LOCAL_UPLOAD_NAMESPACE
            const plugin = downloadPlugins?.find((p) => p.namespace === namespace)
            const groupTitle = isLocalUpload ? t('From your computer') : (plugin?.name ?? namespace)
            return (
              <CollapsibleSection
                key={namespace}
                icon={isLocalUpload ? 'fa-upload' : 'fa-cloud-download-alt'}
                title={`${groupTitle} (${groupItems.length})`}
                caretStyle="right-down"
                defaultOpen
              >
                {groupItems.map((item) => (
                  <QueueItemRow
                    key={item.id}
                    item={item}
                    job={item.job_id ? jobById.get(item.job_id) : undefined}
                    selected={selected.has(item.id)}
                    onToggleSelect={() => {
                      if (item.state !== 'queued' && item.state !== 'error' && item.state !== 'cancelled')
                        return
                      setSelected((prev) => {
                        const next = new Set(prev)
                        if (next.has(item.id)) next.delete(item.id)
                        else next.add(item.id)
                        return next
                      })
                    }}
                    metadataPlugin={findMatchingPlugin(metadataPlugins, item.url)}
                  />
                ))}
              </CollapsibleSection>
            )
          })}
      </ul>
    </div>
  )
}

