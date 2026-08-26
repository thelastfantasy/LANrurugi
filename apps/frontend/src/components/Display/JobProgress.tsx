import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { useJobProgressOverride } from "@/api/hooks"
import type { JobRecord, JobRecordState } from "@/api/types"
import { Tooltip } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

/** State → color, shared between the Jobs page's own badges/borders and this component's default
 * bar color (so a state's color only needs to change in one place). */
export const STATE_COLOR: Record<JobRecordState, string> = {
  queued: "rgb(66, 133, 244)",
  active: "rgb(26, 165, 26)",
  finished: "rgb(120, 120, 120)",
  failed: "rgb(207, 37, 37)",
}

const BAR_HEIGHT = 8
const BAR_BACKGROUND = "rgba(128,128,128,0.25)"
const ROW_GAP = 6
// A download subject to a configured rate limit gets its speed label rendered in this distinct
// amber (vs. the default inherited text color) so a throttled transfer is visually distinguishable
// from an unrestricted one at a glance (issue #2).
const RATE_LIMITED_SPEED_COLOR = "rgb(230, 126, 34)"
// Below this, a speed reading is more poll-jitter than signal (e.g. two ticks landing 50ms apart
// due to render timing, not the actual poll interval) — showing "0.0 B/s" or a wildly inflated
// number from a near-zero time delta is worse than just omitting the speed for that one tick.
const MIN_INTERVAL_FOR_SPEED_MS = 500

/** Formats a byte count as a short human-readable string (`45.3 MB`) — 1024-based (binary)
 * divisions, matching every real legacy byte-size conversion, labeled `KB`/`MB`/`GB` per that same
 * convention (not the technically-correct `KiB`/`MiB`/`GiB`, which legacy never uses either). One
 * decimal place from `MB` upward — whole KB values are already meaningful signal, but a whole
 * number at MB+ hides a real difference in transfer rate (`1 MB/s` vs `1.9 MB/s`). */
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

/** Download-job progress: real, incrementally-updating byte progress instead of the plain
 * queued→finished jump every other job type still uses. Three cases:
 * - `downloaded_bytes` + `total_bytes` both known → a determinate bar sized by the real ratio,
 *   labeled with both byte counts rather than just a percentage.
 * - `downloaded_bytes` known but `total_bytes` absent → an indeterminate spinner-style indicator,
 *   never a stuck-at-some-fraction or divide-by-zero bar.
 * - Neither present → the pre-existing plain `progress` fraction bar, unchanged.
 *
 * `color` defaults to the Jobs page's own per-state color convention when not supplied by the
 * caller. */
/** One job's last-seen `(bytes, timestamp, speed)` reading, keyed by job ID — a module-level `Map`
 * rather than a `useRef`, since the same job can be rendered by more than one `JobProgressBar`
 * instance over its lifetime (e.g. remounted rows on a poll-driven list re-sort), and speed
 * tracking needs to survive that. Entries for finished/gone jobs are harmless dead weight; nothing
 * prunes them. */
const lastReadings = new Map<string, { bytes: number; at: number; speed: number | null }>()

