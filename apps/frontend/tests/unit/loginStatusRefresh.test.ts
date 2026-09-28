import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/api/clientReportedInfo", () => ({
  collectClientReportedInfo: vi.fn(async () => ({})),
}))

import { fetchLoginStatusWithRefresh } from "@/api/client"

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  })
}

describe("fetchLoginStatusWithRefresh", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it("refreshes an expired access token even when guest mode is off", async () => {
    let statusCalls = 0
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url === "/api/login/status") {
        statusCalls += 1
        return jsonResponse({
          logged_in: statusCalls > 1,
          guest_mode_enabled: false,
          using_default_password: false,
        })
      }
      if (url === "/api/token/refresh") {
        return jsonResponse({ operation: "token_refresh", success: 1 })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal("fetch", fetchMock)

    const status = await fetchLoginStatusWithRefresh<{
      logged_in: boolean
      guest_mode_enabled: boolean
      using_default_password: boolean
    }>()

    expect(fetchMock).toHaveBeenCalledWith("/api/token/refresh", expect.objectContaining({ method: "POST" }))
    expect(status.logged_in).toBe(true)
  })
})
