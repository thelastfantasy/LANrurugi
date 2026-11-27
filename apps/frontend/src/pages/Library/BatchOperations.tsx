import { useQueryClient } from "@tanstack/react-query"
import type { TFunction } from "i18next"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError, fetchJson, sendJson, sleep } from "@/api/client"
import { useBatchDeleteArchives, useCategories, usePlugins, useSettings } from "@/api/hooks"
import type { ArchiveMetadata, TankoubonMetadata } from "@/api/types"
import { Input, NumberInput, RadioGroup, RadioItem, Select } from "@/components/common-ui/Form"
import { invalidateLibraryData } from "@/CrossTabSync"
import { choiceDialog, confirmDialog } from "@/dialog"
import { postDataChange } from "@/lib/crossTabData"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"
import { sortCategories } from "@/lib/utils/sortCategories"
import { findSourcePlugin } from "@/pages/Upload/shared"
import { Z_OVERLAY_ABOVE_DRAWER } from "@/theme"
import { toast } from "@/toast"

type ScrapeOutcome = "ok" | "failed" | "none"

type Operation = "plugin" | "clearnew" | "tagrules" | "addcat"

/** Legacy's own progress line uses a DOM-id template, not `{{}}` interpolation — substituted the
 * same way rather than adding a second, differently-shaped key across all 14 locales. */
function formatProgress(t: TFunction, done: number, total: number): string {
  return t('Processed <span id="arcs"></span> out of <span id="totalarcs"></span>')
    .replace('<span id="arcs"></span>', String(done))
    .replace('<span id="totalarcs"></span>', String(total))
}

/** Every field matches the panel's 36px buttons (`.batch-ops-start`/`.batch-ops-back`). */
const CONTROL = { width: "100%", height: 36, margin: 0, boxSizing: "border-box" } as const

/** Names what will actually happen: Tankoubons and archives counted apart, and a dissolved
 *  Tankoubon never described as deleted. */
function deleteMessage(t: TFunction, tanks: number, archives: number, tankMode: string | null): string {
  if (tanks === 0) return t("batch.confirmDeleteSelectedN", { n: archives })
  if (tankMode === "recycle") {
    return archives === 0
      ? t("batch.confirmRecycleTanksN", { tanks })
      : t("batch.confirmRecycleTanksAndArchives", { tanks, archives })
  }
  return t("batch.confirmDissolveTanksAndDeleteArchives", { tanks, archives })
}

const fetchArchive = (id: string) => fetchJson<ArchiveMetadata>(`/archives/${id}/metadata`)

export type BatchActions = ReturnType<typeof useBatchActions>

/** The batch operations over a selection (legacy's batch.html.tt2), run archive by archive.
 *  One instance per drawer, shared by its one-tap buttons and its batch view, so the two can never
 *  run at once. Tankoubon ids are skipped by the per-archive operations. */
