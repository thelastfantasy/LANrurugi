import { describe, expect, it } from "vitest"

import { parseSharePermissions } from "@/share/permissions"

describe("share default permissions", () => {
  it("ignores the spaces around a comma-separated setting", () => {
    const all = { import: true, read: true, download: true }
    expect(parseSharePermissions("import, read, download", "")).toEqual(all)
    expect(parseSharePermissions("import,read,download", "")).toEqual(all)
    expect(parseSharePermissions(" read , download ,", "")).toEqual({ import: false, read: true, download: true })
  })

  it("falls back to all three when the setting holds nothing", () => {
    expect(parseSharePermissions(undefined, "import,read,download")).toEqual({
      import: true,
      read: true,
      download: true,
    })
    expect(parseSharePermissions("", "")).toEqual({ import: false, read: false, download: false })
  })

  it("grants nothing for a name it does not know", () => {
    expect(parseSharePermissions("read, write", "")).toEqual({ import: false, read: true, download: false })
  })
})
