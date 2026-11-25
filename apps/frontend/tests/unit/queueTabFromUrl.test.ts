import { describe, expect, it } from "vitest"

import { tabByGroupFromUrl } from "@/pages/Upload/DownloadQueuePanel"

/** The reader's back button (and a phone's swipe-back) returns to the URL a finished row was
 * opened from, so the tab a person left has to live there — no `localStorage` key to clear. */
describe("tabByGroupFromUrl", () => {
  it("restores the tab of the group the address names", () => {
    const params = new URLSearchParams("queue_group=download%2Fehentai&queue_tab=subscription")
    expect(tabByGroupFromUrl(params)).toEqual({ "download/ehentai": "subscription" })
  })

  it("ignores a tab on its own and any unknown value", () => {
    expect(tabByGroupFromUrl(new URLSearchParams("queue_tab=subscription"))).toEqual({})
    expect(
      tabByGroupFromUrl(new URLSearchParams("queue_group=download%2Fehentai&queue_tab=bogus")),
    ).toEqual({})
    // No parameters at all is the common case: a group opens on its own default, not on `manual`
    // written into the address.
    expect(tabByGroupFromUrl(new URLSearchParams(""))).toEqual({})
  })
})
