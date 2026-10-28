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

export function IssuesModal() {
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
    <div className="diagnostics-issues">
      <div className="diagnostics-filters">
        <div className="diagnostics-level-chips">
          {LEVELS.map((level) => (
            <button
              key={level}
              type="button"
              className={`diagnostics-chip diagnostics-chip-${level}${
                levels.includes(level) ? " is-on" : ""
              }`}
              onClick={() => toggleLevel(level)}
            >
              {t(`diagnostics.level.${level}`)}
            </button>
          ))}
        </div>
        <input
          className="stdinput"
          type="search"
          placeholder={t("diagnostics.issues.search")}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <select
          className="stdinput"
          value={state}
          onChange={(event) => setState(event.target.value as typeof state)}
        >
          <option value="open">{t("diagnostics.issues.stateOpen")}</option>
          <option value="resolved">{t("diagnostics.issues.stateResolved")}</option>
          <option value="ignored">{t("diagnostics.issues.stateIgnored")}</option>
          <option value="all">{t("diagnostics.issues.stateAll")}</option>
        </select>
        <select
          className="stdinput"
          value={sort}
          onChange={(event) => setSort(event.target.value as typeof sort)}
        >
          <option value="recent">{t("diagnostics.issues.sortRecent")}</option>
          <option value="frequent">{t("diagnostics.issues.sortFrequent")}</option>
        </select>
      </div>

      <div className="diagnostics-selection-bar">
        <span>
          {t("diagnostics.issues.selected", {
            count: selected.size,
            total: rows.length,
          })}
        </span>
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

      <div className="diagnostics-issue-list">
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
    <div className={`diagnostics-issue diagnostics-issue-${row.level}`}>
      <div className="diagnostics-issue-row">
        <CheckboxField checked={checked} onCheckedChange={onToggle}>
          <span className="diagnostics-issue-title">{row.title}</span>
        </CheckboxField>
        <span className={`diagnostics-level diagnostics-level-${row.level}`}>
          {t(`diagnostics.level.${row.level}`)}
        </span>
        <span className="diagnostics-issue-count">×{row.count}</span>
        <span className="diagnostics-issue-time">{formatStamp(row.last_seen)}</span>
        <button type="button" className="diagnostics-issue-open stdbtn" onClick={onOpen}>
          {expanded ? t("diagnostics.issues.hide") : t("diagnostics.issues.show")}
        </button>
      </div>
      {expanded && (
        <div className="diagnostics-issue-detail">
          {detail.data?.events.map((event) => (
            <div key={event.id} className="diagnostics-event">
              <div className="diagnostics-event-head">
                <span className="diagnostics-event-time">{formatStamp(event.at)}</span>
                <span className="diagnostics-event-source">{event.source}</span>
                {event.release && (
                  <span className="diagnostics-event-release">{event.release}</span>
                )}
                {event.request_id && (
                  <code className="diagnostics-event-request">req={event.request_id}</code>
                )}
              </div>
              <pre className="diagnostics-event-stack">{event.stack ?? event.message}</pre>
              {event.breadcrumbs.length > 0 && (
                <ul className="diagnostics-breadcrumbs">
                  {event.breadcrumbs.map((crumb, index) => (
                    <li key={`${crumb.at}-${index}`}>
                      <span className="diagnostics-breadcrumb-at">{formatStamp(crumb.at)}</span>
                      <span className="diagnostics-breadcrumb-cat">{crumb.category}</span>
                      {crumb.message}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          ))}
          {detail.data?.log_excerpt && (
            <details className="diagnostics-log-excerpt">
              <summary>{t("diagnostics.issues.relatedLogs")}</summary>
              {Object.entries(detail.data.log_excerpt).map(([category, lines]) => (
                <div key={category}>
                  <div className="diagnostics-log-category">{category}</div>
                  <pre>{lines.map((line) => line.text).join("\n")}</pre>
                </div>
              ))}
            </details>
          )}
          <div className="diagnostics-issue-actions">
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
