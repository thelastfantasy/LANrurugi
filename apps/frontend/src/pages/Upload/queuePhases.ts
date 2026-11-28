import type { DownloadPhaseName, JobPhase } from "@/api/types"

/** The three buckets a queue row actually shows: the contract's six phases fold into them
 * because `negotiate`/`waiting` are waits on the source site (the user reads them as queuing) and
 * `publish` is the same cataloguing work as `ingest`. */
export type PhaseKey = "queued" | "transfer" | "ingest"

/** The job's phase timeline folded into the three figures a row shows. */
export interface PhaseBreakdown {
  queuedMs: number
  transferMs: number
  ingestMs: number
  /** First phase's start to the last one's close (or `nowMs` while one is still open). */
  totalMs: number
  /** Timeline the phases do not account for — a contract violation rather than a figure to show,
   * kept so a gap shows up as a difference instead of silently vanishing. */
  unattributedMs: number
  /** The phase currently open, if any. A job whose every phase is closed has none. */
  currentPhase: PhaseKey | null
}

const PHASE_KEY: Record<DownloadPhaseName, PhaseKey> = {
  queued: "queued",
  negotiate: "queued",
  waiting: "queued",
  transfer: "transfer",
  ingest: "ingest",
  publish: "ingest",
}

/** Whether there is a timeline to read at all — a maintenance job, an older server and an empty
 * array all have none, and every caller then falls back to its own coarser signal. */
export function hasPhases(phases: readonly JobPhase[] | null | undefined): boolean {
  return !!phases && phases.length > 0
}

/** The phase currently open in `phases`, for callers that need only that and not the durations. */
export function currentPhaseOf(phases: readonly JobPhase[] | null | undefined): PhaseKey | null {
  let current: PhaseKey | null = null
  for (const phase of phases ?? []) {
    if (phase.ended_at == null) current = PHASE_KEY[phase.name] ?? null
  }
  return current
}

/**
 * Folds `job.phases` into the three figures a queue row shows, or `null` when there is no timeline
 * to show at all. Callers must treat `null` as "render the older, coarser line" rather than as
 * "all zeros".
 *
 * `nowMs` closes the still-open phase for a running job; the row re-renders on the same tick that
 * moves its elapsed time, so the open segment advances with it.
 */
export function foldPhases(
  phases: readonly JobPhase[] | null | undefined,
  nowMs: number,
): PhaseBreakdown | null {
  if (!hasPhases(phases)) return null
  const list: JobPhase[] = [...(phases ?? [])]
  // Sorted defensively: the server emits the timeline oldest-first, but a fold must not depend on
  // an order it does not enforce itself.
  const ordered = list.sort((a, b) => a.started_at - b.started_at)
  let queuedMs = 0
  let transferMs = 0
  let ingestMs = 0
  const startMs = ordered[0].started_at
  let endMs = startMs
  for (const phase of ordered) {
    const endedAt = phase.ended_at ?? Math.max(nowMs, phase.started_at)
    const duration = Math.max(0, endedAt - phase.started_at)
    const key = PHASE_KEY[phase.name]
    if (key === "queued") queuedMs += duration
    else if (key === "transfer") transferMs += duration
    else if (key === "ingest") ingestMs += duration
    endMs = Math.max(endMs, endedAt)
  }
  const totalMs = Math.max(0, endMs - startMs)
  return {
    queuedMs,
    transferMs,
    ingestMs,
    totalMs,
    unattributedMs: Math.max(0, totalMs - queuedMs - transferMs - ingestMs),
    currentPhase: currentPhaseOf(ordered),
  }
}

/**
 * Whether the row is cataloguing right now. The phase timeline knows exactly; without one, the
 * older signal is `transferring === false` while the item still says `downloading` (only an
 * explicit `false` counts — a server predating the field omits it).
 */
export function ingestingNow(
  phases: readonly JobPhase[] | null | undefined,
  transferring: boolean | undefined,
): boolean {
  if (hasPhases(phases)) return currentPhaseOf(phases) === "ingest"
  return transferring === false
}
