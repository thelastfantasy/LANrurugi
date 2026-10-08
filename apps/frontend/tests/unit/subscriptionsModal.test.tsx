import { render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { DownloadQueueItem, PendingApproval, ReservationEntry, Subscription } from "@/api/types"

// The modal's whole point is which records it shows, and that is decided entirely by these three
// hooks — mocking them is the only way to see the list without a server that has matched something.
const state = {
  subscriptions: [] as Subscription[],
  pending: [] as PendingApproval[],
  reservations: [] as ReservationEntry[],
  queue: [] as DownloadQueueItem[],
}

vi.mock("@/api/hooks", () => ({
  useSubscriptions: () => ({ data: state.subscriptions }),
  usePendingApprovals: () => ({ data: state.pending }),
  useReservations: () => ({ data: state.reservations }),
  useDownloadQueue: () => ({ data: state.queue }),
  useApprovePending: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useDismissPending: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useDiscardReservation: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useStartQueueItem: () => ({ isPending: false, mutateAsync: vi.fn() }),
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

function renderModal() {
  return render(
    <MemoryRouter>
      <SubscriptionsModal onClose={() => {}} />
    </MemoryRouter>,
  )
}

describe("SubscriptionsModal", () => {
  beforeEach(() => {
    state.subscriptions = [subscription()]
    state.pending = []
    state.reservations = []
    state.queue = []
  })

  it("shows where an auto-downloaded match ended up, not just what is waiting", () => {
    // An auto-downloading subscription produces no approvals at all, so without the queue the modal
    // claimed nothing had matched while five works were downloading behind it.
    state.queue = [
      {
        id: "q1",
        url: "https://e-hentai.org/g/1/abcdef/",
        subscription_id: "s1",
        plugin_namespace: "download/ehentai",
        state: "done",
        title: "An Auto-Downloaded Work",
        created_at: 2,
      } as DownloadQueueItem,
      {
        id: "q2",
        url: "https://e-hentai.org/g/2/abcdef/",
        subscription_id: "s1",
        plugin_namespace: "download/ehentai",
        state: "downloading",
        title: null,
        created_at: 1,
      } as DownloadQueueItem,
    ]
    renderModal()

    expect(screen.getByText("An Auto-Downloaded Work")).toBeTruthy()
    expect(screen.getByText(/download queue/i)).toBeTruthy()
    expect(screen.getByText("Done")).toBeTruthy()
    expect(screen.getByText("Downloading")).toBeTruthy()
    // The empty state must not claim nothing matched when the queue has these.
    expect(screen.queryByText(/No subscription has matched/)).toBeNull()
  })

  it("lists the matched works, not the subscriptions themselves", () => {
    state.pending = [
      {
        id: "p1",
        subscription_id: "s1",
        source_url: "https://e-hentai.org/g/1/abcdef/",
        title: "A Matched Work",
        created_at: 1,
      },
    ]
    renderModal()

    expect(screen.getByText("A Matched Work")).toBeTruthy()
    // The subscription's own name belongs to the row as attribution, not as the list's subject…
    expect(screen.getByText("trace测试")).toBeTruthy()
    // …and the subscription list itself (its columns) is not what this modal is for.
    expect(screen.queryByText("Checks")).toBeNull()
    expect(screen.queryByText("Last checked")).toBeNull()
  })

  it("says so when nothing has matched, instead of showing an empty table", () => {
    renderModal()

    expect(screen.getByText(/No subscription has matched anything yet/)).toBeTruthy()
    expect(screen.queryByText("Checks")).toBeNull()
  })

  it("keeps the way to the full settings page, and the count of rules behind it", () => {
    renderModal()

    const link = screen.getByRole("link", { name: /Open the full settings page/ })
    expect(link.getAttribute("href")).toBe("/config?section=subscriptions")
    expect(screen.getByText(/1\/1/)).toBeTruthy()
  })
})
