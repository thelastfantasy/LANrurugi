import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/api/clientReportedInfo", () => ({
  collectClientReportedInfo: vi.fn(async () => ({})),
}))

import { ApiError, fetchJson, fetchLoginStatusWithRefresh } from "@/api/client"
import { queryClient } from "@/api/queryClient"
import { SESSION_QUERY_KEY } from "@/session/queryKey"

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  })
}

/** Seeds what the tab's `login-status` query currently holds — the signal that decides whether a
 * `403` is worth a renewal attempt (see `isRefreshWorthyStatus`). */
function cacheSessionStatus(loggedIn: boolean) {
  queryClient.setQueryData(SESSION_QUERY_KEY, {
    logged_in: loggedIn,
    guest_mode_enabled: true,
    using_default_password: false,
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

/** A tab left open past its 4h access token's expiry: with guest mode on the server answers admin
 * endpoints `403` (the request is indistinguishable from a legitimate guest's), so a refresh path
 * that only watched for `401` left the tab in that half-guest state until a full reload — the
 * live symptom behind this describe block (2026-10-08: one open tab polled `/api/shinobu` every
 * 5s and got `403` for 7.6 hours straight, with no refresh ever attempted). */
describe("403 recovery for a silently expired access token", () => {
  beforeEach(() => {
    localStorage.clear()
    queryClient.clear()
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
  })

  it("renews the session and retries the request once", async () => {
    cacheSessionStatus(true)
    let jobsCalls = 0
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url === "/api/jobs") {
        jobsCalls += 1
        return jobsCalls === 1 ? jsonResponse({ error: "Forbidden" }, 403) : jsonResponse({ jobs: [] })
      }
      if (url === "/api/token/refresh") return jsonResponse({ operation: "token_refresh", success: 1 })
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchJson<{ jobs: unknown[] }>("/jobs")).resolves.toEqual({ jobs: [] })

    expect(fetchMock).toHaveBeenCalledWith("/api/token/refresh", expect.objectContaining({ method: "POST" }))
    expect(jobsCalls).toBe(2)
  })

  it("leaves a genuine guest's 403 alone — nothing there can be renewed", async () => {
    cacheSessionStatus(false)
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url === "/api/jobs") return jsonResponse({ error: "Forbidden" }, 403)
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchJson("/jobs")).rejects.toBeInstanceOf(ApiError)

    expect(fetchMock).toHaveBeenCalledTimes(1)
    expect(fetchMock).not.toHaveBeenCalledWith("/api/token/refresh", expect.anything())
  })

  it("surfaces the 403 rather than looping when the renewed session still can't reach it", async () => {
    cacheSessionStatus(true)
    let jobsCalls = 0
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url === "/api/jobs") {
        jobsCalls += 1
        return jsonResponse({ error: "Forbidden" }, 403)
      }
      if (url === "/api/token/refresh") return jsonResponse({ operation: "token_refresh", success: 1 })
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchJson("/jobs")).rejects.toMatchObject({ status: 403 })

    expect(jobsCalls).toBe(2)
    expect(fetchMock.mock.calls.filter(([input]) => String(input) === "/api/token/refresh")).toHaveLength(1)
  })

  it("marks the session dead when the recovery refresh is rejected", async () => {
    cacheSessionStatus(true)
    const invalidateSpy = vi.spyOn(queryClient, "invalidateQueries")
    const fetchMock = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url === "/api/jobs") return jsonResponse({ error: "Forbidden" }, 403)
      if (url === "/api/token/refresh") return jsonResponse({ error: "Invalid or expired refresh token." }, 401)
      if (url === "/api/login/status") {
        return jsonResponse({ logged_in: false, guest_mode_enabled: true, using_default_password: false })
      }
      throw new Error(`unexpected fetch: ${url}`)
    })
    vi.stubGlobal("fetch", fetchMock)

    await expect(fetchJson("/jobs")).rejects.toMatchObject({ status: 403 })

    expect(invalidateSpy).toHaveBeenCalledWith({ queryKey: SESSION_QUERY_KEY })
  })
})
