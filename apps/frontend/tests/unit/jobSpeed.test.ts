import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  computeEta,
  computeRemainingSpeed,
  computeSpeed,
} from "@/components/Display/JobProgress"

const MB = 1024 * 1024

describe("download speed smoothing", () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(1_000_000)
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it("keeps the ETA rate calm when a burst inflates the live rate", () => {
    // One MB/s for 30s, then a 20MB burst in one tick: a real transfer hiccup.
    const job = "eta-smoothing"
    computeSpeed(job, 0)
    for (let second = 1; second <= 30; second++) {
      vi.advanceTimersByTime(1_000)
      computeSpeed(job, second * MB)
    }
    vi.advanceTimersByTime(1_000)
    const burstBytes = 31 * MB + 20 * MB
    computeSpeed(job, burstBytes)
    vi.advanceTimersByTime(1_000)
    const nextBytes = burstBytes + MB

    // The live readout (8s) reacts to the burst...
    expect(computeSpeed(job, nextBytes)).toBeGreaterThan(3 * MB)
    // ...while the ETA's ~3-minute window barely moves: ~52MB over 32s.
    expect(computeRemainingSpeed(job, nextBytes)).toBeCloseTo((52 * MB) / 32, -5)
  })

  it("starts a fresh window when a retry or resume resets the byte counter", () => {
    const job = "eta-reset"
    computeSpeed(job, 100 * MB)
    vi.advanceTimersByTime(1_000)
    computeSpeed(job, 101 * MB)
    // A resumed transfer starts at 0 again: the old window must not produce a negative speed.
    vi.advanceTimersByTime(1_000)
    expect(computeSpeed(job, 0)).toBeNull()
    expect(computeRemainingSpeed(job, 0)).toBeNull()
  })
})

describe("remaining-time countdown", () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(1_000_000)
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it("counts down one second per second and re-syncs on the 10s cadence", () => {
    const job = "eta-countdown"
    expect(computeEta(job, 60_000, 1_000_000, 0)).toBe(60_000)

    for (let second = 1; second <= 4; second++) {
      vi.advanceTimersByTime(1_000)
      expect(computeEta(job, 60_000 - second * 1_000, 1_000_000, second * MB)).toBe(
        60_000 - second * 1_000,
      )
    }

    // A rate collapse at t=5 pushes the estimate to 70s. No adjustment is due yet, so the readout
    // keeps counting down instead of jumping.
    vi.advanceTimersByTime(1_000)
    expect(computeEta(job, 70_000, 1_000_000, 5 * MB)).toBe(55_000)
    for (let second = 6; second <= 9; second++) {
      vi.advanceTimersByTime(1_000)
      expect(
        computeEta(job, 70_000 - (second - 5) * 1_000, 1_000_000, second * MB),
      ).toBe(55_000 - (second - 5) * 1_000)
    }

    // t=10: the cadence is due, so it adopts the current estimate in one adjustment.
    vi.advanceTimersByTime(1_000)
    expect(computeEta(job, 75_000, 1_000_000, 10 * MB)).toBe(75_000)
  })

  it("freezes while bytes stop arriving, and resets on a retry", () => {
    const job = "eta-stall"
    computeEta(job, 60_000, 1_000_000, 0)
    vi.advanceTimersByTime(1_000)
    expect(computeEta(job, 59_000, 1_000_000, MB)).toBe(59_000)

    // No new bytes: the clock advances, the readout must not tick towards zero.
    for (let tick = 0; tick < 3; tick++) {
      vi.advanceTimersByTime(1_000)
      expect(computeEta(job, 59_000, 1_000_000, MB)).toBe(59_000)
    }

    // A retry/resume restarts the byte counter (back to 0): adopt the new estimate instead of
    // counting on from the interrupted run.
    vi.advanceTimersByTime(1_000)
    expect(computeEta(job, 30_000, 1_000_000, 0)).toBe(30_000)
  })

  it("uses the exact estimate in the last 10s and never reports a negative value", () => {
    const job = "eta-tail"
    computeEta(job, 40_000, 1_000_000, 0)
    vi.advanceTimersByTime(1_000)
    // Once the estimate itself is under 10s, it is shown as-is every tick.
    expect(computeEta(job, 9_000, 1_000_000, MB)).toBe(9_000)
    vi.advanceTimersByTime(1_000)
    expect(computeEta(job, 4_000, 1_000_000, 2 * MB)).toBe(4_000)
    vi.advanceTimersByTime(1_000)
    expect(computeEta(job, -500, 1_000_000, 3 * MB)).toBe(0)
  })
})
