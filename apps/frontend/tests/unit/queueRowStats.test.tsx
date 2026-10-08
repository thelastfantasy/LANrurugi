// Pulls in the app's own i18next initialization, so the assertions below can match the real
// English strings rather than the raw keys.
import "@/i18n"

import { act, render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router-dom"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { DownloadQueueItem, JobRecord } from "@/api/types"
import { RowStats } from "@/pages/Upload/QueueItemRow"

function item(over: Partial<DownloadQueueItem> = {}): DownloadQueueItem {
  return { id: "i1", url: "https://e/x", plugin_namespace: "download/ehentai", state: "done", ...over } as DownloadQueueItem
}

function job(over: Partial<JobRecord> = {}): JobRecord {
  return { id: "j1", name: "download_url", state: "finished", progress: 1, result: null, result_truncated: false, error: null, ...over } as JobRecord
}

function renderStats(i: DownloadQueueItem, j?: JobRecord) {
  return render(
    <MemoryRouter>
      <RowStats item={i} job={j} subscriptionName={undefined} onOpenSubscriptions={vi.fn()} />
    </MemoryRouter>,
  )
}

describe("RowStats", () => {
  // A running row reads the clock from a tick (never during render), so the timings appear one tick
  // in — which is exactly what these fake timers advance.
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it("summarises a live job: started at, elapsed, average speed and size", async () => {
    const started = new Date(2026, 9, 7, 14, 32, 5).getTime()
    vi.setSystemTime(started + 120_000)
    renderStats(
      item({ state: "downloading" }),
      job({ state: "active", started_at: started, downloaded_bytes: 1024 * 1024 * 1024, total_bytes: 1024 * 1024 * 1024 }),
    )
    await act(async () => {
      vi.advanceTimersByTime(1000)
    })

    const line = screen.getByText(/started/).textContent ?? ""
    // Deliberately format-agnostic: the clock is rendered through the browser's own
    // `toLocaleTimeString`, so CI (en-US) prints `2:32:05 PM` where a 24-hour locale prints
    // `14:32:05`. Asserting one of the two made this test pass locally and fail in CI.
    expect(line).toMatch(/started \d{1,2}:32:05/)
    expect(line).toMatch(/2:01 elapsed/)
    // 1 GB over 2 minutes is ~8.5 MB/s: the run's own average, not a last-poll reading.
    expect(line).toMatch(/8\.5 MB\/s avg/)
    expect(line).toMatch(/1\.0 GB/)
  })

  it("uses the item's own persisted timings once the job is gone", () => {
    const started = new Date(2026, 9, 7, 10, 0, 0).getTime()
    renderStats(
      item({ started_at: started, finished_at: started + 90_000, file_size: 90 * 1024 * 1024 }),
      undefined,
    )

    const line = screen.getByText(/started/).textContent ?? ""
    expect(line).toMatch(/started 10:00:00/)
    expect(line).toMatch(/1:30 elapsed/)
    // 90 MB in 90 s = 1 MB/s
    expect(line).toMatch(/1\.0 MB\/s avg/)
    // `file_size` is the completed size, and the average divides *that* by the persisted duration.
    expect(line).toMatch(/1\.0 MB\/s avg/)
  })

  it("renders only the subscription mark when there is nothing to measure", () => {
    const { container } = renderStats(item({ state: "queued", subscription_id: "s1" }), undefined)

    expect(screen.queryByText(/started/)).toBeNull()
    expect(container.querySelector("i.fa-rss")).toBeTruthy()
  })

  it("renders nothing at all for a plain item with no timings", () => {
    const { container } = renderStats(item({ state: "queued" }), undefined)

    expect(container.textContent).toBe("")
  })
})
