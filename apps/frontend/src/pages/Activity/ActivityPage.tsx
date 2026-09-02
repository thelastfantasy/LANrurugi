import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useActivity, useActivityFacets, useBulkDeleteActivityEntries, useDeleteActivityEntry } from "@/api/hooks"
import type { ActivityEntry } from "@/api/types"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle, useIsNarrowViewport } from "@/hooks"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"
import { toast } from "@/toast"

import { ActivityDetailPanel } from "./ActivityDetailPanel"
import { ActivityFilterBar, type ActivityFilterState } from "./ActivityFilterBar"
import { ActivityRow } from "./ActivityRow"
import { RetentionSettingsMenu } from "./RetentionSettingsInline"

const PAGE_LIMIT = 50

/** Operator Activity page — cursor-paginated against `GET /activity`, with a client-side "page
 * stack" of cursors so Previous can step back without the backend needing reverse pagination. */
export function ActivityPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  useApplyTheme()
  useDocumentTitle(t("activity.pageTitle") ?? undefined)
  const narrow = useIsNarrowViewport()

  const [filter, setFilter] = useState<ActivityFilterState>({ actors: [], actionTypes: [], outcomes: [] })
  const [cursorStack, setCursorStack] = useState<(string | undefined)[]>([undefined])
  const [pageIndex, setPageIndex] = useState(0)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [detailEntry, setDetailEntry] = useState<ActivityEntry | null>(null)

  const cursor = cursorStack[pageIndex]
  const activity = useActivity({
    cursor,
    limit: PAGE_LIMIT,
    start_ts: filter.start_ts,
    end_ts: filter.end_ts,
    actors: filter.actors,
    actionTypes: filter.actionTypes,
    outcomes: filter.outcomes,
  })
  const facets = useActivityFacets()
  const bulkDelete = useBulkDeleteActivityEntries()
  const deleteOne = useDeleteActivityEntry()

  const entries = useMemo(() => activity.data?.entries ?? [], [activity.data])
  const canDelete = true

  const allSelected = entries.length > 0 && entries.every((e) => selected.has(e.id))
  const someSelected = entries.some((e) => selected.has(e.id))
  const selectAllCheckboxRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (selectAllCheckboxRef.current) {
      selectAllCheckboxRef.current.indeterminate = someSelected && !allSelected
    }
  }, [someSelected, allSelected])

  function updateFilter(next: ActivityFilterState) {
    setFilter(next)
    setCursorStack([undefined])
    setPageIndex(0)
    setSelected(new Set())
  }

  function goNext() {
    const next = activity.data?.next_cursor
    if (!next) return
    setCursorStack((prev) => [...prev.slice(0, pageIndex + 1), next])
    setPageIndex((prev) => prev + 1)
  }

  function goPrevious() {
    if (pageIndex === 0) return
    setPageIndex((prev) => prev - 1)
  }

  function toggleOne(id: string) {
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  function toggleAllOnPage() {
    setSelected((prev) => {
      const next = new Set(prev)
      const allSelected = entries.every((e) => next.has(e.id))
      if (allSelected) entries.forEach((e) => next.delete(e.id))
      else entries.forEach((e) => next.add(e.id))
      return next
    })
  }

  async function handleBulkDelete() {
    const ids = [...selected]
    if (ids.length === 0) return
    if (!(await confirmDialog(t("activity.confirmDeleteSelectedN", { n: ids.length }) ?? "", true))) return
    try {
      const result = await bulkDelete.mutateAsync(ids)
      setSelected(new Set())
      toast({ text: t("activity.deletedNEntries", { n: result.deleted_count }) ?? undefined, icon: "success" })
    } catch {
      toast({ heading: t("activity.errorDeletingEntries") ?? undefined, icon: "error" })
    }
  }

  async function handleDeleteOne(entry: ActivityEntry) {
    if (!(await confirmDialog(t("activity.confirmDeleteEntry") ?? "", true))) return
    try {
      await deleteOne.mutateAsync(entry.id)
      setSelected((prev) => {
        if (!prev.has(entry.id)) return prev
        const next = new Set(prev)
        next.delete(entry.id)
        return next
      })
      if (detailEntry?.id === entry.id) setDetailEntry(null)
      toast({ text: t("activity.deletedNEntries", { n: 1 }) ?? undefined, icon: "success" })
    } catch {
      toast({ heading: t("activity.errorDeletingEntries") ?? undefined, icon: "error" })
    }
  }

  const isEmpty = !activity.isLoading && entries.length === 0 && pageIndex === 0
  const gridColumns = canDelete ? "auto auto auto auto 1fr auto auto" : "auto auto auto 1fr auto"

  return (
    <div className="ido" style={{ paddingLeft: 12, paddingRight: 12, boxSizing: "border-box", position: "relative" }}>
      <div style={{ position: "absolute", top: 8, right: 8 }}>
        <RetentionSettingsMenu />
      </div>
      <h1 className="ih">{t("activity.pageTitle")}</h1>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("activity.pageDescription")}</p>

      <ActivityFilterBar
        filter={filter}
        onFilterChange={updateFilter}
        facets={facets.data}
        canDelete={canDelete}
        selectedCount={selected.size}
        onBulkDelete={() => void handleBulkDelete()}
        bulkDeleting={bulkDelete.isPending}
      />

      {activity.isLoading && (
        <div id="processing">
          <i className="fa fa-3x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {isEmpty && (
        <div style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-list"></i>
          <p>{t("activity.noActivityYet")}</p>
        </div>
      )}

      {entries.length > 0 && (
        <>
          {narrow ? (
            <div style={{ marginTop: 16 }}>
              {canDelete && (
                <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "4px 4px", fontSize: FONT_SIZE_SM, opacity: 0.65 }}>
                  <input
                    ref={selectAllCheckboxRef}
                    type="checkbox"
                    aria-label={t("activity.selectAll") ?? undefined}
                    checked={allSelected}
                    onChange={toggleAllOnPage}
                  />
                  <span>{t("activity.selectAll")}</span>
                </div>
              )}
              {entries.map((entry) => (
                <div key={entry.id} style={{ borderBottom: "1px solid rgba(128,128,128,0.25)" }}>
                  <ActivityRow
                    entry={entry}
                    selected={selected.has(entry.id)}
                    selectable={canDelete}
                    narrow={narrow}
                    onToggleSelect={() => toggleOne(entry.id)}
                    onOpenDetail={() => setDetailEntry(entry)}
                    onDelete={() => void handleDeleteOne(entry)}
                  />
                </div>
              ))}
            </div>
          ) : (
            <div style={{ overflowX: "auto", marginTop: 16 }}>
              <div
                style={{
                  display: "grid",
                  gridTemplateColumns: gridColumns,
                  columnGap: 12,
                  fontSize: FONT_SIZE_SM,
                  minWidth: 640,
                }}
              >
                {canDelete && (
                  // Matches ActivityRow's checkbox cell layout exactly to avoid header/row misalignment.
                  <div style={{ display: "flex", justifyContent: "center", opacity: 0.65, padding: "4px 6px", margin: "0 -6px 0 0" }}>
                    <input
                      ref={selectAllCheckboxRef}
                      type="checkbox"
                      aria-label={t("activity.selectAll") ?? undefined}
                      checked={allSelected}
                      onChange={toggleAllOnPage}
                    />
                  </div>
                )}
                <div style={{ opacity: 0.65, padding: "4px 0", whiteSpace: "nowrap", textAlign: "left" }}>{t("activity.time")}</div>
                <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("activity.actor")}</div>
                <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("activity.action")}</div>
                <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("activity.operationContent")}</div>
                <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "left" }}>{t("activity.outcome")}</div>
                {canDelete && <div></div>}

                {entries.map((entry) => (
                  <ActivityRow
                    key={entry.id}
                    entry={entry}
                    selected={selected.has(entry.id)}
                    selectable={canDelete}
                    narrow={narrow}
                    onToggleSelect={() => toggleOne(entry.id)}
                    onOpenDetail={() => setDetailEntry(entry)}
                    onDelete={() => void handleDeleteOne(entry)}
                  />
                ))}
              </div>
            </div>
          )}

          <div className="control-btn-group" style={{ marginTop: 8, justifyContent: "center", alignItems: "center" }}>
            <button type="button" className="stdbtn" disabled={pageIndex === 0} onClick={goPrevious}>
              {t("jobs.previous")}
            </button>
            <span style={{ fontSize: FONT_SIZE_SM }}>{t("activity.pageN", { n: pageIndex + 1 })}</span>
            <button type="button" className="stdbtn" disabled={!activity.data?.next_cursor} onClick={goNext}>
              {t("jobs.next")}
            </button>
          </div>
        </>
      )}

      <div className="control-btn-group" style={{ marginTop: 12, justifyContent: "center" }}>
        <input
          type="button"
          className="stdbtn"
          value={t("common.returnToLibrary") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>

      {detailEntry && (
        <ActivityDetailPanel
          entry={detailEntry}
          onClose={() => setDetailEntry(null)}
          onDelete={canDelete ? () => void handleDeleteOne(detailEntry) : undefined}
        />
      )}
    </div>
  )
}
