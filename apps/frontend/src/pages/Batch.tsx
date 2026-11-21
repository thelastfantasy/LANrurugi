import { useQueryClient } from "@tanstack/react-query"
import type { TFunction } from "i18next"
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { ApiError, fetchJson, sendJson, sleep } from "@/api/client"
import {
  fetchAllSearchResults,
  useBatchDeleteArchives,
  useCategories,
  usePlugins,
  useSearchPages,
  useSettings,
} from "@/api/hooks"
import type { ArchiveMetadata, TankoubonMetadata } from "@/api/types"
import { BottomSheet, Drawer } from "@/components/common-ui/Display"
import { Checkbox, NumberInput } from "@/components/common-ui/Form"
import { WindowedList } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { useIsNarrowViewport } from "@/hooks/useIsNarrowViewport"
import { routes } from "@/lib/routes"
import { MSM_SELECTION_KEY } from "@/lib/storageKeys"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"
import { sortCategories } from "@/lib/utils/sortCategories"
import { useApplyTheme } from "@/theme"
import { toast } from "@/toast"

async function fetchArchive(id: string): Promise<ArchiveMetadata> {
  const response = await fetch(`/api/archives/${id}/metadata`)
  return (await response.json()) as ArchiveMetadata
}

const ShareDialog = lazy(() => import("@/share/ShareDialog").then((m) => ({ default: m.ShareDialog })))

type Operation = "plugin" | "clearnew" | "tagrules" | "addcat" | "share" | "delete"

/** Legacy's own progress line uses a DOM-id template, not `{{}}` interpolation — substituted the
 * same way rather than adding a second, differently-shaped key across all 14 locales. */
function formatProgress(t: TFunction, done: number, total: number): string {
  return t('Processed <span id="arcs"></span> out of <span id="totalarcs"></span>')
    .replace('<span id="arcs"></span>', String(done))
    .replace('<span id="totalarcs"></span>', String(total))
}

/** A premade selection handed off from the Library page's "Run Batch Operations" action — read
 * once as the initial `selected` set and immediately cleared. `TANK_` IDs are left unexpanded. */
let premadeSelection: string[] | null = null

/** Cached at module level, not just per mount: `main.tsx` wraps the app in `StrictMode`, which
 *  mounts every component twice in development — the first mount consumed and cleared the key, so
 *  the second one seeded an empty selection and the handoff looked like it had never happened. */
function takePremadeSelection(): string[] {
  if (premadeSelection) return premadeSelection
  const raw = localStorage.getItem(MSM_SELECTION_KEY)
  localStorage.removeItem(MSM_SELECTION_KEY)
  if (!raw) return (premadeSelection = [])
  try {
    const ids = JSON.parse(raw) as unknown
    premadeSelection = Array.isArray(ids) ? ids.filter((id) => typeof id === "string") : []
  } catch {
    premadeSelection = []
  }
  return premadeSelection
}

/** Mirrors legacy's batch.html.tt2 — task select + one operation row / archive checklist; runs
 * each operation archive-by-archive synchronously, no job-progress polling. */
