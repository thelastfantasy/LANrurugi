import { render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { PendingApproval, ReservationEntry, Subscription } from "@/api/types"

const state = {
  subscriptions: [] as Subscription[],
  pending: [] as PendingApproval[],
  reservations: [] as ReservationEntry[],
}

vi.mock("@/api/hooks", () => ({
  useSubscriptions: () => ({ data: state.subscriptions }),
  usePendingApprovals: () => ({ data: state.pending }),
  useReservations: () => ({ data: state.reservations }),
}))

const { SubscriptionStatus } = await import("@/pages/Upload/SubscriptionStatus")

function subscription(): Subscription {
  return { id: "s1", name: "trace测试", state: { state: "enabled" } } as Subscription
}

function renderRow() {
  return render(
    <MemoryRouter>
      <SubscriptionStatus />
    </MemoryRouter>,
  )
}

describe("SubscriptionStatus", () => {
  beforeEach(() => {
    state.subscriptions = [subscription()]
    state.pending = []
    state.reservations = []
  })

  it("shows the button alone when nothing needs acting on", () => {
    renderRow()

    // The count lives on the button; a "0/1 enabled" caption beside it would repeat it.
    expect(screen.getByRole("button", { name: "Manage subscriptions" }).textContent).toBe("1")
    expect(screen.queryByText(/次检查|待确认|预约/)).toBeNull()
  })

  it("keeps a line for the counts that do need acting on", () => {
    state.pending = [{ id: "p1", subscription_id: "s1", source_url: "https://e/x", created_at: 1 }]
    state.reservations = [
      { id: "r1", subscription_id: "s1", source_url: "https://e/y", reason: "insufficient_credit", status: "waiting", created_at: 2 },
    ]
    renderRow()

    expect(screen.getByText(/waiting/i)).toBeTruthy()
    expect(screen.getByText(/Reserved/i)).toBeTruthy()
  })

  it("renders nothing at all with no subscriptions", () => {
    state.subscriptions = []
    const { container } = renderRow()

    expect(container.textContent).toBe("")
  })
})
