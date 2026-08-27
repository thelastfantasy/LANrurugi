import { render, screen } from "@testing-library/react"
import { MemoryRouter, Route, Routes } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { LoginStatus } from "@/api/types"
import { AllowGuest } from "@/RouteGuards"

// `AllowGuest` (007-guest-restricted-access) reads `useLoginStatus()` directly — mocking the
// whole `@/api/hooks` module (rather than standing up a real TanStack Query provider + MSW
// handler) keeps this a fast, no-backend unit test per this suite's own Layer 1 charter
// (vitest.config.ts's own docs).
const { useLoginStatusMock } = vi.hoisted(() => ({ useLoginStatusMock: vi.fn() }))
vi.mock("@/api/hooks", () => ({ useLoginStatus: useLoginStatusMock }))

function mockLoginStatus(data: Partial<LoginStatus> | undefined, isSuccess: boolean) {
  useLoginStatusMock.mockReturnValue({ data, isSuccess })
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

describe("AllowGuest", () => {
  beforeEach(() => {
    useLoginStatusMock.mockReset()
  })

  it("renders children when the login-status query is still resolving (optimistic render, matching RequireAuth's own default)", () => {
    mockLoginStatus(undefined, false)
    renderAllowGuest()
    expect(screen.getByText("protected content")).toBeInTheDocument()
  })

  it("renders children for a real logged-in session", () => {
    mockLoginStatus({ logged_in: true, guest_mode_enabled: false } as LoginStatus, true)
    renderAllowGuest()
    expect(screen.getByText("protected content")).toBeInTheDocument()
  })

  it("renders children for an eligible unauthenticated guest (guest_mode_enabled: true)", () => {
    mockLoginStatus({ logged_in: false, guest_mode_enabled: true } as LoginStatus, true)
    renderAllowGuest()
    expect(screen.getByText("protected content")).toBeInTheDocument()
  })

  it("redirects to /login when neither logged in nor guest-eligible", () => {
    mockLoginStatus({ logged_in: false, guest_mode_enabled: false } as LoginStatus, true)
    renderAllowGuest()
    expect(screen.getByText("login page")).toBeInTheDocument()
    expect(screen.queryByText("protected content")).not.toBeInTheDocument()
  })
})
