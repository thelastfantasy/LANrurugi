import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { fireEvent, render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { HistoryEntry, Subscription } from "@/api/types"

// The history modal's own two states: a first page still arriving (skeleton) and rows whose
// *outcome* — not just the cycle's decision — is what the reader needs.
const state = {
  entries: [] as HistoryEntry[],
  subscriptions: [] as Subscription[],
  pending: false,
  blocked: [] as { subscription_id: string; subscription_name: string; source_url: string; blocked_at: number }[],
}

// Partially mocked: the modal's own two hooks are faked, everything else in the module stays real
// (the routing and title helpers it renders through reach further into it than the mocks alone).
vi.mock("@/api/hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/hooks")>()),
  useSubscriptionHistory: () => ({ data: state.entries, isPending: state.pending }),
  useStartQueueItem: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useBlockedWorks: () => ({ data: state.blocked, isPending: false }),
  useBlockSubscriptionSources: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useForgetSeen: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useUnskipSubscriptionSources: () => ({ isPending: false, mutateAsync: vi.fn() }),
  // The merged dialog's other tabs and footer; empty here, since these tests are about history rows.
  usePendingApprovals: () => ({ data: [] }),
  useReservations: () => ({ data: [] }),
  useRetryReservation: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useDiscardReservation: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useSubscriptionFailedCount: () => 0,
  useSubscriptions: () => ({ data: state.subscriptions }),
  useSettings: () => ({ data: { download_queue_retention_days: 30 }, isPending: false }),
  useUpdateSettings: () => ({ isPending: false, mutateAsync: vi.fn() }),
}))

const { SubscriptionsModal } = await import("@/pages/Upload/SubscriptionsModal")

function entry(over: Partial<HistoryEntry> = {}): HistoryEntry {
  return {
    subscription_id: "s1",
    subscription_name: "a-sub",
    checked_at: 1_790_000_000,
    candidate: {
      source_url: "e-hentai.org/g/1/abcdef",
      title: { origin: "A Work" },
      tags: [],
      verdict: { verdict: "queued" },
    },
    download: null,
    ...over,
  } as HistoryEntry
}

