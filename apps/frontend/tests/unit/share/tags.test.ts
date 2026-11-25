import { describe, expect, it } from "vitest"

import { visibleShareTags } from "@/share/ShareTags"

describe("share page tags", () => {
  it("keeps the content tags and drops the dates", () => {
    const tags = "date_added:1795410053,source:e-hentai.org/g/1/abc,cosplayer:someone"
    expect(visibleShareTags(tags)).toBe("source:e-hentai.org/g/1/abc,cosplayer:someone")
  })

  it("leaves out a plugin's download URL, which can carry the sharer's key", () => {
    const tags = "download:https://example.invalid/a.zip?key=secret,timestamp:1785981034,artist:someone"
    expect(visibleShareTags(tags)).toBe("artist:someone")
  })

  it("hides them case-insensitively and with leading spaces", () => {
    expect(visibleShareTags(" Date_Added:1, Source:example.org/g/1")).toBe(" Source:example.org/g/1")
  })

  it("is empty for a record with nothing left to show", () => {
    expect(visibleShareTags("date_added:1,,")).toBe("")
    expect(visibleShareTags("")).toBe("")
  })
})
