import { beforeEach, describe, expect, it } from "vitest"

import { clearDiagnosticsDevice, diagnosticsDeviceId } from "@/diagnostics/device"

describe("diagnostics device id", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("is created once and reused for later events", () => {
    const first = diagnosticsDeviceId()
    expect(first).toMatch(/^[0-9a-f-]{32,36}$/i)
    expect(diagnosticsDeviceId()).toBe(first)
    expect(localStorage.getItem("lanrurugi_diagnostics_device")).toBe(first)
  })

  it("does not carry over to the next login", () => {
    const first = diagnosticsDeviceId()
    clearDiagnosticsDevice()
    expect(localStorage.getItem("lanrurugi_diagnostics_device")).toBeNull()
    expect(diagnosticsDeviceId()).not.toBe(first)
  })
})
