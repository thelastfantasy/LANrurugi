import { describe, expect, it } from "vitest"

import { memberLabel } from "@/pages/Library/matchReason"

describe("memberLabel", () => {
  it("shows the title and the chapter name when the tankoubon renamed the volume", () => {
    expect(memberLabel({ name: "巻の拾", title: "千代娘アンソロジー 10", chapter: "巻の拾" })).toBe(
      "千代娘アンソロジー 10 — 巻の拾",
    )
  })

  it("shows the title alone when the tankoubon set no chapter name", () => {
    expect(memberLabel({ name: "俺の上京性生活12", title: "俺の上京性生活12", chapter: null })).toBe(
      "俺の上京性生活12",
    )
  })

  it("does not repeat a chapter name that equals the title", () => {
    expect(memberLabel({ name: "第4巻", title: "第4巻", chapter: "第4巻" })).toBe("第4巻")
  })

  it("does not repeat a chapter name the title already ends with", () => {
    expect(
      memberLabel({ name: "Sparxie", title: "Yuikai Chan - Sparxie", chapter: "Sparxie" }),
    ).toBe("Yuikai Chan - Sparxie")
  })

  it("falls back to the single name an older payload carries", () => {
    expect(memberLabel({ name: "Sparxie" })).toBe("Sparxie")
  })
})
