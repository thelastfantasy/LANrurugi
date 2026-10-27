import { afterEach, describe, expect, it, vi } from "vitest"

import { isGuestAccessiblePath, peerReachable } from "@/AuthBridgeGate"

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

describe("peerReachable", () => {
  afterEach(() => vi.unstubAllGlobals())

  it("is true when the peer answers, even opaquely", async () => {
    const fetchMock = vi.fn().mockResolvedValue(new Response(null, { status: 200 }))
    vi.stubGlobal("fetch", fetchMock)
    expect(await peerReachable("http://localhost:3100")).toBe(true)
    expect(fetchMock).toHaveBeenCalledWith("http://localhost:3100/api/info", expect.objectContaining({ mode: "no-cors" }))
  })

  it("is false on a network error, so the bridge does not navigate to a dead page", async () => {
    vi.stubGlobal("fetch", vi.fn().mockRejectedValue(new TypeError("Failed to fetch")))
    expect(await peerReachable("http://localhost:3100")).toBe(false)
  })

  it("never probes its own origin", async () => {
    const fetchMock = vi.fn()
    vi.stubGlobal("fetch", fetchMock)
    expect(await peerReachable(window.location.origin)).toBe(true)
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
