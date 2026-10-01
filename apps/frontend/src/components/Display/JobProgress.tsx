import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { useJobProgressOverride } from "@/api/hooks"
import type { JobRecord, JobRecordState } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { FONT_SIZE_SM } from "@/theme"

/** State → color, shared between the Jobs page's badges/borders and this component's default bar
 * color. */
export const STATE_COLOR: Record<JobRecordState, string> = {
  queued: "rgb(66, 133, 244)",
  active: "rgb(26, 165, 26)",
  finished: "rgb(120, 120, 120)",
  failed: "rgb(207, 37, 37)",
}

const BAR_HEIGHT = 8
const BAR_BACKGROUND = "rgba(128,128,128,0.25)"
const ROW_GAP = 6
// Marks a rate-limited download's speed label so it's visually distinguishable at a glance.
const RATE_LIMITED_SPEED_COLOR = "rgb(230, 126, 34)"
// Below this, a speed reading is more poll-jitter than signal, so it's omitted for that tick.
const MIN_INTERVAL_FOR_SPEED_MS = 500

/** Formats a byte count as a short human-readable string (`45.3 MB`), 1024-based to match legacy.
 * One decimal place from MB upward so transfer-rate differences stay visible. */
export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"]
  let value = bytes
  let unitIndex = 0
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex += 1
  }
  const decimals = unitIndex >= 2 ? 1 : 0
  return `${value.toFixed(decimals)} ${units[unitIndex]}`
}

/** Last-seen `(bytes, timestamp, speed)` per job ID. Module-level `Map`, not a `useRef`, since the
 * same job can be rendered by more than one `JobProgressBar` instance over its lifetime. */
const lastReadings = new Map<string, { bytes: number; at: number; speed: number | null }>()

/** Bytes/sec since this job's last-seen reading, or `null` with no usable prior sample. Called
 * from render (most renders see byte-identical `bytes`), so "unchanged" returns the last speed. */
function computeSpeed(jobId: string, bytes: number): number | null {
  const now = Date.now()
  const previous = lastReadings.get(jobId)
  if (previous && previous.bytes === bytes) {
    return previous.speed
  }
  if (!previous) {
    lastReadings.set(jobId, { bytes, at: now, speed: null })
    return null
  }
  const elapsedMs = now - previous.at
  if (elapsedMs < MIN_INTERVAL_FOR_SPEED_MS) {
    // Keeps `at`/`bytes` pinned to the last real reading rather than resetting on every tick.
    lastReadings.set(jobId, { ...previous, speed: previous.speed })
    return previous.speed
  }
  const deltaBytes = bytes - previous.bytes
  if (deltaBytes < 0) {
    lastReadings.set(jobId, { bytes, at: now, speed: null })
    return null
  }
  const speed = (deltaBytes / elapsedMs) * 1000
  lastReadings.set(jobId, { bytes, at: now, speed })
  return speed
}

/** Download-job progress bar: determinate ratio bar when `total_bytes` is known, an indeterminate
 * indicator when only `downloaded_bytes` is known, else the plain `progress` fraction bar. */
export function JobProgressBar({
  job,
  color,
  speedTooltip,
}: {
  job: JobRecord
  color?: string
  /** Hover detail for a rate-limited download — wraps only the speed figure, not the whole row. */
  speedTooltip?: ReactNode
}) {
  const { t } = useTranslation()
  const barColor = color ?? STATE_COLOR.active
  // SSE-pushed progress merged over the polled fields — the 1s poll alone can miss a short download.
  const override = useJobProgressOverride(job.id)
  if (override) {
    job = { ...job, downloaded_bytes: override.downloaded_bytes, total_bytes: override.total_bytes ?? job.total_bytes }
  }
  const speed = job.downloaded_bytes != null ? computeSpeed(job.id, job.downloaded_bytes) : null
  const speedLabel = speed != null ? t("components.display.s", { rate: formatBytes(speed) }) : null
  const isRateLimited = job.rate_limit_bytes_per_sec != null && job.rate_limit_bytes_per_sec > 0
  const speedColor = isRateLimited ? RATE_LIMITED_SPEED_COLOR : undefined
  const speedNode = <span style={speedColor ? { color: speedColor } : undefined}>{speedLabel}</span>

  if (job.downloaded_bytes != null && job.total_bytes != null && job.total_bytes > 0) {
    const ratio = Math.min(1, job.downloaded_bytes / job.total_bytes)
    const pctLabel = (ratio * 100).toFixed(1)
    return (
      <div style={{ display: "flex", alignItems: "center", gap: ROW_GAP }}>
        <div
          style={{
            flex: 1,
            height: BAR_HEIGHT,
            background: BAR_BACKGROUND,
            borderRadius: 4,
            overflow: "hidden",
          }}
        >
          <div style={{ width: `${ratio * 100}%`, height: "100%", background: barColor }} />
        </div>
        <span style={{ fontSize: FONT_SIZE_SM, minWidth: 120, textAlign: "right", whiteSpace: "nowrap" }}>
          {formatBytes(job.downloaded_bytes)} / {formatBytes(job.total_bytes)} ({pctLabel}%)
          {speedLabel && (
            <>
              {" · "}
              {speedTooltip ? (
                <Tooltip label={speedTooltip} wrapperStyle={{ display: "inline" }}>
                  {speedNode}
                </Tooltip>
              ) : (
                speedNode
              )}
            </>
          )}
        </span>
      </div>
    )
  }

  if (job.downloaded_bytes != null) {
    return (
      <div style={{ display: "flex", alignItems: "center", gap: ROW_GAP }}>
        <div
          style={{
            flex: 1,
            height: BAR_HEIGHT,
            background: BAR_BACKGROUND,
            borderRadius: 4,
            overflow: "hidden",
          }}
        >
          <div
            style={{
              width: "30%",
              height: "100%",
              background: barColor,
              borderRadius: 4,
              animation: "lrr-indeterminate-bar 1.2s ease-in-out infinite",
            }}
          />
        </div>
        <span style={{ fontSize: FONT_SIZE_SM, whiteSpace: "nowrap" }}>
          {t("components.display.downloaded", { size: formatBytes(job.downloaded_bytes) })}
          {speedLabel && (
            <>
              {" ("}
              {speedTooltip ? (
                <Tooltip label={speedTooltip} wrapperStyle={{ display: "inline" }}>
                  {speedNode}
                </Tooltip>
              ) : (
                speedNode
              )}
              {")"}
            </>
          )}
        </span>
      </div>
    )
  }

  const pct = Math.round(job.progress * 100)
  return (
    <div style={{ display: "flex", alignItems: "center", gap: ROW_GAP }}>
      <div
        style={{
          flex: 1,
          height: BAR_HEIGHT,
          background: BAR_BACKGROUND,
          borderRadius: 4,
          overflow: "hidden",
        }}
      >
        <div style={{ width: `${pct}%`, height: "100%", background: barColor }} />
      </div>
      <span style={{ fontSize: FONT_SIZE_SM, minWidth: 34, textAlign: "right" }}>{pct}%</span>
    </div>
  )
}
