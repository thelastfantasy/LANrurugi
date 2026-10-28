/** The issue list (level/time/source filters, multi-select) and one issue's detail. */

import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { CheckboxField } from "@/components/common-ui/Form"

import { type IssueRow,useIssueDetail, useIssues, useSetIssueState } from "./api"

const LEVELS = ["error", "warning", "info"] as const

function formatStamp(millis: number): string {
  const date = new Date(millis)
  if (Number.isNaN(date.getTime())) return "—"
  const pad = (value: number) => String(value).padStart(2, "0")
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

export function IssuesView() {
  const { t } = useTranslation()
  const [levels, setLevels] = useState<string[]>([...LEVELS])
  const [state, setState] = useState<"open" | "resolved" | "ignored" | "all">("open")
  const [query, setQuery] = useState("")
  const [sort, setSort] = useState<"recent" | "frequent">("recent")
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [open, setOpen] = useState<string | null>(null)

  const params = useMemo(() => {
    const search = new URLSearchParams()
    levels.forEach((level) => search.append("level", level))
    if (state !== "all") search.set("state", state)
    if (query.trim()) search.set("q", query.trim())
    search.set("sort", sort)
    search.set("limit", "100")
    return search
  }, [levels, state, query, sort])

  const issues = useIssues(params)
  const setIssueState = useSetIssueState()
  const rows = issues.data?.issues ?? []

  const toggleLevel = (level: string) =>
    setLevels((current) =>
      current.includes(level) ? current.filter((value) => value !== level) : [...current, level],
    )

  const toggleRow = (fingerprint: string) =>
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(fingerprint)) next.delete(fingerprint)
      else next.add(fingerprint)
      return next
    })

  const onlyErrors = () => {
    setLevels(["error"])
    setSelected(new Set(rows.filter((row) => row.level === "error").map((row) => row.fingerprint)))
  }

  return (
    <div className="diag-issues">
      <div className="diag-filters">
        <div className="diag-chips">
          {LEVELS.map((level) => (
            <button
              key={level}
              type="button"
              className={`diag-chip diag-chip-${level}${
                levels.includes(level) ? " is-on" : ""
              }`}
              onClick={() => toggleLevel(level)}
            >
              {t(`diagnostics.level.${level}`)}
            </button>
          ))}
        </div>
        <input
          className="stdinput diag-search"
          type="search"
          placeholder={t("diagnostics.issues.search")}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <select
          className="stdinput diag-select-state"
          value={state}
          onChange={(event) => setState(event.target.value as typeof state)}
        >
          <option value="open">{t("diagnostics.issues.stateOpen")}</option>
          <option value="resolved">{t("diagnostics.issues.stateResolved")}</option>
          <option value="ignored">{t("diagnostics.issues.stateIgnored")}</option>
          <option value="all">{t("diagnostics.issues.stateAll")}</option>
        </select>
        <select
          className="stdinput diag-select-sort"
          value={sort}
          onChange={(event) => setSort(event.target.value as typeof sort)}
        >
          <option value="recent">{t("diagnostics.issues.sortRecent")}</option>
          <option value="frequent">{t("diagnostics.issues.sortFrequent")}</option>
        </select>
      </div>

      <div className="diag-selection-bar">
        <span className="diag-selection-count">
          {t("diagnostics.issues.selected", {
            count: selected.size,
            total: rows.length,
          })}
        </span>
        <div className="diag-selection-actions">
        <button type="button" className="stdbtn" onClick={() => setSelected(new Set(rows.map((row) => row.fingerprint)))}>
          {t("diagnostics.issues.selectAll")}
        </button>
        <button type="button" className="stdbtn" onClick={() => setSelected(new Set())}>
          {t("diagnostics.issues.selectNone")}
        </button>
          <button type="button" className="stdbtn" onClick={onlyErrors}>
            {t("diagnostics.issues.onlyErrors")}
          </button>
        </div>
      </div>

      <div className="diag-issue-list">
        {rows.map((row) => (
          <IssueLine
            key={row.fingerprint}
            row={row}
            checked={selected.has(row.fingerprint)}
            onToggle={() => toggleRow(row.fingerprint)}
            onOpen={() => setOpen(row.fingerprint === open ? null : row.fingerprint)}
            expanded={open === row.fingerprint}
            onResolve={(next) =>
              setIssueState.mutate({ fingerprint: row.fingerprint, state: next })
            }
          />
        ))}
        {rows.length === 0 && !issues.isLoading && (
          <p className="settings-hint">{t("diagnostics.issues.empty")}</p>
        )}
      </div>
    </div>
  )
}

function IssueLine({
  row,
  checked,
  onToggle,
  onOpen,
  expanded,
  onResolve,
}: {
  row: IssueRow
  checked: boolean
  onToggle: () => void
  onOpen: () => void
  expanded: boolean
  onResolve: (state: IssueRow["state"]) => void
}) {
  const { t } = useTranslation()
  const detail = useIssueDetail(expanded ? row.fingerprint : null)

  return (
    <div className={`diagnostics-issue diag-issue-${row.level}`}>
      <div className="diag-issue-row">
        <CheckboxField checked={checked} onCheckedChange={onToggle}>
          <span className="diag-issue-title">{row.title}</span>
        </CheckboxField>
        <span className={`diag-level diag-level-${row.level}`}>
          {t(`diagnostics.level.${row.level}`)}
        </span>
        <span className="diag-issue-count">×{row.count}</span>
        <span className="diag-issue-time">{formatStamp(row.last_seen)}</span>
        <button type="button" className="diag-issue-open stdbtn" onClick={onOpen}>
          {expanded ? t("diagnostics.issues.hide") : t("diagnostics.issues.show")}
        </button>
      </div>
      {expanded && (
        <div className="diag-issue-detail">
          {detail.data?.events.map((event) => (
            <div key={event.id} className="diag-event">
              <div className="diag-event-head">
                <span className="diag-event-time">{formatStamp(event.at)}</span>
                <span className="diag-event-source">{event.source}</span>
                {event.release && (
                  <span className="diag-event-release">{event.release}</span>
                )}
                {event.request_id && (
                  <code className="diag-event-request">req={event.request_id}</code>
                )}
              </div>
              <pre className="diag-event-stack">{event.stack ?? event.message}</pre>
              {event.breadcrumbs.length > 0 && (
                <ul className="diag-breadcrumbs">
                  {event.breadcrumbs.map((crumb, index) => (
                    <li key={`${crumb.at}-${index}`}>
                      <span className="diag-breadcrumb-at">{formatStamp(crumb.at)}</span>
                      <span className="diag-breadcrumb-cat">{crumb.category}</span>
                      {crumb.message}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ))}
          {detail.data?.log_excerpt && (
            <details className="diag-log-excerpt">
              <summary>{t("diagnostics.issues.relatedLogs")}</summary>
              {Object.entries(detail.data.log_excerpt).map(([category, lines]) => (
                <div key={category}>
                  <div className="diag-log-category">{category}</div>
                  <pre>{lines.map((line) => line.text).join("\n")}</pre>
                </div>
              ))}
            </details>
          )}
          <div className="diag-issue-actions">
            <button type="button" className="stdbtn" onClick={() => onResolve("resolved")}>
              {t("diagnostics.issues.resolve")}
            </button>
            <button type="button" className="stdbtn" onClick={() => onResolve("ignored")}>
              {t("diagnostics.issues.ignore")}
            </button>
            <button type="button" className="stdbtn" onClick={() => onResolve("open")}>
              {t("diagnostics.issues.reopen")}
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
