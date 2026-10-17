import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { act, render, screen, waitFor } from "@testing-library/react"
import { useEffect, useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { useTankoubonReading } from "@/hooks/useTankoubonReading"

function jsonResponse(data: unknown) {
  return { ok: true, status: 200, json: () => Promise.resolve(data) } as Response
}

function Probe({ seen }: { seen: unknown[] }) {
  const { pages } = useTankoubonReading("tank-1")
  const [, setRenderCount] = useState(0)

  useEffect(() => {
    if (pages.data) seen.push(pages.data)
  }, [pages.data, seen])

  return (
    <button onClick={() => setRenderCount((n) => n + 1)}>
      {pages.data ? "loaded" : "loading"}
    </button>
  )
}

describe("useTankoubonReading", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it("keeps pages.data identity stable across unrelated reader re-renders", async () => {
    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input)
      if (url.includes("/api/tankoubons/tank-1/full")) {
        return jsonResponse({
          result: {
            id: "tank-1",
            name: "Tankoubon",
            summary: "",
            tags: "",
            archives: ["archive-a"],
            progress: 0,
            chapter_names: [],
            full_data: [
              { arcid: "archive-a", title: "Chapter A", size: 123, toc: [] },
            ],
          },
          total: 1,
          filtered: 1,
        })
      }
      if (url.includes("/api/archives/archive-a/files")) {
        return jsonResponse({
          job: 0,
          pages: [
            { url: "/api/archives/archive-a/page?path=1&optimize=1", is_patch: false },
          ],
        })
      }
      throw new Error(`unexpected fetch: ${url}`)
    }))

    const seen: unknown[] = []
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    })
    const { container } = render(
      <QueryClientProvider client={queryClient}>
        <Probe seen={seen} />
      </QueryClientProvider>,
    )

    await waitFor(() => expect(screen.getByRole("button")).toHaveTextContent("loaded"))
    const button = container.querySelector("button")!
    for (let i = 0; i < 3; i += 1) {
      await act(async () => {
        button.click()
      })
    }

    // The old implementation re-created `pages.data` on every render, which reset the reader's
    // debounced prefetch timer. With the memoized splice, unrelated renders must not change it.
    expect(seen.length).toBe(1)
    expect(seen[0]).toBeDefined()
  })
})
