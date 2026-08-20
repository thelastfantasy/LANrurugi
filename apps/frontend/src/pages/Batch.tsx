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
import { useApplyTheme } from "@/theme"
import { toast } from "@/toast"

async function fetchArchive(id: string): Promise<ArchiveMetadata> {
  const response = await fetch(`/api/archives/${id}/metadata`)
  return (await response.json()) as ArchiveMetadata
}

type Operation = "plugin" | "clearnew" | "tagrules" | "addcat" | "delete"

/** Legacy's own progress line (`batch.js`'s `#arcs`/`#totalarcs` DOM-id template, not `{{}}`
 * interpolation) — substituted the same way rather than adding a second, differently-shaped key
 * for the same translated sentence across all 14 locales. */
function formatProgress(t: (key: string) => string, done: number, total: number): string {
  return t('Processed <span id="arcs"></span> out of <span id="totalarcs"></span>')
    .replace('<span id="arcs"></span>', String(done))
    .replace('<span id="totalarcs"></span>', String(total))
}

// Mirrors legacy's `~/LANraragi/templates/batch.html.tt2` — `.left-column` (task `<select
// class="favtag-btn">` + one `.tag-options` row per operation, only the selected one shown) /
// `.id1.right-column` (`ul.checklist` of archives to apply it to). Doesn't reproduce per-plugin arg
// overrides or job-progress polling (`batch.js`) — this still runs each operation archive-by-archive
// synchronously, matching what the existing hooks/API surface already do.
/** A premade selection handed off from the index page's multi-select mode (`Library.tsx`'s own
 * "Run Batch Operations on selection" action, which opens this page in a new tab — matching
 * legacy's own `openBatchOnSelection`) — read once as the initial `selected` set and immediately
 * cleared, exactly matching legacy's own `localStorage.getItem/removeItem("msmSelection")` pair
 * (`~/LANraragi/public/js/{mod/index,batch}.js`). `TANK_`-prefixed IDs (a Tankoubon caught up in
 * the selection) are left as-is rather than expanded to their constituent archives — a real,
 * documented simplification versus legacy's own fetch-and-expand behavior. */
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
        // Mirrors legacy's `Controller::Batch::batch_plugin` (`set_tags($id, $new_tags, 1)` —
        // the trailing `1` means append-to-existing, not replace — plus `set_title`/`set_summary`
        // when present) applying the plugin's result directly, unlike the single-archive Edit
        // page's staging-then-Save flow: a batch run over many archives has no per-archive review
        // step to stage through, so legacy persists immediately, and this matches that.
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
      // One request for the whole batch (issue #63) — the backend does the same per-id delete
      // work `DELETE /archives/{id}` always did, just server-side in a loop, and reports which
      // ids actually succeeded so a partial failure (an id already deleted elsewhere, a locked
      // file, ...) doesn't silently read as "all done" the way the old client-side loop did.
      const response = await batchDeleteArchives.mutateAsync([...selected])
      setSelected(new Set())
      if (response.deleted === response.total) {
        setStatus(formatProgress(t, response.deleted, response.total))
        toast({ text: formatProgress(t, response.deleted, response.total), icon: "success" })
      } else {
        const message = t("batch.deletedNOfMFailed", { deleted: response.deleted, total: response.total })
        setStatus(message ?? null)
        // Named per-title with its own failure reason, not just a bare count — "3 of 10 failed"
        // alone gives no way to tell *which* 3 or *why* without cross-referencing ids by hand
        // against server logs. Falls back to the bare id if the archive isn't in the already-loaded
        // `archives.data` for some reason (most commonly: `error` already says "does not exist",
        // so there's nothing in the library list to look the title up in).
        const failedLines = response.results
          .filter((r) => !r.success)
          .map((r) => {
            const title = archives.data?.find((a) => a.arcid === r.id)?.title ?? r.id
            return r.error ? `${title}: ${r.error}` : title
          })
        // `deleted === 0` (every single one failed, not just some) still shows as `warning` here
        // rather than `error` — the request itself succeeded (a real per-id result list came
        // back, this isn't a network/auth failure), so it's a "here's what happened, some/all of
        // it didn't go as planned" situation, same as a partial failure, just at 0%. The `catch`
        // branch below is what's reserved for `error` — the request itself never completing.
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

          {/* Legacy overrides `.id1`'s own `width: 228px`/`min-height: 335px` (meant for
              thumbnail cards) inline here too: `style="padding:4px; height:unset; width:97%;"`. */}
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
                  {categories.data?.map((c) => (
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

        {/* Legacy sets `width: 60% !important` inline on this exact element
            (`~/LANraragi/templates/batch.html.tt2`) since `.id1`'s own class rule (`width: 228px`,
            meant for thumbnail cards) would otherwise win. */}
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
