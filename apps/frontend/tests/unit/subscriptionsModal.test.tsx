import { render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { HistoryEntry, PendingApproval, ReservationListItem, Subscription } from "@/api/types"

// The merged subscriptions dialog: what waits on the user, what failed (whether or not it ever
// reached the queue), and the way out to the full settings page. History rows have their own spec.
const state = {
  subscriptions: [] as Subscription[],
  pending: [] as PendingApproval[],
  reservations: [] as ReservationListItem[],
  history: [] as HistoryEntry[],
}

vi.mock("@/api/hooks", () => ({
  useSubscriptions: () => ({ data: state.subscriptions }),
  usePendingApprovals: () => ({ data: state.pending }),
  useApprovePending: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useDismissPending: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useReservations: () => ({ data: state.reservations }),
  useRetryReservation: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useDiscardReservation: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useSubscriptionFailedCount: () => state.reservations.length,
  useSubscriptionHistory: () => ({ data: state.history, isPending: false }),
  useBlockedWorks: () => ({ data: [], isPending: false }),
  useBlockSubscriptionSources: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useForgetSeen: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useUnskipSubscriptionSources: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useStartQueueItem: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useSettings: () => ({ data: { download_queue_retention_days: 30 }, isPending: false }),
  useUpdateSettings: () => ({ isPending: false, mutateAsync: vi.fn() }),
}))

const { SubscriptionsModal } = await import("@/pages/Upload/SubscriptionsModal")

function subscription(over: Partial<Subscription> = {}): Subscription {
  return {
    id: "s1",
    name: "trace测试",
    source: "discovery/ehentai",
    interval_secs: 3600,
    last_checked_at: null,
    state: { state: "enabled" },
    ...over,
  } as Subscription
}

function renderModal(initialBand?: "pending" | "failed") {
  return render(
    <MemoryRouter>
      <SubscriptionsModal initialBand={initialBand} onClose={() => {}} />
    </MemoryRouter>,
  )
}

describe("SubscriptionsModal", () => {
  beforeEach(() => {
    state.subscriptions = [subscription()]
    state.pending = []
    state.reservations = []
    state.history = []
  })

  it("lists the works waiting for approval on its own tab, with the count on the tab", () => {
    state.pending = [
      { id: "p1", subscription_id: "s1", source_url: "e-hentai.org/g/1/abcdef", title: { origin: "A Matched Work" }, created_at: 1 },
    ]
    renderModal("pending")

    expect(screen.getByText("A Matched Work")).toBeTruthy()
    expect(screen.getByRole("tab", { name: /\(1\)/ })).toBeTruthy()
  })

  it("says so when nothing waits, instead of an empty table", () => {
    renderModal("pending")

    expect(screen.getByText(/Nothing is waiting for your go-ahead/)).toBeTruthy()
  })

  it("shows a work that never reached the queue as a failed download, with its reason", () => {
    state.reservations = [
      {
        id: "r1",
        subscription_id: "s1",
        source_url: "e-hentai.org/g/2/abcdef",
        reason: "insufficient_credit",
        status: "waiting",
        created_at: 2,
        retryable: true,
      },
    ]
    renderModal("failed")

    expect(screen.getByText("Download failed")).toBeTruthy()
    expect(screen.getByText(/credit/i)).toBeTruthy()
    expect(screen.getByRole("button", { name: /Retry/i })).toBeTruthy()
  })

  it("keeps the way to the full settings page, and the count of rules behind it", () => {
    renderModal()

    const link = screen.getByRole("link", { name: /Open the full settings page/ })
    expect(link.getAttribute("href")).toBe("/config?section=subscriptions")
    expect(screen.getByText(/1\/1/)).toBeTruthy()
  })
})
