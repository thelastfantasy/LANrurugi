import { useQueryClient } from "@tanstack/react-query"
import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { ApiError, sendJson, sleep } from "@/api/client"
import { useArchives, useBatchDeleteArchives, useCategories, usePlugins, useSettings } from "@/api/hooks"
import type { ArchiveMetadata } from "@/api/types"
import { ArchiveChecklistItem } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { MSM_SELECTION_KEY } from "@/lib/storageKeys"
import { sortCategories } from "@/lib/utils/sortCategories"
import { useApplyTheme } from "@/theme"
import { toast } from "@/toast"

async function fetchArchive(id: string): Promise<ArchiveMetadata> {
  const response = await fetch(`/api/archives/${id}/metadata`)
  return (await response.json()) as ArchiveMetadata
}

type Operation = "plugin" | "clearnew" | "tagrules" | "addcat" | "delete"

/** Legacy's own progress line uses a DOM-id template, not `{{}}` interpolation — substituted the
 * same way rather than adding a second, differently-shaped key across all 14 locales. */
function formatProgress(t: (key: string) => string, done: number, total: number): string {
  return t('Processed <span id="arcs"></span> out of <span id="totalarcs"></span>')
    .replace('<span id="arcs"></span>', String(done))
    .replace('<span id="totalarcs"></span>', String(total))
}

/** A premade selection handed off from the Library page's "Run Batch Operations" action — read
 * once as the initial `selected` set and immediately cleared. `TANK_` IDs are left unexpanded. */
function takePremadeSelection(): string[] {
  const raw = localStorage.getItem(MSM_SELECTION_KEY)
  localStorage.removeItem(MSM_SELECTION_KEY)
  if (!raw) return []
  try {
    const ids = JSON.parse(raw) as unknown
    return Array.isArray(ids) ? ids.filter((id) => typeof id === "string") : []
  } catch {
    return []
  }
}

/** Mirrors legacy's batch.html.tt2 — task select + one operation row / archive checklist; runs
 * each operation archive-by-archive synchronously, no job-progress polling. */
