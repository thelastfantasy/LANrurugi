import { describe, expect, it } from "vitest"

import type { JobPhase } from "@/api/types"
import { currentPhaseOf, foldPhases, ingestingNow } from "@/pages/Upload/queuePhases"

const phase = (
  name: JobPhase["name"],
  startedAt: number,
  endedAt: number | null,
): JobPhase => ({ name, started_at: startedAt, ended_at: endedAt })

describe("foldPhases", () => {
  it("returns null when the job reports no timeline at all", () => {
    expect(foldPhases(undefined, 1000)).toBeNull()
    expect(foldPhases(null, 1000)).toBeNull()
    expect(foldPhases([], 1000)).toBeNull()
  })

  it("folds negotiate and waiting into the queued figure", () => {
    // The 916 MB sample from the report: 13:03 waiting its turn, 4:27 transferring, then ingest.
    const breakdown = foldPhases(
      [
        phase("queued", 0, 0),
        phase("negotiate", 0, 60_000),
        phase("waiting", 60_000, 783_000),
        phase("transfer", 783_000, 1_050_000),
        phase("ingest", 1_050_000, 1_056_000),
      ],
      2_000_000,
    )
    expect(breakdown).not.toBeNull()
    expect(breakdown?.queuedMs).toBe(783_000)
    expect(breakdown?.transferMs).toBe(267_000)
    expect(breakdown?.ingestMs).toBe(6_000)
    expect(breakdown?.totalMs).toBe(1_056_000)
    expect(breakdown?.unattributedMs).toBe(0)
    expect(breakdown?.currentPhase).toBeNull()
  })

  it("keeps a still-open phase open and advances it with the clock", () => {
    const phases = [phase("queued", 0, 1_000), phase("transfer", 1_000, null)]
    const early = foldPhases(phases, 5_000)
    const later = foldPhases(phases, 9_000)
    expect(early?.transferMs).toBe(4_000)
    expect(later?.transferMs).toBe(8_000)
    expect(early?.currentPhase).toBe("transfer")
    // A `publish` phase still counts as ingesting.
    expect(foldPhases([phase("ingest", 0, 100), phase("publish", 100, null)], 500)?.currentPhase).toBe(
      "ingest",
    )
  })

  it("folds publish into the ingest figure", () => {
    const breakdown = foldPhases([phase("ingest", 0, 400), phase("publish", 400, 1_000)], 2_000)
    expect(breakdown?.ingestMs).toBe(1_000)
    expect(breakdown?.queuedMs).toBe(0)
    expect(breakdown?.transferMs).toBe(0)
  })

  it("does not depend on the array already being in order", () => {
    const shuffled = [
      phase("transfer", 1_000, 2_000),
      phase("queued", 0, 1_000),
      phase("ingest", 2_000, 2_500),
    ]
    const breakdown = foldPhases(shuffled, 9_000)
    expect(breakdown?.queuedMs).toBe(1_000)
    expect(breakdown?.transferMs).toBe(1_000)
    expect(breakdown?.ingestMs).toBe(500)
    expect(breakdown?.totalMs).toBe(2_500)
  })

  it("counts a timeline gap as unattributed rather than folding it into a phase", () => {
    const breakdown = foldPhases([phase("queued", 0, 1_000), phase("ingest", 4_000, 5_000)], 9_000)
    expect(breakdown?.totalMs).toBe(5_000)
    expect(breakdown?.unattributedMs).toBe(3_000)
  })
})

describe("ingestingNow", () => {
  it("prefers the phase timeline over the coarser transferring flag", () => {
    const transferring = [phase("transfer", 0, null)]
    const ingest = [phase("ingest", 0, null)]
    // A server reporting an open transfer phase while its own flag still says `false` is stale —
    // the timeline is the finer-grained truth.
    expect(ingestingNow(transferring, false)).toBe(false)
    expect(ingestingNow(ingest, true)).toBe(true)
  })

  it("falls back to the transferring flag when there is no timeline", () => {
    expect(ingestingNow(null, false)).toBe(true)
    expect(ingestingNow(undefined, true)).toBe(false)
    expect(ingestingNow([], undefined)).toBe(false)
  })
})

describe("currentPhaseOf", () => {
  it("reports the open phase and null when every phase is closed", () => {
    expect(currentPhaseOf([phase("queued", 0, 100), phase("transfer", 100, null)])).toBe("transfer")
    expect(currentPhaseOf([phase("queued", 0, 100)])).toBeNull()
    expect(currentPhaseOf(null)).toBeNull()
  })
})
