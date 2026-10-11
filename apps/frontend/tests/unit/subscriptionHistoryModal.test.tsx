import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen } from "@testing-library/react"
import { MemoryRouter } from "react-router-dom"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type { HistoryEntry } from "@/api/types"

// The history modal's own two states: a first page still arriving (skeleton) and rows whose
// *outcome* — not just the cycle's decision — is what the reader needs.
const state = {
  entries: [] as HistoryEntry[],
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
}))

const { SubscriptionHistoryModal } = await import("@/pages/Settings/SubscriptionHistoryModal")

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
        <SubscriptionHistoryModal onClose={() => {}} />
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

describe("SubscriptionHistoryModal", () => {
  beforeEach(() => {
    state.entries = []
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
    expect(screen.getByDisplayValue("Reconsider")).toBeTruthy()
  })

  it("falls back to the cycle's own verdict when the download is still in flight", () => {
    state.entries = [entry({ download: { id: "q1", state: "downloading", can_retry: false } as HistoryEntry["download"] })]
    renderModal()

    expect(screen.getByText("queued for download")).toBeTruthy()
    expect(screen.getByLabelText("Downloading")).toBeTruthy()
  })
})
