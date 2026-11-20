import { render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { PendingApproval, Subscription } from "@/api/types"

const state = {
  subscriptions: [] as Subscription[],
  pending: [] as PendingApproval[],
  failed: 0,
}

vi.mock("@/api/hooks", () => ({
  useSubscriptions: () => ({ data: state.subscriptions }),
  usePendingApprovals: () => ({ data: state.pending }),
  useSubscriptionFailedCount: () => state.failed,
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
    state.failed = 0
  })

  it("shows the button alone when nothing needs acting on", () => {
    renderRow()

    // The count lives on the button; a "0/1 enabled" caption beside it would repeat it.
    expect(screen.getByRole("button", { name: "Manage subscriptions" }).textContent).toBe("1")
    expect(screen.queryByText(/次检查|待确认|failed/i)).toBeNull()
  })

  it("keeps a line for the counts that do need acting on", () => {
    state.pending = [{ id: "p1", subscription_id: "s1", source_url: "https://e/x", created_at: 1 }]
    // Failed in the queue or before reaching it: one count either way.
    state.failed = 2
    renderRow()

    // The counts are spelled out only in the button's label (and its tooltip, which may not be
    // mounted), so query the label rather than visible text.
    const button = screen.getByRole("button", { name: /1 awaiting approval/i })
    expect(button.getAttribute("aria-label")).toMatch(/2 failed downloads/i)
  })

  it("renders nothing at all with no subscriptions", () => {
    state.subscriptions = []
    const { container } = renderRow()

    expect(container.textContent).toBe("")
  })
})
