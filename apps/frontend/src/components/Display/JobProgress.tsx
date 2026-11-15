import type { CSSProperties, ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { useJobProgressOverride } from "@/api/hooks"
import type { JobRecord, JobRecordState } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { useIsNarrowViewport } from "@/hooks"
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme"

/** State → color, shared between the Jobs page's badges/borders and this component's default bar
 * color. */
export const STATE_COLOR: Record<JobRecordState, string> = {
  queued: "rgb(66, 133, 244)",
  active: "rgb(26, 165, 26)",
  finished: "rgb(120, 120, 120)",
  failed: "rgb(207, 37, 37)",
}

const BAR_HEIGHT = 8
/** Taller than [`BAR_HEIGHT`], because the reading is rendered inside it rather than beside it. */
const INLINE_BAR_HEIGHT = 20
const BAR_BACKGROUND = "rgba(128,128,128,0.25)"
const ROW_GAP = 6
// Marks a rate-limited download's speed label so it's visually distinguishable at a glance.
const RATE_LIMITED_SPEED_COLOR = "rgb(230, 126, 34)"

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

/** A duration as a short human-readable string (`2:13`, `1:04:07`). Minutes/seconds are zero-padded
 * so a column of them stays readable; an hour or more switches to `h:mm:ss` rather than growing an
 * ambiguous third number. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const seconds = total % 60
  const minutes = Math.floor(total / 60) % 60
  const hours = Math.floor(total / 3600)
  const pad = (n: number) => String(n).padStart(2, "0")
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`
}

/**
 * Fixed width of the numeric label that sits beside every track below.
 *
 * It has to be *fixed*: sizing the label to its own text moved the track's right edge every time the
 * reading changed shape (`999.9 MB` → `1.0 GB`, `(9.9%)` → `(13.1%)`, a speed that gains a digit),
 * which reads as the progress bar itself growing and shrinking while a download runs (reported
 * live). In `em` so it follows the theme's font: a fixed 224px, measured against one theme, clipped
 * the speed under a wider one. 24em leaves room for `1023.9 MB / 1023.9 MB (100.0%) · 1023.9 MB/s`.
 */
const LABEL_WIDTH = "24em"


/** Timestamped byte readings per job, in two windows. Module-level `Map`, not a `useRef`, since
 * the same job is rendered by more than one component over its lifetime. */
const lastReadings = new Map<
  string,
  {
    /** Every update, trimmed to `SPEED_WINDOW_MS` — drives the live speed readout. */
    samples: { bytes: number; at: number }[]
    /** At most one per second, trimmed to `REMAINING_WINDOW_MS` — drives the ETA. */
    slow: { bytes: number; at: number }[]
    speed: number | null
    slowSpeed: number | null
  }
>()

/** How much history the live speed is averaged over. A single poll-to-poll delta swings with TCP
 *  bursts and the server's 200ms progress throttle; 8s keeps the reading responsive while hiding
 *  that jitter. */
const SPEED_WINDOW_MS = 8_000
/** Below this span there is not enough signal to call it a rate. */
const MIN_SPEED_SPAN_MS = 2_000
/** ~5 SSE samples/s * 8s, with headroom; bounds memory for a long transfer. */
const MAX_SPEED_SAMPLES = 64

/** The ETA gets a much longer window on purpose: it divides a large remaining byte count by the
 *  rate, so a short-window rate makes it bounce by minutes on every burst. Three minutes of history
 *  trades responsiveness for a countdown that mostly just counts down. */
const REMAINING_WINDOW_MS = 180_000
const SLOW_SAMPLE_INTERVAL_MS = 1_000
/** Below this the window is still too young to beat the live rate. */
const MIN_SLOW_SPAN_MS = 15_000
const MAX_SLOW_SAMPLES = 200

/** Rate over the span covered by `window`, or `null` when the window is shorter than `minSpan`. */
function rateOver(window: { bytes: number; at: number }[], minSpan: number): number | null {
  const first = window[0]
  const last = window[window.length - 1]
  const span = last.at - first.at
  if (span < minSpan) return null
  return ((last.bytes - first.bytes) / span) * 1000
}

/** Drops samples older than `windowMs`, keeping at least the two needed to form a span, and caps
 * the buffer so a fast progress stream cannot grow it without bound. */
function trim(
  window: { bytes: number; at: number }[],
  now: number,
  windowMs: number,
  maxSamples: number,
) {
  while (window.length > 2 && now - window[0].at > windowMs) window.shift()
  if (window.length > maxSamples) window.splice(0, window.length - maxSamples)
}

/** Records one byte reading for `jobId` (idempotent for repeated identical values) and refreshes
 * both windows. */
function recordReading(jobId: string, bytes: number) {
  const now = Date.now()
  const entry = lastReadings.get(jobId)
  if (!entry) {
    const fresh = {
      samples: [{ bytes, at: now }],
      slow: [{ bytes, at: now }],
      speed: null,
      slowSpeed: null,
    }
    lastReadings.set(jobId, fresh)
    return fresh
  }
  const last = entry.samples[entry.samples.length - 1]
  if (last.bytes === bytes) return entry
  if (bytes < last.bytes) {
    // A retry/resume reset the counter, or a stale reading arrived out of order; neither window
    // says anything about the current run.
    entry.samples = [{ bytes, at: now }]
    entry.slow = [{ bytes, at: now }]
    entry.speed = null
    entry.slowSpeed = null
    return entry
  }

  entry.samples.push({ bytes, at: now })
  trim(entry.samples, now, SPEED_WINDOW_MS, MAX_SPEED_SAMPLES)
  entry.speed = rateOver(entry.samples, MIN_SPEED_SPAN_MS) ?? entry.speed

  const lastSlow = entry.slow[entry.slow.length - 1]
  if (now - lastSlow.at >= SLOW_SAMPLE_INTERVAL_MS) {
    entry.slow.push({ bytes, at: now })
    trim(entry.slow, now, REMAINING_WINDOW_MS, MAX_SLOW_SAMPLES)
    entry.slowSpeed = rateOver(entry.slow, MIN_SLOW_SPAN_MS) ?? entry.slowSpeed
  }
  return entry
}

/** Smoothed bytes/sec over this job's last ~8s of progress, or `null` until a usable window
 * exists. Called from render, so "bytes unchanged" returns the cached speed. Callers must pass the
 * *same* byte series: mixing polled and SSE-pushed values into one job's window produces deltas that
 * go backwards on every render (and was why the displayed rate swung far wider than the transfer
 * actually did). */
export function computeSpeed(jobId: string, bytes: number): number | null {
  return recordReading(jobId, bytes).speed
}

/** The rate the remaining-time estimate should divide by: ~3 minutes of history once available, the
 * live rate before that. It falls to the live rate on a fresh window because a 15s-old average
 * still bounces ("remaining" would jump by minutes on the first few samples). */
export function computeRemainingSpeed(jobId: string, bytes: number): number | null {
  const entry = recordReading(jobId, bytes)
  return entry.slowSpeed ?? entry.speed
}

/** The ETA readout's own state, kept apart from the rate windows: between re-syncs it is a plain
 *  countdown, so it needs its own clock/byte anchors. */
const etaStates = new Map<
  string,
  {
    displayedMs: number
    bytes: number
    rate: number | null
    at: number
    adjustedAt: number
  }
>()

/** Re-sync the countdown to the freshly computed estimate at most this often. */
const ETA_ADJUST_INTERVAL_MS = 10_000
/** ...or immediately once the countdown has drifted this far from the estimate. */
const ETA_MAX_DRIFT_MS = 30_000
/** ...or immediately when the rate changed by more than this fraction (a real speed change, not
 *  sample noise — the rate itself is already smoothed). */
const ETA_RATE_CHANGE_RATIO = 0.25
/** Under this the estimate is shown directly rather than counted down. */
const ETA_HIGH_PRECISION_MS = 10_000

/** Turns the estimate into the number the row shows: a one-second-per-second countdown between
 *  re-syncs, so it reads as "counting down" instead of wobbling with every sample. It re-syncs to
 *  `estimateMs` every 10s, on a >25% rate change, or when it has drifted more than 30s; it freezes
 *  while no bytes arrive (a stall must not tick the readout down to 0:00 while data is still
 *  outstanding), and under 10s it returns the estimate directly so the final seconds stay exact.
 *  Never negative.
 *
 *  `rate` is the calm rate the estimate was derived from (used only for change detection), and
 *  `bytes` anchors both the stall freeze and a retry/resume reset. Returns `null` whenever there is
 *  no estimate to show. */
export function computeEta(
  jobId: string,
  estimateMs: number | null,
  rate: number | null,
  bytes: number | null,
): number | null {
  if (estimateMs == null || bytes == null) {
    etaStates.delete(jobId)
    return null
  }
  const now = Date.now()
  const state = etaStates.get(jobId)
  if (!state || state.bytes > bytes) {
    const initial = Math.max(0, estimateMs)
    etaStates.set(jobId, { displayedMs: initial, bytes, rate, at: now, adjustedAt: now })
    return initial
  }

  if (estimateMs <= ETA_HIGH_PRECISION_MS) {
    const exact = Math.max(0, estimateMs)
    state.displayedMs = exact
    state.bytes = bytes
    state.rate = rate
    state.at = now
    state.adjustedAt = now
    return exact
  }

  const arriving = bytes > state.bytes
  let displayed = arriving ? state.displayedMs - (now - state.at) : state.displayedMs
  const rateChanged =
    state.rate != null &&
    rate != null &&
    state.rate > 0 &&
    Math.abs(rate - state.rate) / state.rate > ETA_RATE_CHANGE_RATIO
  const drifted = Math.abs(estimateMs - displayed) > ETA_MAX_DRIFT_MS
  if (now - state.adjustedAt >= ETA_ADJUST_INTERVAL_MS || rateChanged || drifted) {
    displayed = estimateMs
    state.adjustedAt = now
  }
  displayed = Math.max(0, displayed)
  state.displayedMs = displayed
  state.bytes = bytes
  state.rate = rate
  state.at = now
  return displayed
}

/** A full-width bar with its reading inside the track.
 *
 * The bar used to share its row with a fixed-width label column, which left the track only as wide
 * as whatever the column did not reserve. Splitting the reading at the fill boundary — theme text
 * colour over the empty track, white over the filled part — keeps it readable without a per-theme
 * colour: the fill is `STATE_COLOR.active` in every theme, and the done row already writes white on
 * that same green. The indeterminate variants keep the theme colour throughout (their moving 30%
 * block is rendered at low opacity, so the text stays legible over it). */
function InlineBar({
  ratio,
  label,
  color,
  flowing = false,
}: {
  ratio: number | null
  label: string
  color: string
  /** Animate the fill while the transfer is actually running; a paused/finished row must stay still. */
  flowing?: boolean
}) {
  const pct = ratio == null ? 0 : Math.max(0, Math.min(1, ratio)) * 100
  const textStyle: CSSProperties = {
    position: "absolute",
    inset: 0,
    display: "flex",
    alignItems: "center",
    // Right-aligned, not centered: the fill grows from the left, so a centred reading is cut by the
    // fill boundary through most of the transfer, while a right-aligned one only meets it late.
    // Numbers also read naturally right-aligned.
    justifyContent: "flex-end",
    padding: "0 10px",
    boxSizing: "border-box",
    fontSize: FONT_SIZE_XS,
    lineHeight: 1,
    whiteSpace: "nowrap",
    overflow: "hidden",
    fontVariantNumeric: "tabular-nums",
  }
  return (
    <div
      style={{
        position: "relative",
        width: "100%",
        height: INLINE_BAR_HEIGHT,
        background: BAR_BACKGROUND,
        borderRadius: 4,
        overflow: "hidden",
      }}
    >
      {ratio == null ? (
        <div
          style={{
            position: "absolute",
            top: 0,
            bottom: 0,
            left: 0,
            width: "30%",
            background: color,
            opacity: 0.35,
            borderRadius: 4,
            animation: "lrr-indeterminate-bar 1.2s ease-in-out infinite",
          }}
        />
      ) : (
        <div
          style={{
            position: "absolute",
            top: 0,
            bottom: 0,
            left: 0,
            width: `${pct}%`,
            backgroundColor: color,
            // Clips the stripe layer, which is deliberately one tile wider than the fill.
            overflow: "hidden",
          }}
        >
          {flowing ? <div className="lrr-flowing-fill" /> : null}
        </div>
      )}
      {ratio == null ? (
        <span style={textStyle}>{label}</span>
      ) : (
        <>
          <span style={{ ...textStyle, clipPath: `inset(0 0 0 ${pct}%)` }}>{label}</span>
          <span
            aria-hidden="true"
            style={{
              ...textStyle,
              color: "#fff",
              clipPath: `inset(0 ${100 - pct}% 0 0)`,
            }}
          >
            {label}
          </span>
        </>
      )}
    </div>
  )
}

/** Download-job progress bar: determinate ratio bar when `total_bytes` is known, an indeterminate
 * indicator when only `downloaded_bytes` is known, else the plain `progress` fraction bar. */
export function JobProgressBar({
  job,
  color,
  speedTooltip,
  pendingLabel,
  inlineLabel,
  readingLabel,
}: {
  job: JobRecord
  color?: string
  /** Hover detail for a rate-limited download — wraps only the speed figure, not the whole row. */
  speedTooltip?: ReactNode
  /** Shown instead of the `progress` fallback while the job is running but no byte has arrived
   *  yet — a server can spend minutes preparing an archive before it sends anything, and a
   *  `0%` bar for that window reads as stalled. */
  pendingLabel?: string
  /** Renders the reading inside a full-width bar instead of in a fixed-width column beside a thin
   *  track (only meaningful for the plain, tooltip-free readings). */
  inlineLabel?: boolean
  /** Replaces the byte reading with a phase description. Used once the transfer itself is done but
   *  the job is still busy (cataloguing/writing the archive), where "1.4 GB / 1.4 GB (100%)" is
   *  technically true and completely uninformative. */
  readingLabel?: string
}) {
  const { t } = useTranslation()
  // Below `useIsNarrowViewport`'s 640px the label goes *under* the track instead of beside it: 224px
  // of fixed label on a phone-width row would leave the track a sliver and start clipping the
  // numbers, so the narrow layout keeps the track full-width and puts the reading on its own line —
  // still a constant-length bar, with nothing truncated.
  const narrow = useIsNarrowViewport()
  const barColor = color ?? STATE_COLOR.active
  // SSE-pushed progress merged over the polled fields — the 1s poll alone can miss a short download.
  const override = useJobProgressOverride(job.id)
  if (override) {
    job = { ...job, downloaded_bytes: override.downloaded_bytes, total_bytes: override.total_bytes ?? job.total_bytes }
  }
  // Only while the task is running: `computeSpeed` keeps returning its last sample for unchanged
  // bytes, so a finished/failed (or paused) job would otherwise display a stale rate.
  const speed =
    job.state === "active" && job.downloaded_bytes != null
      ? computeSpeed(job.id, job.downloaded_bytes)
      : null
  const speedLabel = speed != null ? t("components.display.s", { rate: formatBytes(speed) }) : null
  const isRateLimited = job.rate_limit_bytes_per_sec != null && job.rate_limit_bytes_per_sec > 0
  const speedColor = isRateLimited ? RATE_LIMITED_SPEED_COLOR : undefined
  const speedNode = <span style={speedColor ? { color: speedColor } : undefined}>{speedLabel}</span>

  /** The numeric label's own style: fixed and right-aligned beside the track on a wide row, plain
   *  full-width under it on a narrow one. Either way the *track* never resizes because of it. */
  // Narrow: no fixed width and no `nowrap`, so a reading wider than the row wraps onto a second
  // line instead of hiding a speed or a byte count.
  const labelStyle: CSSProperties = narrow
    ? { fontSize: FONT_SIZE_SM }
    : {
        fontSize: FONT_SIZE_SM,
        width: LABEL_WIDTH,
        flex: "0 0 auto",
        textAlign: "right",
        whiteSpace: "nowrap",
        // Degrades by clipping rather than by pushing the track: an over-long reading must never
        // change the bar's length.
        overflow: "hidden",
        textOverflow: "ellipsis",
      }
  /** The track itself. `flex: 1` only in the row layout: the narrow layout is a *column*, where
   *  `flex: 1` applies to the vertical axis, whose `flex-basis: 0` overrides `height` and collapses
   *  the track to nothing — the bar disappeared entirely on a phone (reported live). Width comes
   *  from the column's own `align-items: stretch` there. */
  const trackStyle: CSSProperties = {
    height: BAR_HEIGHT,
    background: BAR_BACKGROUND,
    borderRadius: 4,
    overflow: "hidden",
    ...(narrow ? { width: "100%" } : { flex: 1 }),
  }
  /** Row wrapper shared by every branch — column on a narrow viewport, so the track keeps the full
   *  row width and the label is never squeezed. */
  const rowStyle: CSSProperties = {
    display: "flex",
    flexDirection: narrow ? "column" : "row",
    alignItems: narrow ? "stretch" : "center",
    gap: ROW_GAP,
  }

  if (job.downloaded_bytes != null && job.total_bytes != null && job.total_bytes > 0) {
    const ratio = Math.min(1, job.downloaded_bytes / job.total_bytes)
    const pctLabel = (ratio * 100).toFixed(1)
    const reading = `${formatBytes(job.downloaded_bytes)} / ${formatBytes(job.total_bytes)} (${pctLabel}%)`
    if (inlineLabel) {
      return (
        <InlineBar
          ratio={ratio}
          label={readingLabel ?? (speedLabel ? `${reading} · ${speedLabel}` : reading)}
          color={barColor}
          flowing={job.state === "active"}
        />
      )
    }
    return (
      <div style={rowStyle}>
        <div style={trackStyle}>
          <div style={{ width: `${ratio * 100}%`, height: "100%", background: barColor }} />
        </div>
        <span style={labelStyle}>
          {readingLabel ?? (
            <>
              {formatBytes(job.downloaded_bytes)} / {formatBytes(job.total_bytes)} ({pctLabel}%)
            </>
          )}
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
    const reading = t("components.display.downloaded", {
      size: formatBytes(job.downloaded_bytes),
    })
    if (inlineLabel) {
      return (
        <InlineBar
          ratio={null}
          label={readingLabel ?? (speedLabel ? `${reading} · ${speedLabel}` : reading)}
          color={barColor}
        />
      )
    }
    return (
      <div style={rowStyle}>
        <div style={trackStyle}>
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
        <span style={labelStyle}>
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

  // No byte has arrived yet: `progress` is still 0 by construction, so the fraction branch below
  // would render a stalled-looking 0% for however long the server takes to send its first byte.
  if (pendingLabel != null) {
    if (inlineLabel) {
      return <InlineBar ratio={null} label={pendingLabel} color={barColor} />
    }
    return (
      <div style={rowStyle}>
        <div style={trackStyle}>
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
        <span style={labelStyle}>{pendingLabel}</span>
      </div>
    )
  }

  const pct = Math.round(job.progress * 100)
  return (
    <div style={rowStyle}>
      <div style={trackStyle}>
        <div style={{ width: `${pct}%`, height: "100%", background: barColor }} />
      </div>
      <span style={labelStyle}>{pct}%</span>
    </div>
  )
}
