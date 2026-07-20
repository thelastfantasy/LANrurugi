import { useTranslation } from 'react-i18next'

import type { JobRecord, JobRecordState } from '../api/types'
import { FONT_SIZE_10PT } from '../theme'

/** State → color, shared between the Jobs page's own badges/borders and this component's default
 * bar color (so a state's color only needs to change in one place). */
export const STATE_COLOR: Record<JobRecordState, string> = {
  queued: 'rgb(66, 133, 244)',
  active: 'rgb(26, 165, 26)',
  finished: 'rgb(120, 120, 120)',
  failed: 'rgb(207, 37, 37)',
}

const BAR_HEIGHT = 8
const BAR_BACKGROUND = 'rgba(128,128,128,0.25)'
const ROW_GAP = 6

/** Formats a byte count as a short human-readable string (`45.3 MB`) — decimal (1000-based) units,
 * matching how file sizes are already shown elsewhere in this app (Library page's archive size
 * column), not binary (1024-based) KiB/MiB. */
export function formatBytes(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unitIndex = 0
  while (value >= 1000 && unitIndex < units.length - 1) {
    value /= 1000
    unitIndex += 1
  }
  const decimals = unitIndex === 0 ? 0 : 1
  return `${value.toFixed(decimals)} ${units[unitIndex]}`
}

/** Download-job progress (specs/005-download-plugin-progress, US1): real, incrementally-updating
 * byte progress instead of the plain queued→finished jump every other job type still uses.
 * Three cases (spec FR-001/FR-002):
 * - `downloaded_bytes` + `total_bytes` both known → a determinate bar sized by the real ratio,
 *   labeled with both byte counts (e.g. "45.2 MB / 120.0 MB") rather than just a percentage —
 *   more informative for a multi-hundred-MB archive than "38%" alone.
 * - `downloaded_bytes` known but `total_bytes` absent (server didn't report a size) → an
 *   indeterminate spinner-style indicator, never a stuck-at-some-fraction or divide-by-zero bar.
 * - Neither present (every non-download job, or a download job that hasn't started transferring
 *   bytes yet) → the pre-existing plain `progress` fraction bar, unchanged.
 *
 * `color` defaults to the Jobs page's own per-state color convention when a `job.state`-derived
 * value isn't supplied by the caller (e.g. the Upload page's download-queue panel, which has no
 * equivalent per-state color scheme of its own).
 */
export function JobProgressBar({ job, color }: { job: JobRecord; color?: string }) {
  const { t } = useTranslation()
  const barColor = color ?? STATE_COLOR.active

  if (job.downloaded_bytes != null && job.total_bytes != null && job.total_bytes > 0) {
    const pct = Math.min(100, Math.round((job.downloaded_bytes / job.total_bytes) * 100))
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: ROW_GAP }}>
        <div
          style={{
            flex: 1,
            height: BAR_HEIGHT,
            background: BAR_BACKGROUND,
            borderRadius: 4,
            overflow: 'hidden',
          }}
        >
          <div style={{ width: `${pct}%`, height: '100%', background: barColor }} />
        </div>
        <span style={{ fontSize: FONT_SIZE_10PT, minWidth: 90, textAlign: 'right', whiteSpace: 'nowrap' }}>
          {formatBytes(job.downloaded_bytes)} / {formatBytes(job.total_bytes)}
        </span>
      </div>
    )
  }

  if (job.downloaded_bytes != null) {
    return (
      <div style={{ display: 'flex', alignItems: 'center', gap: ROW_GAP, justifyContent: 'center' }}>
        <i className="fa fa-circle-notch fa-spin" aria-hidden="true"></i>
        <span style={{ fontSize: FONT_SIZE_10PT, whiteSpace: 'nowrap' }}>
          {t('{{size}} downloaded', { size: formatBytes(job.downloaded_bytes) })}
        </span>
      </div>
    )
  }

  const pct = Math.round(job.progress * 100)
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: ROW_GAP }}>
      <div
        style={{
          flex: 1,
          height: BAR_HEIGHT,
          background: BAR_BACKGROUND,
          borderRadius: 4,
          overflow: 'hidden',
        }}
      >
        <div style={{ width: `${pct}%`, height: '100%', background: barColor }} />
      </div>
      <span style={{ fontSize: FONT_SIZE_10PT, minWidth: 34, textAlign: 'right' }}>{pct}%</span>
    </div>
  )
}
