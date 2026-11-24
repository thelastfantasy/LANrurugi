import type { ReactNode } from "react"

/** The one progress bar every backup-page job uses (snapshot/export creation, export restore, and
 *  both import flows). The track/fill styles live in `index.css` (`.backup-progress-*`), shared with
 *  the rest of the page; callers only supply the numbers and the status wording. */
export function BackupProgressBar({
  percent,
  label,
  detail,
  note,
}: {
  /** 0–100, one decimal. Also mirrored into `aria-valuenow`. */
  percent: number
  /** The running/done/failed sentence ("生成中… 62.4%（已用 8.1 秒）"). */
  label: ReactNode
  /** The byte reading ("4.1 MiB / 6.6 MiB"), rendered on its own line on a phone. */
  detail?: ReactNode
  /** Extra state, e.g. the "live progress unavailable" fallback notice. */
  note?: ReactNode
}) {
  return (
    <div className="backup-run-progress">
      <div
        className="backup-progress-track"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Number(percent.toFixed(1))}
      >
        <div className="backup-progress-fill" style={{ width: `${percent}%` }} />
      </div>
      <p className="backup-run-status">
        <span>{label}</span>
        {detail != null && <span>{detail}</span>}
        {note != null && <span>{note}</span>}
      </p>
    </div>
  )
}
