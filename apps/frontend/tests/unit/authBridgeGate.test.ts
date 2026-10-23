import { describe, expect, it } from "vitest"

import { isGuestAccessiblePath } from "@/AuthBridgeGate"

describe("isGuestAccessiblePath", () => {
  it("matches the two routes App.tsx wraps in AllowGuest", () => {
    expect(isGuestAccessiblePath("/")).toBe(true)
    expect(isGuestAccessiblePath("/reader/abc123")).toBe(true)
  })

  it("does not treat admin or unowned routes as guest-accessible", () => {
    expect(isGuestAccessiblePath("/login")).toBe(false)
    expect(isGuestAccessiblePath("/config")).toBe(false)
    expect(isGuestAccessiblePath("/reader")).toBe(false)
    expect(isGuestAccessiblePath("/reader/abc123/edit")).toBe(false)
  })
})
