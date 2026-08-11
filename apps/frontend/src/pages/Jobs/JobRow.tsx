import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { JobRecord, JobRecordState } from "@/api/types"
import { CodeBlock } from "@/components/Display"
import { JobProgressBar, STATE_COLOR } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

/** State → i18n key (bare English word, added to the locale files in T024). */
export const STATE_LABEL_KEYS: Record<JobRecordState, string> = {
  queued: "Queued",
  active: "Active",
  finished: "Finished",
  failed: "Failed",
}

export const isTerminal = (s: JobRecordState) => s === "finished" || s === "failed"

/** One job row + its expandable detail (US2/T010). The leading checkbox drives multi-select clear
 * (US3); only terminal jobs are selectable, others render a disabled checkbox so the row still
 * aligns under the select-all column. No per-row action button — clearing is via the toolbar. */
export function JobRow({
  job,
  selected,
  onToggleSelect,
}: {
  job: JobRecord
  selected: boolean
  onToggleSelect: () => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const selectable = isTerminal(job.state)

  return (
    <>
      <tr className={open ? "gtr1" : undefined}>
        <td style={{ textAlign: "center" }}>
          <input
            type="checkbox"
            checked={selected}
            disabled={!selectable}
            onChange={onToggleSelect}
            aria-label={t("Select job") ?? undefined}
          />
        </td>
        <td
          onClick={() => setOpen((o) => !o)}
          style={{ cursor: "pointer", wordBreak: "break-word", textAlign: "left" }}
        >
          {job.name}
        </td>
        <td>
          <span style={{ color: STATE_COLOR[job.state], fontWeight: "bold" }}>
            {t(STATE_LABEL_KEYS[job.state])}
          </span>
        </td>
        <td>
          <JobProgressBar job={job} color={STATE_COLOR[job.state]} />
        </td>
        <td
          style={{ textAlign: "center", cursor: "pointer" }}
          onClick={() => setOpen((o) => !o)}
        >
          <i className={`fa fa-caret-${open ? "down" : "right"}`} aria-hidden="true"></i>
        </td>
      </tr>
      {open && (
        <tr>
          <td></td>
          <td colSpan={4}>
            <JobDetail job={job} />
          </td>
        </tr>
      )}
    </>
  )
}

/** US2 (T010): a finished job's result (syntax-highlighted JSON via `CodeBlock`), or a failed job's
 * specific error (a red-tinted panel) — the data is already in `GET /jobs`'s response, so no extra
 * fetch is needed. */
function JobDetail({ job }: { job: JobRecord }) {
  const { t } = useTranslation()
  if (job.state === "failed") {
    return (
      <div style={{ fontSize: FONT_SIZE_SM }}>
        <strong style={{ color: STATE_COLOR.failed }}>{t("Error")}: </strong>
        <pre
          style={{
            margin: "4px 0 0",
            padding: "8px 12px",
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
            color: STATE_COLOR.failed,
            background: "rgba(207, 37, 37, 0.08)",
            borderLeft: `3px solid ${STATE_COLOR.failed}`,
            borderRadius: 4,
          }}
        >
          {job.error || t("(no error message captured)")}
        </pre>
      </div>
    )
  }
  if (job.state === "finished") {
    const code =
      job.result == null
        ? t("(no result)")
        : typeof job.result === "string"
          ? job.result
          : JSON.stringify(job.result, null, 2)
    return (
      <div style={{ fontSize: FONT_SIZE_SM }}>
        <strong>{t("Result")}: </strong>
        <div style={{ marginTop: 4 }}>
          <CodeBlock code={code} language="json" />
        </div>
      </div>
    )
  }
  return (
    <div style={{ fontSize: FONT_SIZE_SM, color: STATE_COLOR.finished }}>
      {t("This job is still running — no result yet.")}
    </div>
  )
}
