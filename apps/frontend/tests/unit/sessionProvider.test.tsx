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
      first_run_wizard_available: true,
      first_run_wizard_pending: true,
    }
    fetchLoginStatusWithRefreshMock.mockResolvedValue(status)

    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.isAuthenticated).toBe(false)
    expect(result.current.isGuestModeEnabled).toBe(true)
    expect(result.current.isGuest).toBe(true)
    expect(result.current.usingDefaultPassword).toBe(true)
    // The server already gates both wizard fields on `logged_in`, so a `true` here can only mean a
    // stale or hand-crafted payload — the derived values must not trust them on their own.
    expect(result.current.firstRunWizardAvailable).toBe(false)
    expect(result.current.firstRunWizardPending).toBe(false)
    expect(result.current.status).toEqual(status)
  })

  it("exposes the pending first-run wizard only to an authenticated session", async () => {
    fetchLoginStatusWithRefreshMock.mockResolvedValue({
      logged_in: true,
      guest_mode_enabled: false,
      using_default_password: false,
      first_run_wizard_available: true,
      first_run_wizard_pending: true,
    } satisfies LoginStatus)

    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.isAuthenticated).toBe(true)
    expect(result.current.firstRunWizardAvailable).toBe(true)
    expect(result.current.firstRunWizardPending).toBe(true)
  })

  it("keeps the wizard available but not pending once the instance has handled it", async () => {
    fetchLoginStatusWithRefreshMock.mockResolvedValue({
      logged_in: true,
      guest_mode_enabled: false,
      using_default_password: false,
      first_run_wizard_available: true,
      first_run_wizard_pending: false,
    } satisfies LoginStatus)

    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    // Still runnable from the Settings page — only the automatic nudge is done with.
    expect(result.current.firstRunWizardAvailable).toBe(true)
    expect(result.current.firstRunWizardPending).toBe(false)
  })

  it("reports the wizard as unavailable on a deployment that switched it off", async () => {
    fetchLoginStatusWithRefreshMock.mockResolvedValue({
      logged_in: true,
      guest_mode_enabled: false,
      using_default_password: false,
      first_run_wizard_available: false,
      first_run_wizard_pending: false,
    } satisfies LoginStatus)

    const { result } = renderHook(() => useSession(), { wrapper: makeWrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.isAuthenticated).toBe(true)
    expect(result.current.firstRunWizardAvailable).toBe(false)
    expect(result.current.firstRunWizardPending).toBe(false)
  })

  it("does not classify an unauthenticated visitor as a guest when guest mode is off", async () => {
    fetchLoginStatusWithRefreshMock.mockResolvedValue({
      logged_in: false,
      guest_mode_enabled: false,
      using_default_password: false,
      first_run_wizard_available: false,
      first_run_wizard_pending: false,
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
      first_run_wizard_available: false,
      first_run_wizard_pending: false,
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
      first_run_wizard_available: false,
      first_run_wizard_pending: false,
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
