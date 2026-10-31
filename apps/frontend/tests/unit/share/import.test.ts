import { afterEach, describe, expect, it, vi } from "vitest"

import { liveScanAvailable, shareRefsFromPayloads } from "@/share/ImportShareModal"

describe("import share modal logic", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("offers live scanning only in a secure context with camera access", () => {
    vi.stubGlobal("isSecureContext", false)
    expect(liveScanAvailable()).toBe(false)
    vi.stubGlobal("isSecureContext", true)
    vi.stubGlobal("navigator", { mediaDevices: { getUserMedia: () => Promise.resolve() } })
    expect(liveScanAvailable()).toBe(true)
    vi.stubGlobal("navigator", {})
    expect(liveScanAvailable()).toBe(false)
  })

  it("keeps every distinct share reference and ignores everything else", () => {
    const codes = (payloads: string[]) => shareRefsFromPayloads(payloads).map((r) => r.code)
    expect(codes(["https://unrelated.example.org/page", "LRRG:k7qm9-xrt2h"])).toEqual(["K7QM9XRT2H"])
    expect(codes(["https://lib.example.org/s/K7QM9-XRT2H"])).toEqual(["K7QM9XRT2H"])
    expect(codes(["lrrg:K7QM9-XRT2H", "lrrg:ABCDE-12345"])).toEqual(["K7QM9XRT2H", "ABCDE12345"])
    expect(codes(["https://evil.example.org/login", "WIFI:S:home;;"])).toEqual([])
    expect(codes([])).toEqual([])
  })

  it("collapses repeats of one code, keeping the copy with a via hint", () => {
    const refs = shareRefsFromPayloads(["lrrg:K7QM9-XRT2H", "lrrg:K7QM9XRT2H?via=abc@203.0.113.7:443"])
    expect(refs).toHaveLength(1)
    expect(refs[0].via?.addr).toBe("203.0.113.7:443")
  })

  it("counts a bare code only when no explicit share reference is present", () => {
    const codes = (payloads: string[]) => shareRefsFromPayloads(payloads).map((r) => r.code)
    expect(codes(["12345678", "lrrg:K7QM9-XRT2H"])).toEqual(["K7QM9XRT2H"])
    expect(codes(["K7QM9XRT2H"])).toEqual(["K7QM9XRT2H"])
  })
})
