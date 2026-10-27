import { describe, expect, it } from "vitest"

import { defaultShareTitle } from "@/share/ShareDialog"
import { daysUntil, endOfLocalDay, formFromShare, localDateOf } from "@/share/ShareFieldsForm"

const t = ((key: string, o: { name: string; count: number }) => `${key}|${o.name}|${o.count}`) as never

describe("share form", () => {
  it("titles one item by its name and several by a shortened first name", () => {
    expect(defaultShareTitle("  Volume 1 ", 1, t)).toBe("Volume 1")
    expect(defaultShareTitle("Volume 1", 3, t)).toBe("share.dialog.autoTitle|Volume 1|3")
    const long = "あ".repeat(50)
    expect(defaultShareTitle(long, 2, t)).toBe(`share.dialog.autoTitle|${"あ".repeat(29)}…|2`)
  })

  it("keeps an existing share's expiry exactly as stored", () => {
    const form = formFromShare({
      title: "",
      permissions: { import: true, read: true, download: true },
      download_limit: null,
      expires_at: 1_900_000_123,
      embed: false,
      allow_helpers: false,
    })
    expect(form.expiresAt).toBe(1_900_000_123)
  })

  it("treats a picked date as the end of that local day and counts days rounded up", () => {
    const end = endOfLocalDay("2030-05-17")
    expect(end).not.toBe("")
    expect(localDateOf(end as number)).toBe("2030-05-17")
    expect(new Date((end as number) * 1000).getHours()).toBe(23)
    expect(endOfLocalDay("")).toBe("")
    expect(daysUntil(1000 + 86_400, 1000)).toBe(1)
    expect(daysUntil(1000 + 86_401, 1000)).toBe(2)
    expect(daysUntil(500, 1000)).toBe(1)
  })
})
