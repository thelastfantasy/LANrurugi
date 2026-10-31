/** The issue list (level/time/source filters, multi-select) and one issue's detail. */

import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { CheckboxField, Select } from "@/components/common-ui/Form"

import {
  type IssueRow,
  useBulkSetIssueState,
  useDiagnosticsOverview,
  useIssueDetail,
  useIssues,
  useSetIssueState,
} from "./api"
import { useIssueSelection } from "./selection"

const LEVELS = ["error", "warning", "info"] as const

/// The state filter's options, each with the same glyph the rows use, so the trigger reads like the
/// list it controls.
const STATE_OPTIONS = [
  { value: "open", icon: "fa-circle-dot", labelKey: "diagnostics.issues.stateOpen" },
  { value: "resolved", icon: "fa-circle-check", labelKey: "diagnostics.issues.stateResolved" },
  { value: "ignored", icon: "fa-circle-minus", labelKey: "diagnostics.issues.stateIgnored" },
  { value: "all", icon: "fa-layer-group", labelKey: "diagnostics.issues.stateAll" },
] as const

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
  const selected = useIssueSelection((state) => state.selected)
  const setMany = useIssueSelection((state) => state.setMany)
  const toggleSelected = useIssueSelection((state) => state.toggle)
  const [open, setOpen] = useState<string | null>(null)

  const params = useMemo(() => {
    const search = new URLSearchParams()
    levels.forEach((level) => search.append("level", level))
    search.set("state", state)
    if (query.trim()) search.set("q", query.trim())
    search.set("sort", sort)
    search.set("limit", "100")
    return search
  }, [levels, state, query, sort])

  const issues = useIssues(params)
  const bulkState = useBulkSetIssueState()
  const retain = useIssueSelection((state) => state.retain)
  const counts = useIssues(
    useMemo(() => {
      const search = new URLSearchParams()
      search.set("state", state)
      search.set("limit", "200")
      return search
    }, [state]),
  )
  const perLevel = useMemo(() => {
    const tally: Record<string, number> = {}
    for (const row of counts.data?.issues ?? []) {
      tally[row.level] = (tally[row.level] ?? 0) + 1
    }
    return tally
  }, [counts.data])
  const overview = useDiagnosticsOverview()
  const settings = (overview.data?.settings ?? null) as
    | { tracker_configured?: boolean; tracker_host?: string | null; tracker_project?: string | null }
    | null
  const setIssueState = useSetIssueState()
  const rows = issues.data?.issues ?? []
  const visibleKey = rows.map((row) => row.fingerprint).join(",")
  // Keep the selection inside what is actually listed; see `retain`.
  useEffect(() => {
    // Only once the new list has arrived: a filter change momentarily has no data, and pruning
    // against that empty set wiped the selection entirely.
    if (!issues.data) return
    retain(issues.data.issues.map((row) => row.fingerprint))
    // `visibleKey` is the stable identity of this list; `rows` itself is a fresh array each render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visibleKey, retain])

  const toggleLevel = (level: string) =>
    setLevels((current) =>
      current.includes(level) ? current.filter((value) => value !== level) : [...current, level],
    )



  return (
    <div className="diag-issues">
      {settings?.tracker_configured && settings.tracker_host && settings.tracker_project && (
        <p className="diag-tagline">
          <a
            className="diag-external"
            href={`${settings.tracker_host}/projects/${settings.tracker_project}/issues`}
            target="_blank"
            rel="noreferrer"
          >
            {t("diagnostics.issues.inTracker")}
          </a>{" "}
          {t("diagnostics.issues.inTrackerHint")}
        </p>
      )}
      <div className="diag-filters">
        <div className="diag-filter-row">
          {LEVELS.map((level) => (
            <button
              key={level}
              type="button"
              className={`diag-chip diag-level-${level}${levels.includes(level) ? " is-on" : ""}`}
              onClick={() => toggleLevel(level)}
            >
              {t(`diagnostics.level.${level}`)}
              <span className="diag-chip-count">{perLevel[level] ?? 0}</span>
            </button>
          ))}
        </div>
        <div className="diag-filter-row">
          <div className="diag-select-state">
            <Select
              value={state}
              onValueChange={(next) => setState(next as typeof state)}
              align="start"
              variant="stdbtn"
              style={{ width: "100%" }}
              ariaLabel={t("diagnostics.issues.stateLabel")}
              items={STATE_OPTIONS.map((option) => ({
                value: option.value,
                label: (
                  <span className={`diag-state-choice diag-state-${option.value}`}>
                    <i className={`fas ${option.icon}`} aria-hidden="true" />
                    {t(option.labelKey)}
                  </span>
                ),
              }))}
            />
          </div>
          <div className="diag-select-sort">
            <Select
              value={sort}
              onValueChange={(next) => setSort(next as typeof sort)}
              align="start"
              variant="stdbtn"
              style={{ width: "100%" }}
              ariaLabel={t("diagnostics.issues.sortLabel")}
              items={[
                { value: "recent", label: t("diagnostics.issues.sortRecent") },
                { value: "frequent", label: t("diagnostics.issues.sortFrequent") },
              ]}
            />
          </div>
        </div>
        <div className="diag-filter-row">
          <input
            className="stdinput diag-search"
            type="search"
            placeholder={t("diagnostics.issues.search")}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </div>
      </div>

      <div className="diag-selection-bar">
        <CheckboxField
          checked={rows.length > 0 && selected.size === rows.length}
          indeterminate={selected.size > 0 && selected.size < rows.length}
          onCheckedChange={(checked) =>
            setMany(
              rows.map((row) => row.fingerprint),
              checked,
            )
          }
        >
          <span className="diag-selection-count">
            {t("diagnostics.issues.selected", {
              count: selected.size,
              total: rows.length,
            })}
          </span>
        </CheckboxField>
        {selected.size > 0 && (
          <span className="diag-usage diag-bulk-hint">
            {t("diagnostics.issues.selectionHint")}
          </span>
        )}
        {selected.size > 0 && (
          <div className="diag-bulk">
            <button
              type="button"
              className="diag-state-btn"
              title={t("diagnostics.issues.bulkResolve", { count: selected.size })}
              aria-label={t("diagnostics.issues.bulkResolve", { count: selected.size })}
              onClick={() => bulkState.mutate({ fingerprints: [...selected], state: "resolved" })}
            >
              <i className="fas fa-circle-check" aria-hidden="true" />
            </button>
            <button
              type="button"
              className="diag-state-btn"
              title={t("diagnostics.issues.bulkIgnore", { count: selected.size })}
              aria-label={t("diagnostics.issues.bulkIgnore", { count: selected.size })}
              onClick={() => bulkState.mutate({ fingerprints: [...selected], state: "ignored" })}
            >
              <i className="fas fa-circle-minus" aria-hidden="true" />
            </button>
            <button
              type="button"
              className="diag-state-btn"
              title={t("diagnostics.issues.bulkReopen", { count: selected.size })}
              aria-label={t("diagnostics.issues.bulkReopen", { count: selected.size })}
              onClick={() => bulkState.mutate({ fingerprints: [...selected], state: "open" })}
            >
              <i className="fas fa-rotate-left" aria-hidden="true" />
            </button>
          </div>
        )}
      </div>

      <div className="diag-issue-list">
        {rows.map((row) => (
          <IssueLine
            key={row.fingerprint}
            row={row}
            checked={selected.has(row.fingerprint)}
            onToggle={() => toggleSelected(row.fingerprint)}
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
    <div
      className={`diagnostics-issue diag-issue-${row.level} diag-issue-state-${row.state}`}
    >
      <div className="diag-issue-row">
        <CheckboxField checked={checked} onCheckedChange={onToggle}>
          <span className="diag-issue-title">{row.title}</span>
        </CheckboxField>
        {row.state !== "open" && (
          <span
            className={`diag-state-icon diag-state-${row.state}`}
            title={t(`diagnostics.issues.state.${row.state}`)}
            aria-label={t(`diagnostics.issues.state.${row.state}`)}
          >
            <i
              className={`fas ${
                row.state === "resolved" ? "fa-circle-check" : "fa-circle-minus"
              }`}
              aria-hidden="true"
            />
          </span>
        )}
        <span className="diag-issue-count">×{row.count}</span>
        <span className="diag-issue-time">{formatStamp(row.last_seen)}</span>
        <button
          type="button"
          className="diag-expand"
          aria-expanded={expanded}
          aria-label={
            expanded ? t("diagnostics.issues.hide") : t("diagnostics.issues.show")
          }
          title={expanded ? t("diagnostics.issues.hide") : t("diagnostics.issues.show")}
          onClick={onOpen}
        >
          <i
            className={`fas fa-chevron-${expanded ? "up" : "down"}`}
            aria-hidden="true"
          />
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
          {/* Icon-only so the three fit on one row at phone width; the label stays as the tooltip
              and the accessible name. */}
          <div className="diag-issue-actions">
            <button
              type="button"
              className="diag-state-btn"
              title={t("diagnostics.issues.resolve")}
              aria-label={t("diagnostics.issues.resolve")}
              onClick={() => onResolve("resolved")}
            >
              <i className="fas fa-circle-check" aria-hidden="true" />
            </button>
            <button
              type="button"
              className="diag-state-btn"
              title={t("diagnostics.issues.ignore")}
              aria-label={t("diagnostics.issues.ignore")}
              onClick={() => onResolve("ignored")}
            >
              <i className="fas fa-eye-slash" aria-hidden="true" />
            </button>
            <button
              type="button"
              className="diag-state-btn"
              title={t("diagnostics.issues.reopen")}
              aria-label={t("diagnostics.issues.reopen")}
              onClick={() => onResolve("open")}
            >
              <i className="fas fa-rotate-left" aria-hidden="true" />
            </button>
          </div>
        </div>
      )}
    </div>
  )
}