function renderModal() {
  // A real client, with retries off: the modal's own hooks are mocked, but the components it renders
  // look a few hooks deeper into the same module and expect a provider to exist.
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter>
        <SubscriptionsModal onClose={() => {}} />
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

describe("SubscriptionsModal history tabs", () => {
  beforeEach(() => {
    state.entries = []
    state.subscriptions = []
    state.pending = false
  })

  it("shows shaped placeholder rows while the first page loads", () => {
    state.pending = true
    renderModal()

    // A plain "loading…" line is what this replaced: the modal opens at one row's height and then
    // jumps to a full viewport-tall table.
    expect(screen.getByRole("status")).toBeTruthy()
    expect(document.querySelectorAll(".subscriptions-skeleton-row").length).toBe(12)
    expect(screen.getByText("Work")).toBeTruthy()
  })

  it("reports where the work ended up, not just that it was queued", () => {
    state.entries = [
      entry({ download: { id: "q1", state: "done", can_retry: false } as HistoryEntry["download"] }),
      entry({
        candidate: { ...entry().candidate, source_url: "e-hentai.org/g/2/abcdef" },
        download: { id: "q2", state: "error", can_retry: true } as HistoryEntry["download"],
      }),
    ]
    renderModal()

    // Catalogued: no longer reads as "queued", and bands blue like any already-held work.
    expect(screen.getByText("In the library")).toBeTruthy()
    // The queue state is an icon now, with its name only in the tooltip/aria-label: as a line of text
    // it made every downloaded row taller than one that was merely seen.
    expect(screen.getAllByLabelText("Done").length).toBeGreaterThan(0)
    // Failed: says so, and keeps the retry that the old raw state string ("error") never explained.
    expect(screen.getByLabelText("Failed")).toBeTruthy()
    expect(screen.getByRole("button", { name: /Retry/ }) ?? screen.getByDisplayValue("Retry")).toBeTruthy()
  })

  it("marks a blocked work and offers the way back, in any tab", () => {
    state.entries = [entry()]
    state.blocked = [
      {
        subscription_id: "s1",
        subscription_name: "a-sub",
        source_url: "e-hentai.org/g/1/abcdef",
        blocked_at: 1_790_000_000_000,
      },
    ]
    renderModal()

    expect(screen.getByText(/You blocked this work/)).toBeTruthy()
    // An icon button now, named for screen readers (and its tooltip).
    expect(screen.getByRole("button", { name: "Reconsider" })).toBeTruthy()
  })

  it("orders the list by the heading that was clicked, publication time first", () => {
    const work = (url: string, title: string, posted: number | null) =>
      entry({
        candidate: {
          ...entry().candidate,
          source_url: url,
          title: { origin: title },
          posted_at: posted,
        },
      })
    state.entries = [
      work("e-hentai.org/g/1/aaaaaa", "Beta", 200),
      work("e-hentai.org/g/2/bbbbbb", "Alpha", 300),
      // A source that exposes no publication time: it is not "oldest", it is unknown, so it stays
      // at the bottom whichever way the column is read.
      work("e-hentai.org/g/3/cccccc", "Gamma", null),
    ]
    renderModal()

    const titles = () =>
      [...document.querySelectorAll(".preview-row:not(.preview-head) .preview-title")].map(
        (node) => node.textContent,
      )

    // Opens on publication time, newest first, with the untimed row last.
    expect(titles()).toEqual(["Alpha", "Beta", "Gamma"])
    // Clicking the same heading flips it; the untimed row still cannot be placed, so it stays last.
    const posted = screen.getByRole("button", { name: "Published" })
    fireEvent.click(posted)
    expect(titles()).toEqual(["Beta", "Alpha", "Gamma"])
    expect(posted.getAttribute("aria-pressed")).toBe("true")
    // A different heading starts in its own natural order rather than inheriting the previous one.
    const title = screen.getByRole("button", { name: "Work" })
    fireEvent.click(title)
    expect(titles()).toEqual(["Alpha", "Beta", "Gamma"])
    expect(title.getAttribute("aria-pressed")).toBe("true")
    expect(posted.getAttribute("aria-pressed")).toBe("false")
    // Second click on the same heading reverses it.
    fireEvent.click(title)
    expect(titles()).toEqual(["Gamma", "Beta", "Alpha"])
  })

  it("keeps subscriptions in the filter's order and sorts only the works inside them", () => {
    // Deliberately not the order the works arrive in, so a group order taken from the rows
    // themselves would show up as a failure here.
    state.subscriptions = [
      { id: "s2", name: "second", state: { state: "enabled" } },
      { id: "s1", name: "first", state: { state: "enabled" } },
    ] as Subscription[]
    const work = (subscription: string, url: string, title: string, posted: number) =>
      entry({
        subscription_id: subscription,
        candidate: {
          ...entry().candidate,
          source_url: url,
          title: { origin: title },
          posted_at: posted,
        },
      })
    state.entries = [
      work("s1", "e-hentai.org/g/1/aaaaaa", "Alpha", 100),
      work("s2", "e-hentai.org/g/2/bbbbbb", "Beta", 300),
      work("s1", "e-hentai.org/g/3/cccccc", "Gamma", 200),
    ]
    renderModal()

    const groupNames = () =>
      [...document.querySelectorAll(".preview-group-name")].map((node) => node.textContent)
    const titles = () =>
      [...document.querySelectorAll(".preview-row:not(.preview-head) .preview-title")].map(
        (node) => node.textContent,
      )

    // The filter's own order, and newest first inside each subscription.
    expect(groupNames()).toEqual(["second (1)", "first (2)"])
    expect(titles()).toEqual(["Beta", "Gamma", "Alpha"])
    // Sorting a column reorders the works inside a subscription, never the subscriptions.
    fireEvent.click(screen.getByRole("button", { name: "Work" }))
    expect(groupNames()).toEqual(["second (1)", "first (2)"])
    expect(titles()).toEqual(["Beta", "Alpha", "Gamma"])
  })

  it("falls back to the cycle's own verdict when the download is still in flight", () => {
    state.entries = [entry({ download: { id: "q1", state: "downloading", can_retry: false } as HistoryEntry["download"] })]
    renderModal()

    expect(screen.getByText("queued for download")).toBeTruthy()
    expect(screen.getByLabelText("Downloading")).toBeTruthy()
  })
})
