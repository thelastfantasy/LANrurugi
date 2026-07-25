import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { fetchJson, sendJson } from '../../api/client'
import {
  useClearCompletedQueue,
  useDeleteQueueItem,
  useDeleteSelectedQueue,
  useDownloadQueue,
  useJobs,
  useOverwriteQueueItem,
  useRenameQueueItem,
  useSettings,
  useStartQueueItem,
  useStartSelectedQueue,
  useStopQueueItem,
  useUpdateQueueItem,
} from '../../api/hooks'
import type { ArchiveMetadata, DownloadQueueItem, JobRecord, PluginInfo } from '../../api/types'
import CollapsibleSection from '../../components/CollapsibleSection'
import { formatBytes, JobProgressBar, STATE_COLOR } from '../../components/JobProgress'
import QueueErrorText from '../../components/QueueErrorText'
import Tooltip from '../../components/Tooltip'
import { routes } from '../../routes'
import { FONT_SIZE_8PT, FONT_SIZE_10PT, Z_OVERLAY_BACKDROP } from '../../theme'
import { ConflictMenu, RenamePopover } from './FilenameTemplateEditor'
import {
  findMatchingPlugin,
  ICON_BUTTON_STYLE,
  LOCAL_UPLOAD_NAMESPACE,
  TOOLBAR_BUTTON_STYLE,
  TooltipIfPresent,
  TruncatedFilename,
} from './shared'

/** Fetches `metadataPlugin`'s preview for `item.url` and persists it onto the queue item (title +
 * `metadata_preview`). Module-level so both the single-row Start button and the batch "Start (N)"
 * button can share it — a batch start goes through `DownloadQueuePanel`, which has no per-row
 * `item`/`metadataPlugin` closure to reach into. */
export async function fetchMetadataForItem(
  item: DownloadQueueItem,
  metadataPlugin: PluginInfo | null,
  update: ReturnType<typeof useUpdateQueueItem>,
) {
  if (!metadataPlugin || item.metadata_preview) return
  const result = await sendJson<{ success: number; data?: Record<string, unknown> }>(
    'POST',
    `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&arg=${encodeURIComponent(item.url)}`,
  ).catch(() => null)
  const data = result?.data
  const title = typeof data?.title === 'string' ? data.title : undefined
  if (data) await update.mutateAsync({ id: item.id, title, metadata_preview: data })
}

/** Right-column panel: the persistent queue, grouped by `plugin_namespace` (one
 * `CollapsibleSection` per namespace present), with bulk Select All / Invert Selection / Start /
 * Clear Completed / Delete actions and per-item controls. A row is selectable while `queued` or
 * `error` — `error` is a startable state too, so a failed download has a way back to running again
 * once the underlying problem (e.g. missing login credentials) is fixed. */