export function useBatchActions(
  ids: string[],
  /** Ids the server really deleted, so the selection can drop them. */
  onRemoved: (ids: string[]) => void,
) {
  const { t } = useTranslation()
  const plugins = usePlugins("metadata")
  const settings = useSettings()
  const queryClient = useQueryClient()
  const batchDeleteArchives = useBatchDeleteArchives()
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)

  const archiveIds = ids.filter((id) => !isTankoubonId(id))

  function finish(changed: string[]) {
    invalidateLibraryData(queryClient)
    postDataChange({ changed })
  }

  /** Runs `step` over each archive id in turn, reporting progress as it goes. */
  async function eachArchive(step: (id: string, index: number) => Promise<void>) {
    setBusy(true)
    setStatus(formatProgress(t, 0, archiveIds.length))
    try {
      for (const [index, id] of archiveIds.entries()) {
        await step(id, index)
        setStatus(formatProgress(t, index + 1, archiveIds.length))
      }
      finish(archiveIds)
    } finally {
      setBusy(false)
    }
  }

  const replaceTitles = settings.data?.replacetitles ?? true

  /** Runs one plugin on one archive and writes its answer straight back (a batch has no review
   *  step, unlike the Edit page's staging-then-Save flow). Tags are merged, never replaced. */
  async function runPluginOn(id: string, plugin: string, arg = ""): Promise<boolean> {
    const result = await sendJson<{
      success: number
      data?: { tags?: string; title?: string; summary?: string }
    }>("POST", `/plugins/use?${new URLSearchParams({ plugin, id, arg })}`).catch(() => null)
    if (!result?.success || !result.data) return false
    const { tags: newTags, title, summary } = result.data
    if (newTags || (title && replaceTitles) || summary) {
      const archive = await fetchArchive(id)
      const mergedTags = newTags
        ? Array.from(
            new Set([...archive.tags.split(","), ...newTags.split(",")].map((tg) => tg.trim()).filter(Boolean)),
          ).join(", ")
        : undefined
      await fetch(
        `/api/archives/${id}/metadata?${new URLSearchParams({
          ...(mergedTags !== undefined && { tags: mergedTags }),
          ...(title && replaceTitles && { title }),
          ...(summary && { summary }),
        })}`,
        { method: "PUT" },
      )
    }
    return true
  }

  /** The Edit page's "auto scrape", per archive: the plugin its `source:` tag belongs to, else the
   *  enabled chain ingest runs. */
  async function autoScrape(id: string): Promise<ScrapeOutcome> {
    const archive = await fetchArchive(id).catch(() => null)
    if (!archive) return "failed"
    const source = findSourcePlugin(plugins.data, archive.tags)
    if (source) {
      const ok = await runPluginOn(id, source.plugin.namespace, source.plugin.oneshot_arg ? source.url : "")
      return ok ? "ok" : "failed"
    }
    const result = await sendJson<{ successes: number; failures: number }>(
      "POST",
      `/plugins/auto?${new URLSearchParams({ id })}`,
    ).catch(() => null)
    if (!result) return "failed"
    if (result.successes > 0) return "ok"
    return result.failures > 0 ? "failed" : "none"
  }

  /** `plugin` empty means auto scrape. */
  async function scrape(plugin = "", interval = 0) {
    const tally: Record<ScrapeOutcome, number> = { ok: 0, failed: 0, none: 0 }
    await eachArchive(async (id, index) => {
      const outcome = !plugin ? await autoScrape(id) : (await runPluginOn(id, plugin)) ? "ok" : "failed"
      tally[outcome] += 1
      if (interval > 0 && index + 1 < archiveIds.length) await sleep(interval * 1000)
    })
    const summary = t("batch.scrapeSummary", { ok: tally.ok, failed: tally.failed, none: tally.none })
    setStatus(summary)
    toast({ text: summary, icon: tally.failed > 0 ? "warning" : "success" })
  }

  const clearNewFlag = () =>
    eachArchive(async (id) => {
      await fetch(`/api/archives/${id}/isnew`, { method: "DELETE" })
    })

  const addTag = (tag: string) =>
    eachArchive(async (id) => {
      const archive = await fetchArchive(id)
      const tags = [archive.tags, tag].filter(Boolean).join(", ")
      await fetch(`/api/archives/${id}/metadata?${new URLSearchParams({ tags })}`, { method: "PUT" })
    })

  const addToCategory = (categoryId: string) =>
    eachArchive(async (id) => {
      await fetch(`/api/categories/${categoryId}/${id}`, { method: "PUT" })
    })

  /** Archives go through the batch endpoint; Tankoubons are not archives, so each is deleted
   *  the way the grid deletes one — dissolved or recycled whole, as the user picks. */
  async function deleteSelected() {
    const tankIds = ids.filter(isTankoubonId)
    let tankMode: string | null = null
    if (tankIds.length > 0) {
      tankMode = await choiceDialog(t("batch.tankDeleteChoice", { n: tankIds.length }), [
        { value: "dissolve", label: t("library.dissolveTankoubonOnly") },
        { value: "recycle", label: t("library.recycleWholeTankoubon"), danger: true },
      ])
      if (tankMode === null) return
    }
    // Only dissolving Tankoubons puts nothing in the recycle bin; the choice above was the confirmation.
    const recycles = archiveIds.length > 0 || tankMode === "recycle"
    if (recycles && !(await confirmDialog(deleteMessage(t, tankIds.length, archiveIds.length, tankMode), true, true))) {
      return
    }
    setBusy(true)
    setStatus(null)
    const removed: string[] = []
    const failed: string[] = []
    try {
      // Tankoubons first: recycling one takes its members along, and dissolving one frees its
      // members, which the batch endpoint otherwise refuses to delete.
      const takenAlong = new Set<string>()
      for (const tankId of tankIds) {
        const members =
          tankMode === "recycle"
            ? ((await fetchJson<TankoubonMetadata>(`/tankoubons/${tankId}`).catch(() => null))?.archives ?? [])
            : []
        const query = tankMode === "recycle" ? "?mode=recycle" : ""
        const response = await fetch(`/api/tankoubons/${tankId}${query}`, { method: "DELETE" }).catch(() => null)
        if (response?.ok) {
          removed.push(tankId)
          for (const member of members) takenAlong.add(member)
        } else {
          failed.push(tankId)
        }
      }
      for (const member of takenAlong) if (archiveIds.includes(member)) removed.push(member)
      const rest = archiveIds.filter((id) => !takenAlong.has(id))
      if (rest.length > 0) {
        const response = await batchDeleteArchives.mutateAsync(rest)
        for (const r of response.results) {
          if (r.success) removed.push(r.id)
          else failed.push(r.error ? `${r.id}: ${r.error}` : r.id)
        }
      }
      if (failed.length === 0) {
        toast({ text: formatProgress(t, removed.length, ids.length), icon: "success" })
      } else {
        const message = t("batch.deletedNOfMFailed", { deleted: removed.length, total: ids.length })
        setStatus(message ?? null)
        // "warning", not "error": the requests themselves went through with a real result each.
        toast({ heading: message ?? undefined, text: failed.join("\n"), icon: "warning" })
      }
    } catch (err) {
      toast({
        heading: t("batch.deleteRequestFailed") ?? undefined,
        text: err instanceof ApiError ? err.message : String(err),
        icon: "error",
      })
    } finally {
      postDataChange({ removed })
      invalidateLibraryData(queryClient)
      void queryClient.invalidateQueries({ queryKey: ["recycle-bin"] })
      onRemoved(removed)
      setBusy(false)
    }
  }

  return {
    busy,
    status,
    archiveCount: archiveIds.length,
    plugins: plugins.data,
    scrape,
    clearNewFlag,
    addTag,
    addToCategory,
    deleteSelected,
  }
}

