import { useCallback, useEffect, useMemo, useRef, useState } from "react"
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
import { Tooltip } from "@/components/common-ui/Display"
import { CollapsibleSection } from "@/components/Display"

import { QueueItemRow } from "./QueueItemRow"
import {
  findPluginByDomain,
  LOCAL_UPLOAD_NAMESPACE,
  needsDecision,
  partitionByOrigin,
  TOOLBAR_BUTTON_STYLE,
} from "./shared"

/** Which half of a group's list a tab shows. */
type QueueTab = "manual" | "subscription"

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

  /** Which tab each group is showing. Absent = the default, `manual` — a person's own paste is what
   * they came to this page to watch, and a subscription's bulk arriving must not switch the view
   * under them. Kept per group so a choice made for one plugin doesn't follow them to another. */
  const [tabByGroup, setTabByGroup] = useState<Record<string, QueueTab>>({})

  /** The tabs a group shows, and the items behind each. A group with only one kind of item has
   * nothing to switch between, so it gets no tabs at all — the per-row "from a subscription" badge
   * already says where those came from. */
  const tabsFor = useCallback(
    (groupItems: DownloadQueueItem[]) => {
      const { manual, fromSubscriptions } = partitionByOrigin(groupItems)
      const tabs = manual.length > 0 && fromSubscriptions.length > 0
      return { manual, fromSubscriptions, tabs }
    },
    [],
  )

  const visibleItems = useMemo(() => {
    const out: DownloadQueueItem[] = []
    for (const [namespace, groupItems] of grouped) {
      const { manual, fromSubscriptions, tabs } = tabsFor(groupItems)
      if (!tabs) out.push(...groupItems)
      else out.push(...((tabByGroup[namespace] ?? "manual") === "manual" ? manual : fromSubscriptions))
    }
    return out
  }, [grouped, tabByGroup, tabsFor])

  const itemIds = useMemo(() => new Set(items.map((i) => i.id)), [items])
  // Excludes ids whose item has since moved out of a selectable state (e.g. an auto-selected
  // fresh upload that finished before the user unchecked it) — selection must track live state,
  // not just queue membership, or a "done" item can stay selected with no way to uncheck it since
  // its checkbox is disabled.
  const selectableItemIds = useMemo(
    () =>
      new Set(
        items.filter((i) => i.state === "queued" || i.state === "error" || i.state === "cancelled").map((i) => i.id),
      ),
    [items],
  )
  /** Ids the active tabs are showing — what every bulk button below is allowed to touch. */
  const visibleIds = useMemo(() => new Set(visibleItems.map((i) => i.id)), [visibleItems])

  // Scoped to the visible tab: with a group split, the items behind the other tab are still in
  // `selected` (switching back finds them as they were) but they are not part of what Start/Delete
  // act on, and they do not count towards the buttons' own numbers.
  const effectiveSelected = useMemo(
    () =>
      new Set(
        [...selected].filter(
          (id) => itemIds.has(id) && selectableItemIds.has(id) && visibleIds.has(id),
        ),
      ),
    [selected, itemIds, selectableItemIds, visibleIds],
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

  // The bulk buttons act on what is on screen: with a group split into tabs, "select all" meaning
  // "every item in the queue including the ones you cannot see" would be a trap.
  const selectableIds = [...selectableItemIds].filter((id) => visibleIds.has(id))
  // "Completed" means the visible tab's completed items: the queue-wide endpoint would clear ones
  // sitting behind the other tab, which the user cannot see and did not ask to lose.
  const visibleDoneIds = visibleItems.filter((i) => i.state === "done").map((i) => i.id)
  const doneEverywhere = items.filter((i) => i.state === "done").length

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
          disabled={visibleDoneIds.length === 0 || clearCompleted.isPending || deleteSelected.isPending}
          onClick={async () => {
            // The queue-wide endpoint still handles the unscoped case, so its own activity record
            // ("clear completed") is what a user who sees the whole queue generates as before.
            if (visibleDoneIds.length === doneEverywhere) await clearCompleted.mutateAsync()
            else await deleteSelected.mutateAsync(visibleDoneIds)
          }}
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

      <ul className="collapsible extensible with-right-caret queue-groups" style={{ width: "100%" }}>
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
            const { manual, fromSubscriptions, tabs } = tabsFor(groupItems)
            const tab: QueueTab = tabByGroup[namespace] ?? "manual"
            const shown = tabs ? (tab === "manual" ? manual : fromSubscriptions) : groupItems
            // Splitting a list must not bury the rows that need an answer, so whichever tab holds
            // them says how many rather than leaving them one click away and unannounced.
            const undecided: Record<QueueTab, number> = {
              manual: manual.filter(needsDecision).length,
              subscription: fromSubscriptions.filter(needsDecision).length,
            }
            return (
              <CollapsibleSection
                key={namespace}
                icon={isLocalUpload ? "fa-upload" : "fa-cloud-download-alt"}
                title={
                  <span className="queue-group-title">
                    {`${groupTitle} (${groupItems.length})`}
                    {tabs && (
                      // The heading itself toggles the section; a tab click must not also do that.
                      <span className="queue-group-tabs" onClick={(e) => e.stopPropagation()}>
                        {(["manual", "subscription"] as const).map((which) => (
                          <button
                            key={which}
                            type="button"
                            className={`stdbtn queue-tab${tab === which ? " queue-tab-active" : ""}`}
                            aria-pressed={tab === which}
                            onClick={() => setTabByGroup((prev) => ({ ...prev, [namespace]: which }))}
                          >
                            {which === "manual" ? t("upload.tabManual") : t("upload.tabSubscription")}{" "}
                            {which === "manual" ? manual.length : fromSubscriptions.length}
                            {undecided[which] > 0 && (
                              <Tooltip label={t("upload.tabUndecided", { count: undecided[which] }) ?? ""}>
                                <span style={{ color: "#c79121", marginLeft: 4 }}>
                                  <i className="fa fa-exclamation-circle" aria-hidden="true"></i>{" "}
                                  {undecided[which]}
                                </span>
                              </Tooltip>
                            )}
                          </button>
                        ))}
                      </span>
                    )}
                  </span>
                }
                caretStyle="right-down"
                defaultOpen
              >
                {shown.map((item) => (
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

