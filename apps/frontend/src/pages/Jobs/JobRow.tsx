import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { JobRecord, JobRecordState } from "@/api/types"
import { CodeBlock } from "@/components/Display"
import { JobProgressBar, STATE_COLOR } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

/** State → i18n key. */
export const STATE_LABEL_KEYS: Record<JobRecordState, string> = {
  queued: "Queued",
  active: "Active",
  finished: "Finished",
  failed: "Failed",
}

export const isTerminal = (s: JobRecordState) => s === "finished" || s === "failed"

/** One job row + expandable detail. Only terminal jobs are checkbox-selectable for batch clear. */
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
            aria-label={t("jobs.selectJob") ?? undefined}
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
          {/* `inlineLabel`: the reading sits inside the bar instead of in a fixed 224px column
              beside it. In a table cell that fixed column is what made the table wider than the
              page (the header cell and the row overlapped, with a horizontal scrollbar under
              them), because a table's min-content width counts every non-shrinkable piece. */}
          <JobProgressBar job={job} color={STATE_COLOR[job.state]} inlineLabel />
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
          {/* `maxWidth: 0` is the only thing that stops a table cell from growing to fit a long
              result: without it the JSON block widens the whole table past the page (reported
              from a phone 2026-11-06), because `max-width: 100%` on the inner `<pre>` resolves
              against that same content-sized cell. `overflow: hidden` + the inner `overflowX:
              auto` keeps the overflow inside the cell instead. */}
          <td colSpan={4} style={{ maxWidth: 0 }}>
            <div style={{ overflowX: "auto" }}>
              <JobDetail job={job} />
            </div>
          </td>
        </tr>
      )}
    </>
  )
}

/** A finished job's result or a failed job's error, both already in the `GET /jobs` response. */
function JobDetail({ job }: { job: JobRecord }) {
  const { t } = useTranslation()
  if (job.state === "failed") {
    return (
      <div style={{ fontSize: FONT_SIZE_SM }}>
        <strong style={{ color: STATE_COLOR.failed }}>{t("jobs.error")}: </strong>
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
          {job.error || t("jobs.noErrorMessageCaptured")}
        </pre>
      </div>
    )
  }
  if (job.state === "finished") {
    const code = job.result_truncated
      ? t("jobs.resultTooLargeToDisplay")
      : job.result == null
        ? t("jobs.noResult")
        : typeof job.result === "string"
          ? job.result
          : JSON.stringify(job.result, null, 2)
    return (
      <div style={{ fontSize: FONT_SIZE_SM }}>
        <strong>{t("jobs.result")}: </strong>
        <div style={{ marginTop: 4 }}>
          <CodeBlock code={code} language="json" />
        </div>
      </div>
    )
  }
  return (
    <div style={{ fontSize: FONT_SIZE_SM, color: STATE_COLOR.finished }}>
      {t("jobs.thisJobIsStillRunning")}
    </div>
  )
}