export function Batch() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const archives = useArchives()
  const categories = useCategories()
  const plugins = usePlugins("metadata")
  const settings = useSettings()
  const queryClient = useQueryClient()
  const batchDeleteArchives = useBatchDeleteArchives()

  const [operation, setOperation] = useState<Operation>("plugin")
  const [selected, setSelected] = useState<Set<string>>(() => new Set(takePremadeSelection()))
  const [tagToAdd, setTagToAdd] = useState("")
  const [categoryTarget, setCategoryTarget] = useState("")
  const [pluginNamespace, setPluginNamespace] = useState("")
  const [pluginTimeout, setPluginTimeout] = useState(0)
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  useApplyTheme()
  useDocumentTitle(t("batch.batchOperations") ?? undefined)

  function toggle(id: string) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  async function refresh() {
    await queryClient.invalidateQueries({ queryKey: ["archives"] })
    await queryClient.invalidateQueries({ queryKey: ["categories"] })
  }

  async function applyPlugin() {
    if (!pluginNamespace || selected.size === 0) return
    setBusy(true)
    setStatus(null)
    try {
      let processed = 0
      for (const id of selected) {
        const result = await sendJson<{
          success: number
          data?: { tags?: string; title?: string; summary?: string }
        }>("POST", `/plugins/use?${new URLSearchParams({ plugin: pluginNamespace, id })}`).catch(
          () => null,
        )
        // Applies the plugin's result directly, unlike the single-archive Edit page's
        // staging-then-Save flow — a batch run has no per-archive review step.
        if (result?.success && result.data) {
          const { tags: newTags, title, summary } = result.data
          if (newTags || (title && (settings.data?.replacetitles ?? true)) || summary) {
            const archive = await fetchArchive(id)
            const mergedTags = newTags
              ? Array.from(new Set([...archive.tags.split(","), ...newTags.split(",")].map((tg) => tg.trim()).filter(Boolean))).join(", ")
              : undefined
            await fetch(
              `/api/archives/${id}/metadata?${new URLSearchParams({
                ...(mergedTags !== undefined && { tags: mergedTags }),
                ...(title && (settings.data?.replacetitles ?? true) && { title }),
                ...(summary && { summary }),
              })}`,
              { method: "PUT" },
            )
          }
        }
        processed += 1
        setStatus(formatProgress(t, processed, selected.size))
        if (pluginTimeout > 0 && processed < selected.size) await sleep(pluginTimeout * 1000)
      }
      await refresh()
    } finally {
      setBusy(false)
    }
  }

  async function clearNewFlag() {
    if (selected.size === 0) return
    setBusy(true)
    setStatus(null)
    try {
      for (const id of selected) {
        await fetch(`/api/archives/${id}/isnew`, { method: "DELETE" })
      }
      setStatus(formatProgress(t, selected.size, selected.size))
      await refresh()
    } finally {
      setBusy(false)
    }
  }

  async function applyTag() {
    if (!tagToAdd.trim() || selected.size === 0) return
    setBusy(true)
    setStatus(null)
    try {
      for (const id of selected) {
        const archive = await fetchArchive(id)
        const tags = [archive.tags, tagToAdd].filter(Boolean).join(", ")
        await fetch(`/api/archives/${id}/metadata?${new URLSearchParams({ tags })}`, {
          method: "PUT",
        })
      }
      setStatus(formatProgress(t, selected.size, selected.size))
      setTagToAdd("")
      await refresh()
    } finally {
      setBusy(false)
    }
  }

  async function applyCategory() {
    if (!categoryTarget || selected.size === 0) return
    setBusy(true)
    setStatus(null)
    try {
      for (const id of selected) {
        await fetch(`/api/categories/${categoryTarget}/${id}`, { method: "PUT" })
      }
      setStatus(formatProgress(t, selected.size, selected.size))
      await refresh()
    } finally {
      setBusy(false)
    }
  }

  async function deleteSelected() {
    if (selected.size === 0) return
    const count = selected.size
    if (!(await confirmDialog(t("batch.confirmDeleteSelectedN", { n: count }) ?? "", true, true))) return
    setBusy(true)
    setStatus(null)
    try {
      const response = await batchDeleteArchives.mutateAsync([...selected])
      setSelected(new Set())
      if (response.deleted === response.total) {
        setStatus(formatProgress(t, response.deleted, response.total))
        toast({ text: formatProgress(t, response.deleted, response.total), icon: "success" })
      } else {
        const message = t("batch.deletedNOfMFailed", { deleted: response.deleted, total: response.total })
        setStatus(message ?? null)
        const failedLines = response.results
          .filter((r) => !r.success)
          .map((r) => {
            const title = archives.data?.find((a) => a.arcid === r.id)?.title ?? r.id
            return r.error ? `${title}: ${r.error}` : title
          })
        // Still "warning", not "error" — the request itself succeeded with a real result list.
        toast({ heading: message ?? undefined, text: failedLines.join("\n"), icon: "warning" })
      }
      await refresh()
    } catch (err) {
      toast({
        heading: t("batch.deleteRequestFailed") ?? undefined,
        text: err instanceof ApiError ? err.message : String(err),
        icon: "error",
      })
    } finally {
      setBusy(false)
    }
  }

  function runSelectedOperation() {
    if (operation === "plugin") return applyPlugin()
    if (operation === "clearnew") return clearNewFlag()
    if (operation === "tagrules") return applyTag()
    if (operation === "addcat") return applyCategory()
    return deleteSelected()
  }

  return (
    <div className="ido">
      <h1 className="ih">{t("batch.batchOperations")}</h1>
      <p>{t("batch.selectWhatYouDLike")}</p>

      <div>
        <div className="left-column" style={{ width: 400 }}>
          <table className="tag-options">
            <tbody>
              <tr>
                <td>
                  <select id="batch-operation" className="favtag-btn" value={operation} onChange={(e) => setOperation(e.target.value as Operation)}>
                    <option value="plugin">🧩 {t("batch.usePlugin")}</option>
                    <option value="clearnew">🆕 {t("batch.removeNewFlag")}</option>
                    <option value="tagrules">📏 {t("batch.applyTagRules")}</option>
                    <option value="addcat">📚 {t("batch.addToCategory")}</option>
                    <option value="delete">🗑️ {t("common.deleteArchive")}</option>
                  </select>
                </td>
              </tr>
            </tbody>
          </table>

          <div className="id1 tag-options" style={{ padding: 4, height: "unset", width: "97%" }}>
            {operation === "plugin" && (
              <div className="operation plugin-operation">
                <table>
                  <tbody>
                    <tr>
                      <td>{t("batch.usePluginLabel")}</td>
                      <td>
                        <select value={pluginNamespace} onChange={(e) => setPluginNamespace(e.target.value)} className="favtag-btn">
                          <option value=""></option>
                          {plugins.data?.map((p) => (
                            <option key={p.namespace} value={p.namespace}>
                              {p.name}
                            </option>
                          ))}
                        </select>
                      </td>
                    </tr>
                    <tr>
                      <td>{t("batch.timeoutMax20s")}</td>
                      <td>
                        <input
                          type="number"
                          min={0}
                          max={20}
                          value={pluginTimeout}
                          onChange={(e) => setPluginTimeout(Math.min(20, Math.max(0, Number(e.target.value) || 0)))}
                        />{" "}
                        seconds
                      </td>
                    </tr>
                  </tbody>
                </table>
              </div>
            )}
            {operation === "clearnew" && (
              <div className="operation clearnew-operation" style={{ textAlign: "center" }}>
                {t("batch.thisRemovesTheNewFlag")}
              </div>
            )}
            {operation === "tagrules" && (
              <div className="operation tagrules-operation">
                <input value={tagToAdd} onChange={(e) => setTagToAdd(e.target.value)} className="stdinput" placeholder={t("common.tags") ?? undefined} />
              </div>
            )}
            {operation === "addcat" && (
              <div className="operation addcat-operation">
                <select value={categoryTarget} onChange={(e) => setCategoryTarget(e.target.value)} className="favtag-btn">
                  <option value="">{t("common.NoCategory")}</option>
                  {sortCategories(categories.data ?? []).map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
              </div>
            )}
            {operation === "delete" && (
              <div className="operation delete-operation" style={{ textAlign: "center" }}>
                <div style={{ fontSize: 36 }}>💣👀💦💦</div>
                <h3>{t("common.thisWillDeleteBothMetadata")}</h3>
              </div>
            )}
          </div>

          <div className="tag-options">
            <input type="button" id="check-uncheck" className="stdbtn" value={t("batch.selectArchives") ?? undefined} onClick={() => setSelected(new Set(selected.size ? [] : (archives.data?.map((a) => a.arcid) ?? [])))} />
            <input
              type="button"
              id="start-batch"
              className="stdbtn"
              disabled={busy || selected.size === 0}
              onClick={() => void runSelectedOperation()}
              value={t("batch.startTask") ?? undefined}
            />
          </div>
          {status && <p>{status}</p>}
        </div>

        {/* Legacy sets width: 60% inline here since .id1's own class rule would otherwise win. */}
        <div className="id1 right-column" style={{ textAlign: "center", minWidth: 400, width: "60%", height: 500, padding: 12 }}>
          <div id="arclist-container">
            <ul className="checklist" style={{ listStyle: "none" }}>
              {archives.data?.map((a) => (
                <ArchiveChecklistItem
                  key={a.arcid}
                  title={a.title}
                  checked={selected.has(a.arcid)}
                  onChange={() => toggle(a.arcid)}
                />
              ))}
            </ul>
          </div>
        </div>
      </div>

      <input type="button" id="plugin-config" className="stdbtn" value={t("common.pluginConfiguration") ?? undefined} onClick={() => navigate(routes.pluginSettings())} />
      <input type="button" id="return" className="stdbtn" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.settings())} />
    </div>
  )
}
