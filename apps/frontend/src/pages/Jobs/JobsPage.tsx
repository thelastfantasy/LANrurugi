import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useClearFinishedJobs, useClearJobs, useJobs } from "@/api/hooks"
import type { JobRecord, JobRecordState } from "@/api/types"
import { STATE_COLOR } from "@/components/Display"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"

import { FilterChip } from "./FilterChip"
import { isTerminal, JobRow, STATE_LABEL_KEYS } from "./JobRow"

const PAGE_SIZES = [10, 20, 50, 100]
const DEFAULT_PAGE_SIZE = 50

/** Render order for states in the stat bar + filter. */
const STATE_ORDER: JobRecordState[] = ["active", "queued", "finished", "failed"]

/** Background Job Console: browsable admin UI over the in-process job registry — list, inspect,
 * filter/search/paginate, and batch-clear terminal jobs. No Workers/Locks stats (single-process). */
export function Jobs() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const jobs = useJobs()
  const clearJobs = useClearJobs()
  const clearFinished = useClearFinishedJobs()
  useApplyTheme()
  useDocumentTitle(t("jobs.backgroundJobs") ?? undefined)

  const all: JobRecord[] = useMemo(() => jobs.data ?? [], [jobs.data])

  const counts = useMemo(() => {
    const c: Record<JobRecordState, number> = { queued: 0, active: 0, finished: 0, failed: 0 }
    for (const job of all) c[job.state] += 1
    return c
  }, [all])

  const [stateFilter, setStateFilter] = useState<JobRecordState | "all">("all")
  const [search, setSearch] = useState("")
  const [page, setPage] = useState(0)
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [status, setStatus] = useState("")

  const filtered = useMemo(() => {
    const needle = search.trim().toLowerCase()
    return all.filter((job) => {
      if (stateFilter !== "all" && job.state !== stateFilter) return false
      if (needle && !job.name.toLowerCase().includes(needle)) return false
      return true
    })
  }, [all, stateFilter, search])

  // Pagination applied after filters; clamp when the filtered set shrinks.
  const pageCount = Math.max(1, Math.ceil(filtered.length / pageSize))
  const safePage = Math.min(page, pageCount - 1)
  const paginated = filtered.slice(safePage * pageSize, safePage * pageSize + pageSize)

  const liveIds = useMemo(() => new Set(all.map((j) => j.id)), [all])
  const liveSelected = useMemo(
    () => new Set([...selected].filter((id) => liveIds.has(id))),
    [selected, liveIds],
  )

  const pageTerminalIds = useMemo(
    () => paginated.filter((j) => isTerminal(j.state)).map((j) => j.id),
    [paginated],
  )
  const allPageSelected =
    pageTerminalIds.length > 0 && pageTerminalIds.every((id) => liveSelected.has(id))
  const somePageSelected = pageTerminalIds.some((id) => liveSelected.has(id))

  const selectAllRef = useRef<HTMLInputElement>(null)
  useEffect(() => {
    if (selectAllRef.current) {
      selectAllRef.current.indeterminate = somePageSelected && !allPageSelected
    }
  }, [somePageSelected, allPageSelected])

  function toggleAll() {
    setSelected((prev) => {
      const next = new Set(prev)
      if (pageTerminalIds.every((id) => next.has(id))) {
        pageTerminalIds.forEach((id) => next.delete(id))
      } else {
        pageTerminalIds.forEach((id) => next.add(id))
      }
      return next
    })
  }

  function toggleOne(job: JobRecord) {
    if (!isTerminal(job.state)) return
    setSelected((prev) => {
      const next = new Set(prev)
      if (next.has(job.id)) next.delete(job.id)
      else next.add(job.id)
      return next
    })
  }

  async function handleRemoveSelected() {
    const ids = [...liveSelected]
    if (ids.length === 0) return
    const { succeeded, failed } = await clearJobs.mutateAsync(ids)
    setSelected(new Set())
    setStatus(
      t("jobs.removedNJobs", { n: succeeded }) +
        (failed > 0 ? " " + t("jobs.couldNotBeRemoved", { n: failed }) : ""),
    )
  }

  async function handleClearFinished() {
    const result = await clearFinished.mutateAsync()
    setSelected(new Set())
    setStatus(t("jobs.clearedNFinishedJobs", { n: result.cleared }) ?? "")
  }

  const isEmpty = !jobs.isLoading && all.length === 0
  const removing = clearJobs.isPending

  return (
    <div className="ido" style={{ paddingLeft: 12, paddingRight: 12 }}>
      <h1 className="ih">{t("jobs.backgroundJobs")}</h1>
      <p style={{ fontSize: FONT_SIZE_SM }}>
        {t("jobs.theBackgroundJobConsoleShows")}
      </p>

      <div className="control-btn-group" style={{ flexWrap: "wrap", gap: 4 }}>
        <FilterChip
          active={stateFilter === "all"}
          label={t("jobs.all") ?? ""}
          count={all.length}
          onClick={() => {
            setStateFilter("all")
            setPage(0)
          }}
        />
        {STATE_ORDER.map((state) => (
          <FilterChip
            key={state}
            active={stateFilter === state}
            label={t(STATE_LABEL_KEYS[state]) ?? ""}
            count={counts[state]}
            color={STATE_COLOR[state]}
            onClick={() => {
              setStateFilter(state)
              setPage(0)
            }}
          />
        ))}
      </div>

      <div className="control-btn-group" style={{ marginTop: 8, alignItems: "center" }}>
        <input
          className="stdinput"
          type="text"
          style={{ flexGrow: 1, minWidth: 160 }}
          placeholder={t("jobs.searchByName") ?? ""}
          value={search}
          onChange={(e) => {
            setSearch(e.target.value)
            setPage(0)
          }}
        />
        <button
          type="button"
          className="stdbtn"
          disabled={liveSelected.size === 0 || removing}
          onClick={() => void handleRemoveSelected()}
        >
          {removing
            ? t("jobs.removing")
            : t("jobs.removeSelectedN", { n: liveSelected.size })}
        </button>
        <button
          type="button"
          className="stdbtn"
          disabled={clearFinished.isPending}
          onClick={() => void handleClearFinished()}
        >
          {clearFinished.isPending ? t("jobs.clearing") : t("jobs.clearAllFinished")}
        </button>
        <input
          type="button"
          id="return"
          className="stdbtn"
          value={t("common.returnToLibrary") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>

      {status && <p style={{ fontSize: FONT_SIZE_SM }}>{status}</p>}

      {jobs.isLoading && (
        <div id="processing">
          <i className="fa fa-3x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {isEmpty && (
        <div id="nojobs" style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-inbox"></i>
          <p>{t("jobs.noBackgroundJobsYet")}</p>
          <p style={{ fontSize: FONT_SIZE_SM }}>
            {t(
              "jobs.jobsWillAppearHereAs",
            )}
          </p>
        </div>
      )}

      {all.length > 0 && filtered.length === 0 && (
        <div style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-search"></i>
          <p>{t("jobs.noJobsMatchTheCurrent")}</p>
        </div>
      )}

      {all.length > 0 && filtered.length > 0 && (
        <>
          <div style={{ overflowX: "auto", marginTop: 16 }}>
            <table className="itg" style={{ width: "100%", minWidth: 560 }}>
              <thead>
                <tr>
                  <th style={{ width: 32, textAlign: "center" }}>
                    <input
                      ref={selectAllRef}
                      type="checkbox"
                      aria-label={t("jobs.selectAll") ?? undefined}
                      checked={allPageSelected}
                      onChange={toggleAll}
                    />
                  </th>
                  <th style={{ textAlign: "left" }}>{t("jobs.name")}</th>
                  <th style={{ width: 100, textAlign: "center" }}>{t("jobs.state")}</th>
                  <th style={{ width: 180, textAlign: "center" }}>{t("jobs.progress")}</th>
                  <th style={{ width: 28 }}></th>
                </tr>
              </thead>
              <tbody>
                {paginated.map((job) => (
                  <JobRow
                    key={job.id}
                    job={job}
                    selected={liveSelected.has(job.id)}
                    onToggleSelect={() => toggleOne(job)}
                  />
                ))}
              </tbody>
            </table>
          </div>

          <div
            className="control-btn-group"
            style={{ marginTop: 8, justifyContent: "center", alignItems: "center" }}
          >
            <button
              type="button"
              className="stdbtn"
              disabled={safePage === 0}
              onClick={() => setPage(safePage - 1)}
            >
              {t("jobs.previous")}
            </button>
            <span style={{ fontSize: FONT_SIZE_SM }}>
              {t("jobs.pageNOfTotal", { n: safePage + 1, total: pageCount })}
            </span>
            <button
              type="button"
              className="stdbtn"
              disabled={safePage >= pageCount - 1}
              onClick={() => setPage(safePage + 1)}
            >
              {t("jobs.next")}
            </button>
            <label style={{ fontSize: FONT_SIZE_SM }}>
              {t("jobs.perPage")}
              <select
                className="stdinput"
                style={{ marginLeft: 6, width: "auto" }}
                value={pageSize}
                onChange={(e) => {
                  setPageSize(Number(e.target.value))
                  setPage(0)
                }}
              >
                {PAGE_SIZES.map((size) => (
                  <option key={size} value={size}>
                    {size}
                  </option>
                ))}
              </select>
            </label>
          </div>
        </>
      )}
    </div>
  )
}
