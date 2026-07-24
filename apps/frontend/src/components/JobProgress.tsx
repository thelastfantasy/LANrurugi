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
// Below this, a speed reading is more poll-jitter than signal (e.g. two ticks landing 50ms apart
// due to render timing, not the actual poll interval) — showing "0.0 B/s" or a wildly inflated
// number from a near-zero time delta is worse than just omitting the speed for that one tick.
const MIN_INTERVAL_FOR_SPEED_MS = 500

/** Formats a byte count as a short human-readable string (`45.3 MB`) — 1024-based (binary)
 * divisions, matching every real legacy byte-size conversion found in `~/LANraragi` (`common.js`'s
 * `imgSize = ... / 1024`, `reader.js`'s identical preload-size calc, `Model/Reader.pm`'s resize
 * threshold check, `duplicates.html.tt2`'s `archive.arcsize / (1024 * 1024)`) — labeled `KB`/`MB`/
 * `GB` per that same convention (not the technically-correct `KiB`/`MiB`/`GiB`, which legacy never
 * uses either). One decimal place from `MB` upward (explicit user call: a KB/s-range speed reading
 * jittering between whole KB values is already meaningful signal, a fractional KB adds noise
 * without adding precision anyone reads at a glance; MB and up is where a whole number alone hides
 * a real difference in transfer rate, e.g. `1 MB/s` vs `1.9 MB/s`). */
export function formatBytes(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB']
  let value = bytes
  let unitIndex = 0
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024
    unitIndex += 1
  }
  const decimals = unitIndex >= 2 ? 1 : 0
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
/** One job's last-seen `(bytes, timestamp, speed)` reading, keyed by job ID — a plain module-level
 * `Map` rather than a `useRef` inside the component: the same job can be rendered by more than one
 * `JobProgressBar` instance over its lifetime (e.g. the Upload page's queue panel unmounts a row
 * once its state moves on, or the Jobs page remounts rows on every poll-driven list re-sort), and
 * speed tracking needs to survive that — a per-component ref would silently reset to "no previous
 * reading" (and so drop one speed sample) every time. Entries for finished/failed/gone jobs are
 * harmless dead weight (a handful of numbers per job ID) rather than a real leak; nothing here
 * currently prunes them, matching this component's own already-small, short-lived-process scope. */
const lastReadings = new Map<string, { bytes: number; at: number; speed: number | null }>()

/** Bytes/sec since this job's own last-seen reading, or `null` if there isn't a usable prior
 * sample yet (first render for this job, or the last one was too recent — see
 * `MIN_INTERVAL_FOR_SPEED_MS`).
 *
 * Called from render (not an effect), so under `<StrictMode>` React double-invokes the owning
 * component in dev, calling this twice back-to-back with the *same* `bytes` for what is logically
 * one render — and, independent of that, this component's own `job` prop re-renders on *any*
 * parent poll tick, not just the `useJobs()` one that actually refreshes `downloaded_bytes` (the
 * Upload page's download-queue panel polls its own list every 3s, `useJobs()` every 5s, so most
 * renders see byte-identical `bytes` to the previous call). Both cases hit the same "bytes
 * unchanged since last recorded reading" branch below — critically, that branch returns the
 * **last actually-computed speed**, not `0`: an earlier version returned `0` here, which meant
 * the displayed speed flickered down to "0 B/s" on every renders-without-fresh-data tick (i.e.
 * most of the time), even while a real, ongoing transfer was making steady progress. Returning
 * the cached speed instead keeps the display showing the last real measurement until the next
 * genuine byte-count change updates it. */
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
    // Too soon since the last *real* reading for a meaningful rate — record the new byte count
    // (so the next call's delta is measured from here, not the older, still-too-close reading)
    // but don't publish a speed yet.
    lastReadings.set(jobId, { bytes, at: now, speed: null })
    return null
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

export function JobProgressBar({ job, color }: { job: JobRecord; color?: string }) {
  const { t } = useTranslation()
  const barColor = color ?? STATE_COLOR.active
  // Computed unconditionally (not just in the branches that render it) so every render feeds a
  // fresh reading into `lastReadings`, keeping the next render's delta accurate regardless of
  // which branch actually ends up displaying a speed.
  const speed = job.downloaded_bytes != null ? computeSpeed(job.id, job.downloaded_bytes) : null
  const speedLabel = speed != null ? t('{{rate}}/s', { rate: formatBytes(speed) }) : null

  if (job.downloaded_bytes != null && job.total_bytes != null && job.total_bytes > 0) {
    // One decimal place (explicit user call, alongside the faster poll cadence above — at 1s
    // polling a whole-number percentage visibly stalls between ticks even though real progress is
    // happening underneath; a decimal keeps the readout moving) — the *bar width* still uses the
    // unrounded ratio directly (not `pctLabel`) so the fill itself is exact, not just its label.
    const ratio = Math.min(1, job.downloaded_bytes / job.total_bytes)
    const pctLabel = (ratio * 100).toFixed(1)
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
          <div style={{ width: `${ratio * 100}%`, height: '100%', background: barColor }} />
        </div>
        <span style={{ fontSize: FONT_SIZE_10PT, minWidth: 120, textAlign: 'right', whiteSpace: 'nowrap' }}>
          {formatBytes(job.downloaded_bytes)} / {formatBytes(job.total_bytes)} ({pctLabel}%)
          {speedLabel && ` · ${speedLabel}`}
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
          {speedLabel && ` (${speedLabel})`}
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
