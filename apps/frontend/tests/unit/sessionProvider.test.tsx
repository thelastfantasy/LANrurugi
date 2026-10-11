import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { act, renderHook, waitFor } from "@testing-library/react"
import type { ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { LoginStatus } from "@/api/types"
import {
  AUTHENTICATED_RECHECK_INTERVAL_MS,
  SessionProvider,
  useSession,
} from "@/session/SessionProvider"

const { fetchLoginStatusWithRefreshMock } = vi.hoisted(() => ({
  fetchLoginStatusWithRefreshMock: vi.fn(),
}))
vi.mock("@/api/client", () => ({
  fetchLoginStatusWithRefresh: fetchLoginStatusWithRefreshMock,
}))

function makeWrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  })
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <SessionProvider>{children}</SessionProvider>
      </QueryClientProvider>
    )
  }
}

describe("SessionProvider", () => {
  beforeEach(() => {
    fetchLoginStatusWithRefreshMock.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it("derives guest, authenticated, and default-password state from one status payload", async () => {
    const status: LoginStatus = {
      logged_in: false,
      guest_mode_enabled: true,
      using_default_password: true,
    }
    fetchLoginStatusWithRefreshMock.mockResolvedValue(status)

    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.isAuthenticated).toBe(false)
    expect(result.current.isGuestModeEnabled).toBe(true)
    expect(result.current.isGuest).toBe(true)
    expect(result.current.usingDefaultPassword).toBe(true)
    expect(result.current.status).toEqual(status)
  })

  it("does not classify an unauthenticated visitor as a guest when guest mode is off", async () => {
    fetchLoginStatusWithRefreshMock.mockResolvedValue({
      logged_in: false,
      guest_mode_enabled: false,
      using_default_password: false,
    } satisfies LoginStatus)

    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.isGuest).toBe(false)
    expect(result.current.isAuthenticated).toBe(false)
  })

  it("re-checks the session on an interval while signed in, so an expired access token can't linger", async () => {
    fetchLoginStatusWithRefreshMock.mockResolvedValue({
      logged_in: true,
      guest_mode_enabled: true,
      using_default_password: false,
    } satisfies LoginStatus)

    // Fake timers must be installed *before* the query mounts — React Query schedules the interval
    // from a real timer at that point, which switching to fake timers later wouldn't capture.
    vi.useFakeTimers({ shouldAdvanceTime: true })
    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })
    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(fetchLoginStatusWithRefreshMock).toHaveBeenCalledTimes(1)

    await act(async () => {
      await vi.advanceTimersByTimeAsync(AUTHENTICATED_RECHECK_INTERVAL_MS + 1)
    })

    expect(fetchLoginStatusWithRefreshMock).toHaveBeenCalledTimes(2)
  })

  it("never polls on behalf of a caller with no session to renew", async () => {
    fetchLoginStatusWithRefreshMock.mockResolvedValue({
      logged_in: false,
      guest_mode_enabled: true,
      using_default_password: false,
    } satisfies LoginStatus)

    vi.useFakeTimers({ shouldAdvanceTime: true })
    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })
    await waitFor(() => expect(result.current.isSuccess).toBe(true))

    await act(async () => {
      await vi.advanceTimersByTimeAsync(AUTHENTICATED_RECHECK_INTERVAL_MS * 3)
    })

    expect(fetchLoginStatusWithRefreshMock).toHaveBeenCalledTimes(1)
  })
})