export function Batch() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const categories = useCategories()
  const plugins = usePlugins("metadata")
  const settings = useSettings()
  const queryClient = useQueryClient()
  const batchDeleteArchives = useBatchDeleteArchives()

  const [operation, setOperation] = useState<Operation>("plugin")
  // id -> title, in the order chosen. Only what is selected is ever listed here; everything else is
  // found through the picker's search, so the page never holds the whole library.
  const [selected, setSelected] = useState<Map<string, string>>(
    () => new Map(takePremadeSelection().map((id) => [id, ""])),
  )
  const [pickerOpen, setPickerOpen] = useState(false)
  const [opsOpen, setOpsOpen] = useState(false)
  const narrow = useIsNarrowViewport("(max-width: 760px)")
  useSelectionTitles(selected, setSelected)
  const [tagToAdd, setTagToAdd] = useState("")
  const [categoryTarget, setCategoryTarget] = useState("")
  const [pluginNamespace, setPluginNamespace] = useState("")
  const [pluginTimeout, setPluginTimeout] = useState(0)
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const [sharing, setSharing] = useState(false)
  useApplyTheme()
  useDocumentTitle(t("batch.batchOperations") ?? undefined)

  const add = useCallback((entries: { id: string; title: string }[]) => {
    setSelected((prev) => {
      const next = new Map(prev)
      for (const { id, title } of entries) if (!next.has(id)) next.set(id, title)
      return next
    })
  }, [])

  function remove(id: string) {
    setSelected((prev) => {
      const next = new Map(prev)
      next.delete(id)
      return next
    })
  }

  async function refresh() {
    await queryClient.invalidateQueries({ queryKey: ["archives"] })
    await queryClient.invalidateQueries({ queryKey: ["search-pages"] })
    await queryClient.invalidateQueries({ queryKey: ["categories"] })
  }

  async function applyPlugin() {
    if (!pluginNamespace || selected.size === 0) return
    setBusy(true)
    setStatus(null)
    try {
      let processed = 0
      for (const id of selected.keys()) {
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
      for (const id of selected.keys()) {
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
      for (const id of selected.keys()) {
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
      for (const id of selected.keys()) {
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
      const titles = new Map(selected)
      const response = await batchDeleteArchives.mutateAsync([...selected.keys()])
      setSelected(new Map())
      if (response.deleted === response.total) {
        setStatus(formatProgress(t, response.deleted, response.total))
        toast({ text: formatProgress(t, response.deleted, response.total), icon: "success" })
      } else {
        const message = t("batch.deletedNOfMFailed", { deleted: response.deleted, total: response.total })
        setStatus(message ?? null)
        const failedLines = response.results
          .filter((r) => !r.success)
          .map((r) => {
            const title = titles.get(r.id) || r.id
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
    if (operation === "share") return setSharing(true)
    return deleteSelected()
  }

  const operationsPanel = (
    <>
    <table className="tag-options">
      <tbody>
        <tr>
          <td>
            <select id="batch-operation" className="favtag-btn" value={operation} onChange={(e) => setOperation(e.target.value as Operation)}>
              <option value="plugin">🧩 {t("batch.usePlugin")}</option>
              <option value="clearnew">🆕 {t("batch.removeNewFlag")}</option>
              <option value="tagrules">📏 {t("batch.applyTagRules")}</option>
              <option value="addcat">📚 {t("batch.addToCategory")}</option>
              <option value="share">🔗 {t("batch.share")}</option>
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
                  <NumberInput
                    min={0}
                    max={20}
                    value={pluginTimeout}
                    onValueChange={(v) => setPluginTimeout(Math.min(20, Math.max(0, v)))}
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
      {operation === "share" && (
        <div className="operation share-operation" style={{ textAlign: "center" }}>
          {t("batch.shareDescription")}
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
    </>
  )

  const selectedPanel = (
    <SelectedList
      selected={selected}
      onRemove={remove}
      onClear={() => setSelected(new Map())}
      onAdd={narrow ? () => setPickerOpen(true) : undefined}
    />
  )

  return (
    <div className="ido batch-page">
      <h1 className="ih">{t("batch.batchOperations")}</h1>
      <p>{t("batch.selectWhatYouDLike")}</p>

      {narrow ? (
        <>
          <div className="batch-mobile">{selectedPanel}</div>
          {/* Phone: the selection is the page; adding and running each open over it. */}
          <div className="batch-mobile-bar">
            <input
              type="button"
              className="stdbtn"
              disabled={selected.size === 0}
              value={t("batch.chooseOperation") ?? undefined}
              onClick={() => setOpsOpen(true)}
            />
          </div>
          <Drawer open={pickerOpen} onClose={() => setPickerOpen(false)} title={t("batch.addArchives")}>
            <div className="batch-drawer-body">
              <ArchivePicker selected={selected} onAdd={add} />
            </div>
          </Drawer>
          <BottomSheet open={opsOpen} onClose={() => setOpsOpen(false)} cancelLabel={t("common.close")}>
            <div className="batch-ops-sheet">{operationsPanel}</div>
          </BottomSheet>
        </>
      ) : (
        // Desktop: operations | selected | a full-height divider whose button opens the picker.
        <div className={`batch-layout${pickerOpen ? " is-picker-open" : ""}`}>
          <div className="batch-ops">{operationsPanel}</div>
          {selectedPanel}
          <div className="batch-divider">
            <button
              type="button"
              className="stdbtn batch-divider-btn"
              aria-expanded={pickerOpen}
              title={(pickerOpen ? t("batch.hidePicker") : t("batch.addArchives")) ?? undefined}
              aria-label={(pickerOpen ? t("batch.hidePicker") : t("batch.addArchives")) ?? undefined}
              onClick={() => setPickerOpen((open) => !open)}
            >
              <i className={`fa ${pickerOpen ? "fa-chevron-right" : "fa-chevron-left"}`} aria-hidden="true" />
            </button>
          </div>
          {pickerOpen && <ArchivePicker selected={selected} onAdd={add} />}
        </div>
      )}

      {sharing && (
        <Suspense fallback={null}>
          <ShareDialog
            items={[...selected.keys()].map((id) => ({ kind: isTankoubonId(id) ? ("tankoubon" as const) : ("archive" as const), id }))}
            onClose={() => setSharing(false)}
          />
        </Suspense>
      )}

      <div style={{ display: "flex", justifyContent: "center", gap: 8 }}>
        <input type="button" id="plugin-config" className="stdbtn" value={t("common.pluginConfiguration") ?? undefined} onClick={() => navigate(routes.pluginSettings())} />
        <input type="button" id="return" className="stdbtn" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.settings())} />
      </div>
    </div>
  )
}

/** Fills in titles for selected ids that arrived without one (a selection handed over from the
 *  Library is ids only), a few requests at a time. Each id is asked for once. */
function useSelectionTitles(
  selected: Map<string, string>,
  setSelected: React.Dispatch<React.SetStateAction<Map<string, string>>>,
) {
  const requested = useRef(new Set<string>())
  useEffect(() => {
    const missing = [...selected].filter(([id, title]) => !title && !requested.current.has(id)).map(([id]) => id)
    if (missing.length === 0) return
    for (const id of missing) requested.current.add(id)
    const queue = [...missing]
    const settle = (id: string, title: string) =>
      setSelected((prev) => {
        if (!prev.has(id) || prev.get(id)) return prev
        const next = new Map(prev)
        next.set(id, title)
        return next
      })
    const worker = async () => {
      for (let id = queue.shift(); id !== undefined; id = queue.shift()) {
        try {
          const title = isTankoubonId(id)
            ? (await fetchJson<TankoubonMetadata>(`/tankoubons/${id}`)).name
            : (await fetchJson<ArchiveMetadata>(`/archives/${id}/metadata`)).title
          settle(id, title || id)
        } catch {
          settle(id, id)
        }
      }
    }
    void Promise.all(Array.from({ length: Math.min(6, queue.length) }, worker))
  }, [selected, setSelected])
}

const ROW_HEIGHT = 26

/** The selection itself: what the operation will run on, each removable. */
function SelectedList({
  selected,
  onRemove,
  onClear,
  onAdd,
}: {
  selected: Map<string, string>
  onRemove: (id: string) => void
  onClear: () => void
  /** Phone only: opens the picker, which on a desktop is the third column instead. */
  onAdd?: () => void
}) {
  const { t } = useTranslation()
  const rows = [...selected]
  return (
    <div className="batch-panel batch-selected">
      <div className="batch-panel-head">
        <b>{t("batch.selectedN", { n: selected.size })}</b>
        <span style={{ marginLeft: "auto", display: "flex", gap: 4 }}>
          {onAdd && (
            <input type="button" className="stdbtn batch-head-btn" value={t("batch.addArchives") ?? undefined} onClick={onAdd} />
          )}
          <input
            type="button"
            className="stdbtn batch-head-btn"
            disabled={selected.size === 0}
            value={t("batch.clearSelection") ?? undefined}
            onClick={onClear}
          />
        </span>
      </div>
      {rows.length === 0 ? (
        <p className="batch-empty">{t("batch.nothingSelected")}</p>
      ) : (
        <WindowedList
          className="batch-list"
          items={rows}
          rowHeight={ROW_HEIGHT}
          itemKey={([id]) => id}
          renderRow={([id, title]) => (
            <div className="batch-row">
              {isTankoubonId(id) && <i className="fa fa-book batch-row-kind" aria-hidden="true" />}
              <span className="batch-row-title" title={title || id}>
                {title || <span className="batch-row-loading">{id}</span>}
              </span>
              <button
                type="button"
                className="favtag-btn batch-row-remove"
                aria-label={t("batch.removeFromSelection") ?? undefined}
                title={t("batch.removeFromSelection") ?? undefined}
                onClick={() => onRemove(id)}
              >
                <i className="fa fa-xmark" aria-hidden="true" />
              </button>
            </div>
          )}
        />
      )}
    </div>
  )
}

/** Everything not yet selected, found through the Library's own search (server-side, so its query
 *  syntax and filters work and nothing loads the whole library), newest first, a page at a time. */
function ArchivePicker({
  selected,
  onAdd,
}: {
  selected: Map<string, string>
  onAdd: (entries: { id: string; title: string }[]) => void
}) {
  const { t } = useTranslation()
  const [kind, setKind] = useState<"archives" | "tanks">("archives")
  const [input, setInput] = useState("")
  const [filter, setFilter] = useState("")
  const [addingAll, setAddingAll] = useState(false)
  useEffect(() => {
    const timer = window.setTimeout(() => setFilter(input.trim()), 300)
    return () => window.clearTimeout(timer)
  }, [input])

  const options = {
    filter,
    sortby: "date_added",
    order: "desc" as const,
    // Archives one by one (a Tankoubon's members included); Tankoubons on their own tab.
    ...(kind === "archives" ? { groupbyTanks: false } : { tankonly: true }),
  }
  const pages = useSearchPages(options)
  const results = (pages.data?.pages ?? []).flatMap((page) => page.data)
  const shown = results.filter((a) => !selected.has(a.arcid))
  const total = pages.data?.pages[0]?.recordsFiltered ?? 0
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = pages
  const loadMore = useCallback(() => {
    if (hasNextPage && !isFetchingNextPage) void fetchNextPage()
  }, [hasNextPage, isFetchingNextPage, fetchNextPage])

  async function addAll() {
    setAddingAll(true)
    try {
      const all = await fetchAllSearchResults(options)
      onAdd(all.data.map((a) => ({ id: a.arcid, title: a.title })))
    } finally {
      setAddingAll(false)
    }
  }

  return (
    <div className="batch-panel batch-picker">
      <div className="batch-panel-head">
        <input
          type="search"
          className="stdinput"
          style={{ flex: 1, minWidth: 0 }}
          placeholder={t("batch.searchPlaceholder") ?? undefined}
          value={input}
          onChange={(e) => setInput(e.target.value)}
        />
      </div>
      <div className="batch-panel-head">
        <span className="sub-modal-tabs" role="tablist" style={{ margin: 0, flex: 1 }}>
          {(["archives", "tanks"] as const).map((k) => (
            <button
              key={k}
              type="button"
              role="tab"
              aria-selected={kind === k}
              className={kind === k ? "stdbtn is-active" : "stdbtn"}
              onClick={() => setKind(k)}
            >
              {k === "archives" ? t("batch.tabArchives") : t("batch.tabTankoubons")}
            </button>
          ))}
        </span>
        <input
          type="button"
          className="stdbtn batch-head-btn"
          disabled={total === 0 || addingAll}
          value={t("batch.addAllResults", { n: total }) ?? undefined}
          onClick={() => void addAll()}
        />
      </div>
      {pages.isPending ? (
        <p className="batch-empty">{t("common.loading")}</p>
      ) : shown.length === 0 && !hasNextPage ? (
        <p className="batch-empty">{t("batch.noResults")}</p>
      ) : (
        <WindowedList
          className="batch-list"
          items={shown}
          rowHeight={ROW_HEIGHT}
          itemKey={(a) => a.arcid}
          onEndReached={loadMore}
          renderRow={(a) => (
            <label className="batch-row batch-row-pick">
              <Checkbox checked={false} onCheckedChange={() => onAdd([{ id: a.arcid, title: a.title }])} />
              <span className="batch-row-title" title={a.title}>
                {a.title}
              </span>
            </label>
          )}
        />
      )}
    </div>
  )
}
