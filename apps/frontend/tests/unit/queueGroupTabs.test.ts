import { describe, expect, it } from "vitest"

import type { DownloadQueueItem } from "@/api/types"
import { needsDecision, partitionByOrigin } from "@/pages/Upload/shared"

function item(over: Partial<DownloadQueueItem> = {}): DownloadQueueItem {
  return { id: "i1", url: "https://e/x", plugin_namespace: "download/ehentai", state: "queued", ...over } as DownloadQueueItem
}

describe("partitionByOrigin", () => {
  it("splits a group's items by whether a subscription queued them", () => {
    const { manual, fromSubscriptions } = partitionByOrigin([
      item({ id: "a" }),
      item({ id: "b", subscription_id: "s1" }),
      item({ id: "c", subscription_id: null }),
      item({ id: "d", subscription_id: "s2" }),
    ])

    expect(manual.map((i) => i.id)).toEqual(["a", "c"])
    expect(fromSubscriptions.map((i) => i.id)).toEqual(["b", "d"])
  })
})

describe("needsDecision", () => {
  it("counts a stopped item, not one that is merely in progress or finished", () => {
    expect(needsDecision(item({ state: "downloading" }))).toBe(false)
    expect(needsDecision(item({ state: "done" }))).toBe(false)
    expect(needsDecision(item({ state: "error" }))).toBe(true)
    expect(needsDecision(item({ state: "awaiting_revision_confirmation" }))).toBe(true)
    expect(
      needsDecision(
        item({ pending_filename_conflict: { filename: "a.zip", staged_at: 1 } as DownloadQueueItem["pending_filename_conflict"] }),
      ),
    ).toBe(true)
  })
})
