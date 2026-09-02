import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { fetchJson, sendJson } from "@/api/client"
import {
  useClearCompletedQueue,
  useDeleteSelectedQueue,
  useDownloadQueue,
  useJobs,
  useSettings,
  useStartSelectedQueue,
} from "@/api/hooks"
import type { ArchiveMetadata, DownloadQueueItem, JobRecord, PluginInfo } from "@/api/types"
import { CollapsibleSection } from "@/components/Display"

import { QueueItemRow } from "./QueueItemRow"
import {
  findPluginByDomain,
  LOCAL_UPLOAD_NAMESPACE,
  TOOLBAR_BUTTON_STYLE,
} from "./shared"

/** Right-column panel: the persistent queue, grouped by `plugin_namespace`, with bulk
 * Select All / Invert Selection / Start / Clear Completed / Delete actions. */
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
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const items = useMemo(() => queue.data ?? [], [queue.data])
  // Auto-select freshly-added queue items. seenRef starts null: the first non-empty snapshot is
  // the pre-existing queue and must NOT be selected, or the whole queue gets auto-checked once.
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

  const itemIds = useMemo(() => new Set(items.map((i) => i.id)), [items])
  const effectiveSelected = useMemo(
    () => new Set([...selected].filter((id) => itemIds.has(id))),
    [selected, itemIds],
  )

  const triggeredRef = useRef<Set<string>>(new Set())
  useEffect(() => {
    for (const item of items) {
      if (!item.auto_fetch_metadata || !item.job_id) continue
      if (triggeredRef.current.has(item.job_id)) continue
      const job = jobById.get(item.job_id)
      if (!job || job.state !== "finished") continue
      const archiveIds = (job.result as { archive_ids?: string[] } | null)?.archive_ids
      const archiveId = archiveIds?.[0]
      if (!archiveId) continue
      const metadataPlugin = findPluginByDomain(metadataPlugins, item.url)
      if (!metadataPlugin) continue
      triggeredRef.current.add(item.job_id)
      void (async () => {
        const result = await sendJson<{
          success: number
          data?: { tags?: string; title?: string; summary?: string }
        }>(
          "POST",
          `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&id=${encodeURIComponent(archiveId)}`,
        ).catch(() => null)
        if (!result?.success || !result.data) return
        const { tags: newTags, title, summary } = result.data
        if (!newTags && !(title && (settings.data?.replacetitles ?? true)) && !summary) return
        const archive = await fetchJson<ArchiveMetadata>(`/archives/${archiveId}/metadata`).catch(() => null)
        const mergedTags = newTags
          ? Array.from(
              new Set(
                [...(archive?.tags.split(",") ?? []), ...newTags.split(",")].map((tg) => tg.trim()).filter(Boolean),
              ),
            ).join(", ")
          : undefined
        await sendJson(
          "PUT",
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

  const selectableIds = items
    .filter((i) => i.state === "queued" || i.state === "error" || i.state === "cancelled")
    .map((i) => i.id)
  const cleared = items.filter((i) => i.state === "done").length

  function selectAll() {
    setSelected(new Set(selectableIds))
  }

  function invertSelection() {
    setSelected((prev) => new Set(selectableIds.filter((id) => !prev.has(id))))
  }

  return (
    <div style={{ marginTop: 16, textAlign: "left" }}>
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("upload.downloadQueue")}
      </h2>

      <div
        className="control-btn-group"
        style={{ display: "flex", flexWrap: "nowrap", justifyContent: "center", gap: 4, marginBottom: 6 }}
      >
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selectableIds.length === 0}
          onClick={selectAll}
        >
          {t("upload.selectAll")}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={selectableIds.length === 0}
          onClick={invertSelection}
        >
          {t("upload.invertSelection")}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={effectiveSelected.size === 0 || startSelected.isPending}
          onClick={async () => {
            const selectedIds = [...effectiveSelected]
            await startSelected.mutateAsync(selectedIds)
            setSelected(new Set())
          }}
        >
          {t("upload.startN", { n: effectiveSelected.size })}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={cleared === 0 || clearCompleted.isPending}
          onClick={() => void clearCompleted.mutateAsync()}
        >
          {t("upload.clearCompleted")}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={TOOLBAR_BUTTON_STYLE}
          disabled={effectiveSelected.size === 0 || deleteSelected.isPending}
          onClick={async () => {
            await deleteSelected.mutateAsync([...effectiveSelected])
            setSelected(new Set())
          }}
        >
          {t("upload.deleteN", { n: effectiveSelected.size })}
        </button>
      </div>

      <ul className="collapsible extensible with-right-caret" style={{ width: "100%" }}>
        {[...grouped.entries()]
          .sort(([a], [b]) => {
            if (a === LOCAL_UPLOAD_NAMESPACE) return -1
            if (b === LOCAL_UPLOAD_NAMESPACE) return 1
            return 0
          })
          .map(([namespace, groupItems]) => {
            const isLocalUpload = namespace === LOCAL_UPLOAD_NAMESPACE
            const plugin = downloadPlugins?.find((p) => p.namespace === namespace)
            const groupTitle = isLocalUpload ? t("upload.fromYourComputer") : (plugin?.name ?? namespace)
            return (
              <CollapsibleSection
                key={namespace}
                icon={isLocalUpload ? "fa-upload" : "fa-cloud-download-alt"}
                title={`${groupTitle} (${groupItems.length})`}
                caretStyle="right-down"
                defaultOpen
              >
                {groupItems.map((item) => (
                  <QueueItemRow
                    key={item.id}
                    item={item}
                    job={item.job_id ? jobById.get(item.job_id) : undefined}
                    selected={effectiveSelected.has(item.id)}
                    onToggleSelect={() => {
                      if (item.state !== "queued" && item.state !== "error" && item.state !== "cancelled")
                        return
                      setSelected((prev) => {
                        const next = new Set(prev)
                        if (next.has(item.id)) next.delete(item.id)
                        else next.add(item.id)
                        return next
                      })
                    }}
                    metadataPlugin={findPluginByDomain(metadataPlugins, item.url)}
                  />
                ))}
              </CollapsibleSection>
            )
          })}
      </ul>
    </div>
  )
}