/** Bytes/sec since this job's own last-seen reading, or `null` if there isn't a usable prior
 * sample yet (first render for this job, or the last one was too recent — see
 * `MIN_INTERVAL_FOR_SPEED_MS`).
 *
 * Called from render (not an effect): under `<StrictMode>` React double-invokes with the same
 * `bytes`, and this component's `job` prop also re-renders on any parent poll tick, not just the
 * one that refreshes `downloaded_bytes` — most renders see byte-identical `bytes`. That "unchanged
 * since last reading" branch below returns the last actually-computed speed, not `0` — returning
 * `0` there would flicker the displayed speed down to "0 B/s" on every stale-data tick even during
 * a real, ongoing transfer. */
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
    // Too soon since the last *real* reading for a meaningful rate — update the byte count in
    // place but deliberately keep `at` (and thus `speed`) pinned to the last real reading,
    // NOT reset to `now`. SSE-pushed progress (`useJobProgressOverride`) can tick every ~200ms,
    // well under this threshold — bumping `at` on every one of those ticks would restart the
    // "time since last real reading" clock each time and it would never actually reach 500ms,
    // permanently starving the speed calculation for any job receiving faster-than-threshold
    // updates (confirmed live, 2026-08-26: speed stopped appearing entirely once progress moved
    // from the 1s `/jobs` poll, which always cleared this threshold between ticks, to SSE).
    // Keeping the *previous* `bytes` too (not the new one) means the eventual real-reading delta
    // is still measured from the last point speed was actually computed, not from this
    // in-between sample.
    lastReadings.set(jobId, { ...previous, speed: previous.speed })
    return previous.speed
  }
  const deltaBytes = bytes - previous.bytes
  if (deltaBytes < 0) {
    // A retry/restart resetting `downloaded_bytes` backwards.
    lastReadings.set(jobId, { bytes, at: now, speed: null })
    return null
  }
  const speed = (deltaBytes / elapsedMs) * 1000
  lastReadings.set(jobId, { bytes, at: now, speed })
  return speed
}

export function JobProgressBar({
  job,
  color,
  speedTooltip,
}: {
  job: JobRecord
  color?: string
  /** Hover detail for a rate-limited download (issue #2) — wraps *only* the speed figure itself,
   * not the byte-count text or the whole row, so it doesn't shadow/overlap a sibling tooltip
   * anchored to the row (e.g. the upload queue's metadata-preview tooltip on the title above this
   * bar). `undefined` renders the speed as plain text, same as before this prop existed. */
  speedTooltip?: ReactNode
}) {
  const { t } = useTranslation()
  const barColor = color ?? STATE_COLOR.active
  // SSE-pushed byte progress, merged on top of `job`'s own polled `downloaded_bytes`/`total_bytes`
  // — `useJobs()`'s 1s poll alone can miss a download's entire duration if it finishes (or the
  // relevant polling tick happens to land) awkwardly, and the poll itself was observed starving
  // for a download's whole runtime in an earlier version of this SSE wiring that wrote progress
  // straight into the `["jobs"]` query cache (see `useJobProgressOverride`'s own docs for why that
  // approach was wrong). Prefers the override's own value once *any* progress event has arrived
  // for this job — a real streamed update is always at least as fresh as the last poll.
  const override = useJobProgressOverride(job.id)
  if (override) {
    job = { ...job, downloaded_bytes: override.downloaded_bytes, total_bytes: override.total_bytes ?? job.total_bytes }
  }
  // Computed unconditionally (not just in the branches that render it) so every render feeds a
  // fresh reading into `lastReadings`, keeping the next render's delta accurate regardless of
  // which branch actually ends up displaying a speed.
  const speed = job.downloaded_bytes != null ? computeSpeed(job.id, job.downloaded_bytes) : null
  const speedLabel = speed != null ? t("components.display.s", { rate: formatBytes(speed) }) : null
  // Absent cap = unlimited (no rule matched, or the matched rule declared no `max_bytes_per_sec`);
  // a present, positive cap is what the upload-queue UI highlights + tooltips (issue #2). Only the
  // speed figure itself is colored, not the surrounding byte-count/percentage text.
  const isRateLimited = job.rate_limit_bytes_per_sec != null && job.rate_limit_bytes_per_sec > 0
  const speedColor = isRateLimited ? RATE_LIMITED_SPEED_COLOR : undefined
  const speedNode = <span style={speedColor ? { color: speedColor } : undefined}>{speedLabel}</span>

  if (job.downloaded_bytes != null && job.total_bytes != null && job.total_bytes > 0) {
    // One decimal place: a whole-number percentage visibly stalls between fast polling ticks
    // even though real progress is happening. The bar width still uses the unrounded ratio
    // directly (not `pctLabel`) so the fill itself is exact.
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
    // `total_bytes` unknown (e.g. a chunked-encoding response with no `Content-Length`) — same
    // bar/text layout as the determinate case above, but the fill can't be sized by a real ratio,
    // so it's a fixed-width block animated back and forth (an indeterminate/"activity" bar) instead
    // of a spinner icon sitting off to the side of the text.
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