export default function DownloadQueuePanel({
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

function QueueItemRow({
  item,
  job,
  selected,
  onToggleSelect,
  metadataPlugin,
}: {
  item: DownloadQueueItem
  job: JobRecord | undefined
  selected: boolean
  onToggleSelect: () => void
  metadataPlugin: PluginInfo | null
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const update = useUpdateQueueItem()
  const start = useStartQueueItem()
  const stop = useStopQueueItem()
  const del = useDeleteQueueItem()
  const overwriteConflict = useOverwriteQueueItem()
  const renameConflict = useRenameQueueItem()
  // Captured at click time (event-handler read, not render-time `ref.current` — forbidden by
  // `react-hooks/refs`). `null` means closed.
  const [conflictMenuAnchor, setConflictMenuAnchor] = useState<DOMRect | null>(null)
  const [renamePopover, setRenamePopover] = useState<{ x: number; y: number } | null>(null)
  const [fetchingMetadata, setFetchingMetadata] = useState(false)
  // Makes a `done` item's title clickable through to the archive it became. `item.archive_ids`
  // (persisted on the queue item itself) is preferred over the linked job's own result, since
  // `JobRegistry` is purely in-process memory and is lost on every server restart — falls back to
  // the job result only for an item that finished before this field existed.
  const archiveId = item.archive_ids?.[0] ?? (job?.result as { archive_ids?: string[] } | null)?.archive_ids?.[0]
  // `cancelled` is a persisted queue state — `useStopQueueItem`'s optimistic `onMutate` sets it in
  // the cache the instant Stop is clicked, before the request resolves.
  const wasCancelled = item.state === 'cancelled'
  // A local upload has no plugin to fetch metadata from, no `auto_fetch_metadata`/
  // `overwrite_on_duplicate` toggle of its own, and — since it's a synchronous, already-finished
  // operation by the time its queue item even exists — never passes through `queued`/`starting`/
  // `downloading`, so it has nothing to Start/Stop either. All of that UI is hidden outright
  // rather than left to fall back on state-mismatch `disabled` alone (true, but leaves dead
  // controls sitting in the row).
  const isLocalUpload = item.plugin_namespace === LOCAL_UPLOAD_NAMESPACE
  // Persisted at creation time for a local upload (`item.file_size`); a download instead reports
  // its size live through the linked job (`job.total_bytes`), which only becomes known partway
  // through the transfer and isn't persisted onto the queue item itself.
  const fileSize = item.file_size ?? job?.total_bytes

  async function handleFetchMetadata() {
    if (!metadataPlugin) return
    setFetchingMetadata(true)
    try {
      await fetchMetadataForItem(item, metadataPlugin, update)
    } finally {
      setFetchingMetadata(false)
    }
  }

  // Fired alongside Start/Retry, not awaited, so the row's title/tooltip fill in with real
  // metadata while the transfer is still in progress. No-op when there's no metadata plugin, or
  // metadata was already fetched.
  function fetchMetadataOnStart() {
    if (!metadataPlugin || item.metadata_preview || fetchingMetadata) return
    void handleFetchMetadata()
  }

  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 4,
        padding: '4px 2px',
        borderTop: '1px solid rgba(128,128,128,0.2)',
        flexWrap: 'wrap',
      }}
    >
      <input
        type="checkbox"
        checked={selected}
        disabled={item.state !== 'queued' && item.state !== 'error' && item.state !== 'cancelled'}
        onChange={onToggleSelect}
      />

      <TooltipIfPresent preview={item.metadata_preview} url={item.url} wrapperStyle={{ flex: '1 1 180px', minWidth: 0 }}>
        <div
          style={{
            width: '100%',
            boxSizing: 'border-box',
            border: '1px solid rgba(128,128,128,0.3)',
            borderRadius: 4,
            padding: '2px 6px',
          }}
        >
          {item.state === 'downloading' || item.state === 'starting' ? (
            <>
              {/* Title/URL stays visible above the progress bar rather than being fully replaced
                  by `JobProgressBar` (which only renders byte-progress) — keeps the row's identity
                  visible and the `TooltipIfPresent` trigger text in the DOM. */}
              <span style={{ fontSize: FONT_SIZE_10PT, wordBreak: 'break-all', display: 'block' }} title={item.metadata_preview ? undefined : item.url}>
                {item.title ?? item.url}
              </span>
              {job ? (
                <JobProgressBar job={job} />
              ) : (
                <span style={{ fontSize: FONT_SIZE_10PT }}>{t('Starting…')}</span>
              )}
            </>
          ) : item.state === 'done' ? (
            // A full green bar with the title overlaid (not `JobProgressBar`, whose byte-count/
            // speed detail line is just noise once a download is finished).
            <div style={{ position: 'relative', height: 18, borderRadius: 4, overflow: 'hidden', background: STATE_COLOR.active }}>
              {/* `archiveId` can be absent (queue item + job result both not yet available, or a
                  `file_path` fallback with no `archive_ids` at all) — stays plain text rather
                  than a dead link. */}
              <a
                href={archiveId ? routes.reader(archiveId) : undefined}
                onClick={(e) => {
                  if (!archiveId) return
                  e.preventDefault()
                  navigate(routes.reader(archiveId))
                }}
                style={{
                  position: 'absolute',
                  inset: 0,
                  display: 'flex',
                  alignItems: 'center',
                  padding: '0 6px',
                  fontSize: FONT_SIZE_10PT,
                  color: '#fff',
                  textShadow: '0 1px 2px rgba(0,0,0,0.6)',
                  cursor: archiveId ? 'pointer' : 'default',
                }}
              >
                {/* The name and the `(size)` suffix are two separate flex children (not one
                    plain text run) specifically so a long name truncates on its own without
                    also pushing the size suffix out of view — a single `text-overflow:
                    ellipsis` on the whole flex row would silently do nothing (that CSS property
                    only works on a single text-run box, not across flex children) and just let
                    the size get squeezed off/hidden past the row's edge instead. */}
                <TruncatedFilename
                  text={item.title ?? item.url}
                  isFilename={!item.title}
                  style={{ minWidth: 0, flexShrink: 1 }}
                />
                {fileSize != null && (
                  <span style={{ marginLeft: 6, opacity: 0.85, flexShrink: 0 }}>({formatBytes(fileSize)})</span>
                )}
              </a>
            </div>
          ) : (
            <span
              style={{ fontSize: FONT_SIZE_10PT, display: 'flex' }}
              title={item.metadata_preview ? undefined : item.url}
            >
              <TruncatedFilename text={item.title ?? item.url} isFilename={!item.title} />
            </span>
          )}
          {item.state === 'error' && item.error && (
            <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>
              <QueueErrorText error={item.error} />
            </div>
          )}
          {/* A stopped download gets its own real, persisted state (`cancelled`, distinct from
              `queued`) precisely so this label — and the Retry-button treatment below — survive a
              page refresh instead of only living in this mutation's own transient local state. */}
          {wasCancelled && (
            <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>{t('Cancelled')}</div>
          )}
        </div>
      </TooltipIfPresent>

      {!isLocalUpload && (
        <>
          <Tooltip label={t('Auto Fetch Metadata') ?? ''}>
            <input
              type="checkbox"
              checked={item.auto_fetch_metadata}
              disabled={item.state !== 'queued'}
              onChange={(e) => void update.mutateAsync({ id: item.id, auto_fetch_metadata: e.target.checked })}
            />
          </Tooltip>

          <Tooltip label={t('Overwrite Duplicate') ?? ''}>
            <input
              type="checkbox"
              checked={item.overwrite_on_duplicate}
              disabled={item.state !== 'queued'}
              onChange={(e) => void update.mutateAsync({ id: item.id, overwrite_on_duplicate: e.target.checked })}
            />
          </Tooltip>
        </>
      )}

      {item.pending_filename_conflict ? (
        // A `Filename` collision has a real choice (see `PendingFilenameConflict`'s own docs),
        // unlike a `ContentHash` collision (`QueueError::DuplicateArchive`, unconditionally
        // rejected — no buttons, just the plain error text below). Both resolutions share one
        // dropdown trigger button rather than two separate icon buttons.
        <>
          <Tooltip label={t('Resolve Conflict') ?? ''}>
            <button
              type="button"
              className="stdbtn"
              style={ICON_BUTTON_STYLE}
              disabled={overwriteConflict.isPending || renameConflict.isPending}
              onClick={(e) => {
                // `e.currentTarget` must be read synchronously here, not inside the updater
                // function below — React nulls it out by the time that callback runs.
                const rect = e.currentTarget.getBoundingClientRect()
                setConflictMenuAnchor((open) => (open ? null : rect))
              }}
            >
              <i className="fa fa-clone" aria-hidden="true"></i>
            </button>
          </Tooltip>
          {conflictMenuAnchor && (
            <>
              <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={() => setConflictMenuAnchor(null)} />
              <ConflictMenu
                anchor={conflictMenuAnchor}
                onOverwrite={() => {
                  setConflictMenuAnchor(null)
                  void overwriteConflict.mutateAsync(item.id)
                }}
                onRename={() => {
                  // Anchored off the trigger button's rect, not the (about-to-disappear) menu
                  // item that was clicked.
                  setRenamePopover({ x: conflictMenuAnchor.left, y: conflictMenuAnchor.bottom })
                  setConflictMenuAnchor(null)
                }}
              />
            </>
          )}
          {renamePopover && (
            <RenamePopover
              anchor={renamePopover}
              conflict={item.pending_filename_conflict}
              itemTitle={item.title}
              itemNamespace={item.plugin_namespace}
              pending={renameConflict.isPending}
              onCancel={() => setRenamePopover(null)}
              onConfirm={(filename) => {
                setRenamePopover(null)
                void renameConflict.mutateAsync({ id: item.id, filename })
              }}
            />
          )}
        </>
      ) : isLocalUpload ? null : item.state === 'starting' || item.state === 'downloading' ? (
        <Tooltip label={t('Stop') ?? ''}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={stop.isPending}
            onClick={() => void stop.mutateAsync(item.id)}
          >
            <i className="fa fa-stop" aria-hidden="true"></i>
          </button>
        </Tooltip>
      ) : (
        // `cancelled` is its own restartable state (see `wasCancelled`'s own docs) — treated as a
        // restart (Retry wording/icon), same as `error`, rather than a fresh download.
        <Tooltip label={(item.state === 'error' || wasCancelled ? t('Retry') : t('Download')) ?? ''}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={
              (item.state !== 'queued' && item.state !== 'error' && item.state !== 'cancelled') ||
              start.isPending
            }
            onClick={() => {
              void start.mutateAsync(item.id)
              fetchMetadataOnStart()
            }}
          >
            <i
              className={`fa ${item.state === 'error' || wasCancelled ? 'fa-redo' : 'fa-download'}`}
              aria-hidden="true"
            ></i>
          </button>
        </Tooltip>
      )}

      {/* A local upload has no plugin match possible in the first place (`item.url` is a
          filename, not a real URL — `findMatchingPlugin` never matches any `url_pattern` against
          it) — hidden outright rather than left as a permanently-disabled dead button. */}
      {!isLocalUpload && (
        <Tooltip
          label={
            metadataPlugin
              ? `${t('Fetch Metadata')} (${metadataPlugin.name})`
              : (t('Fetch Metadata') ?? '')
          }
        >
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={!metadataPlugin || item.state === 'done' || fetchingMetadata}
            onClick={() => void handleFetchMetadata()}
          >
            <i className={`fa ${fetchingMetadata ? 'fa-spinner fa-spin' : 'fa-tags'}`} aria-hidden="true"></i>
          </button>
        </Tooltip>
      )}

      {/* `DELETE /download_queue/{id}` only removes this queue-history row — it never touches the
          cataloged archive a `done` item became (that's the separate `DELETE /archives/{id}`).
          Relabeled "Remove" with a distinct icon for the `done` state so the wording doesn't imply
          deleting the archive. */}
      <Tooltip label={(item.state === 'done' ? t('Remove') : t('Delete')) ?? ''}>
        <button
          type="button"
          className="stdbtn"
          style={ICON_BUTTON_STYLE}
          // Deleting an in-flight item would rip its Redis entry out from under the still-running
          // download task with no way to stop the transfer. Must Stop first.
          disabled={
            del.isPending || item.state === 'starting' || item.state === 'downloading'
          }
          onClick={() => void del.mutateAsync(item.id)}
        >
          <i className={`fa ${item.state === 'done' ? 'fa-eraser' : 'fa-times'}`} aria-hidden="true"></i>
        </button>
      </Tooltip>
    </div>
  )
}