/** The drawer's batch view: the operations that need settings first. Auto scrape, sharing and
 *  deleting are also one-tap buttons on the drawer's first level. */
export function BatchOperations({ actions }: { actions: BatchActions }) {
  const { t } = useTranslation()
  const categories = useCategories()
  const { busy, status, archiveCount } = actions

  const [operation, setOperation] = useState<Operation>("plugin")
  const [tagToAdd, setTagToAdd] = useState("")
  const [categoryTarget, setCategoryTarget] = useState("")
  const [pluginNamespace, setPluginNamespace] = useState("")
  const [scrapeMode, setScrapeMode] = useState<"auto" | "plugin">("auto")
  const [pluginInterval, setPluginInterval] = useState(0)

  const ready =
    archiveCount > 0 &&
    (operation === "plugin"
      ? scrapeMode === "auto" || Boolean(pluginNamespace)
      : operation === "tagrules"
        ? Boolean(tagToAdd.trim())
        : operation === "addcat"
          ? Boolean(categoryTarget)
          : true)

  async function run() {
    if (operation === "plugin") return actions.scrape(scrapeMode === "auto" ? "" : pluginNamespace, pluginInterval)
    if (operation === "clearnew") return actions.clearNewFlag()
    if (operation === "addcat") return actions.addToCategory(categoryTarget)
    await actions.addTag(tagToAdd)
    setTagToAdd("")
  }

  const operations: { value: Operation; label: string }[] = [
    { value: "plugin", label: `🧩 ${t("batch.scrapeMetadata")}` },
    { value: "clearnew", label: `🆕 ${t("batch.removeNewFlag")}` },
    { value: "tagrules", label: `📏 ${t("batch.applyTagRules")}` },
    { value: "addcat", label: `📚 ${t("batch.addToCategory")}` },
  ]

  return (
    <div className="batch-ops-panel">
      <Select
        value={operation}
        onValueChange={setOperation}
        items={operations}
        ariaLabel={t("batch.chooseOperation") ?? undefined}
        align="start"
        popupZIndex={Z_OVERLAY_ABOVE_DRAWER}
        style={CONTROL}
        disabled={busy}
      />

      {operation === "plugin" && (
        <div className="batch-ops-grid">
          <span>{t("batch.scrapeModeLabel")}</span>
          <RadioGroup value={scrapeMode} onValueChange={setScrapeMode} className="flex flex-wrap gap-x-4 gap-y-1">
            <RadioItem value="auto">{t("batch.scrapeModeAuto")}</RadioItem>
            <RadioItem value="plugin">{t("batch.scrapeModePlugin")}</RadioItem>
          </RadioGroup>
          {scrapeMode === "auto" ? (
            <p className="batch-ops-hint">{t("batch.scrapeModeAutoHelp")}</p>
          ) : (
            <>
              <span>{t("batch.usePluginLabel")}</span>
              <Select
                value={pluginNamespace}
                onValueChange={setPluginNamespace}
                items={[
                  { value: "", label: "—" },
                  ...(actions.plugins ?? []).map((p) => ({ value: p.namespace, label: p.name })),
                ]}
                align="start"
                popupZIndex={Z_OVERLAY_ABOVE_DRAWER}
                style={CONTROL}
              />
            </>
          )}
          <span>{t("batch.intervalBetween")}</span>
          <span className="inline-flex items-center gap-2">
            <NumberInput
              min={0}
              max={20}
              value={pluginInterval}
              onValueChange={(v) => setPluginInterval(Math.min(20, Math.max(0, v)))}
              style={{ width: 90 }}
              inputStyle={{ height: CONTROL.height, margin: 0 }}
            />
            {t("batch.seconds")}
          </span>
        </div>
      )}
      {operation === "clearnew" && <p className="batch-ops-note">{t("batch.thisRemovesTheNewFlag")}</p>}
      {operation === "tagrules" && (
        <Input
          value={tagToAdd}
          onValueChange={(v) => setTagToAdd(v)}
          placeholder={t("common.tags") ?? undefined}
          style={CONTROL}
        />
      )}
      {operation === "addcat" && (
        <Select
          value={categoryTarget}
          onValueChange={setCategoryTarget}
          items={[
            { value: "", label: t("common.NoCategory") },
            ...sortCategories(categories.data ?? []).map((c) => ({ value: c.id, label: c.name })),
          ]}
          align="start"
          popupZIndex={Z_OVERLAY_ABOVE_DRAWER}
          style={CONTROL}
        />
      )}

      <button type="button" className="stdbtn batch-ops-start" disabled={busy || !ready} onClick={() => void run()}>
        {busy && <i className="fa fa-cog fa-spin" aria-hidden="true" />}
        {t("batch.startTask")}
      </button>
      {status && <p className="batch-ops-note">{status}</p>}
    </div>
  )
}
