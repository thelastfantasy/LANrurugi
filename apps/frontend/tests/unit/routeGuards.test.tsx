import { render, screen } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { LoginStatus } from "@/api/types"
import { AllowGuest, RequireAuth } from "@/RouteGuards"

// Route guards now read the single SessionProvider context instead of each calling the
// login-status query directly. Mocking `useSession` keeps this a fast, no-backend unit test per
// this suite's own Layer 1 charter (vitest.config.ts's own docs).
const { useSessionMock } = vi.hoisted(() => ({ useSessionMock: vi.fn() }))
vi.mock("@/session/SessionProvider", () => ({ useSession: useSessionMock }))

function mockSession(
  status: Partial<LoginStatus> | undefined,
  isSuccess: boolean,
  isError = false,
) {
  const normalized = status as LoginStatus | undefined
  const isAuthenticated = normalized?.logged_in === true
  const isGuestModeEnabled = normalized?.guest_mode_enabled === true
  useSessionMock.mockReturnValue({
    status: normalized,
    isSuccess,
    isError,
    isPending: !isSuccess && !isError,
    isFetching: false,
    isAuthenticated,
    isGuestModeEnabled,
    isGuest: !isAuthenticated && isGuestModeEnabled,
    usingDefaultPassword: normalized?.using_default_password === true,
    refresh: vi.fn(),
  })
}

function renderAllowGuest(initialPath = "/") {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <Routes>
        <Route element={<AllowGuest />}>
          <Route path="/" element={<div>protected content</div>} />
        </Route>
        <Route path="/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  )
}

function renderRequireAuth(initialPath = "/") {
  return render(
    <MemoryRouter initialEntries={[initialPath]}>
      <Routes>
        <Route element={<RequireAuth />}>
          <Route path="/" element={<div>admin content</div>} />
        </Route>
        <Route path="/login" element={<div>login page</div>} />
      </Routes>
    </MemoryRouter>,
  )
}

describe("AllowGuest", () => {
  beforeEach(() => {
    useSessionMock.mockReset()
  })

  it("renders children when the session query is still resolving (optimistic render for guest pages)", () => {
    mockSession(undefined, false)
    renderAllowGuest()
    expect(screen.getByText("protected content")).toBeInTheDocument()
  })

  it("renders children for a real logged-in session", () => {
    mockSession({ logged_in: true, guest_mode_enabled: false } as LoginStatus, true)
    renderAllowGuest()
    expect(screen.getByText("protected content")).toBeInTheDocument()
  })

  it("renders children for an eligible unauthenticated guest (guest_mode_enabled: true)", () => {
    mockSession({ logged_in: false, guest_mode_enabled: true } as LoginStatus, true)
    renderAllowGuest()
    expect(screen.getByText("protected content")).toBeInTheDocument()
  })

  it("redirects to /login when neither logged in nor guest-eligible", () => {
    mockSession({ logged_in: false, guest_mode_enabled: false } as LoginStatus, true)
    renderAllowGuest()
    expect(screen.getByText("login page")).toBeInTheDocument()
    expect(screen.queryByText("protected content")).not.toBeInTheDocument()
  })
})

describe("RequireAuth", () => {
  beforeEach(() => {
    useSessionMock.mockReset()
  })

  it("does not render admin content while session status is still resolving", () => {
    mockSession(undefined, false)
    renderRequireAuth()
    expect(screen.queryByText("admin content")).not.toBeInTheDocument()
  })

  it("redirects to /login when session status fails so admin content cannot leak", () => {
    mockSession(undefined, false, true)
    renderRequireAuth()
    expect(screen.getByText("login page")).toBeInTheDocument()
    expect(screen.queryByText("admin content")).not.toBeInTheDocument()
  })

  it("renders admin content for a real logged-in session", () => {
    mockSession({ logged_in: true, guest_mode_enabled: false } as LoginStatus, true)
    renderRequireAuth()
    expect(screen.getByText("admin content")).toBeInTheDocument()
  })

  it("redirects to /login when session status reports logged out", () => {
    mockSession({ logged_in: false, guest_mode_enabled: true } as LoginStatus, true)
    renderRequireAuth()
    expect(screen.getByText("login page")).toBeInTheDocument()
    expect(screen.queryByText("admin content")).not.toBeInTheDocument()
  })
})
