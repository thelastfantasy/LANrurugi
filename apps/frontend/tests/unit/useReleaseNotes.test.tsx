import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { renderHook, waitFor } from "@testing-library/react"
import type { ReactNode } from "react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { useReleaseNotes } from "@/api/hooks"
import type { ChangelogRelease, VersionCheckResponse } from "@/api/types"
import { readSeenBuild, writeSeenBuild } from "@/lib/seenBuild"

const { fetchJsonMock } = vi.hoisted(() => ({ fetchJsonMock: vi.fn() }))
vi.mock("@/api/client", () => ({ fetchJson: fetchJsonMock }))

/** What `GET /info` reports for the running build; `version_name` is the short commit. */
const RUNNING = { version: "0.2.0", version_name: "bbbbbbb" }
const SEEN = { version: "0.1.0", sha: "aaaaaaa" }

function versionResponse(changelog?: ChangelogRelease[]): VersionCheckResponse {
  return {
    enabled: true,
    details: { version: RUNNING.version, sha: RUNNING.version_name },
    data: { isUpstream: false, isRelease: true, isLatest: true, version: null, latest: null },
    ...(changelog ? { changelog } : {}),
    cached: false,
  }
}

const ENTRY: ChangelogRelease = {
  tag: "v0.2.0",
  sha: "bbbbbbb",
  published_at: "2026-10-14T00:00:00Z",
  url: "https://example.test/releases/v0.2.0",
  body: "notes",
}

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
  }
}

function mockBackend(version: VersionCheckResponse | Error) {
  fetchJsonMock.mockImplementation(async (path: string) => {
    if (path === "/info") return RUNNING
    if (typeof path === "string" && path.startsWith("/version")) {
      if (version instanceof Error) throw version
      return version
    }
    throw new Error(`unexpected fetch: ${path}`)
  })
}

/** Every path this hook asked the backend for, in order. */
function requestedPaths(): string[] {
  return fetchJsonMock.mock.calls.map((call) => String(call[0]))
}

describe("useReleaseNotes", () => {
  beforeEach(() => {
    localStorage.clear()
    fetchJsonMock.mockReset()
  })

  it("remembers the running build on a first visit without asking for a changelog", async () => {
    mockBackend(versionResponse())

    const { result } = renderHook(() => useReleaseNotes(true), { wrapper: wrapper() })

    await waitFor(() => expect(readSeenBuild()).toEqual({ version: "0.2.0", sha: "bbbbbbb" }))
    expect(result.current.data).toBeNull()
    expect(requestedPaths().some((p) => p.startsWith("/version"))).toBe(false)
  })

  it("stays quiet when the remembered build is the running one", async () => {
    writeSeenBuild({ version: "0.2.0", sha: "bbbbbbb" })
    mockBackend(versionResponse([ENTRY]))

    const { result } = renderHook(() => useReleaseNotes(true), { wrapper: wrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.data).toBeNull()
    expect(requestedPaths().some((p) => p.startsWith("/version"))).toBe(false)
  })

  it("asks for the window between the remembered build and the running one", async () => {
    writeSeenBuild(SEEN)
    mockBackend(versionResponse([ENTRY]))

    const { result } = renderHook(() => useReleaseNotes(true), { wrapper: wrapper() })

    await waitFor(() => expect(result.current.data).toEqual([ENTRY]))
    expect(requestedPaths()).toContain("/version?since=aaaaaaa")
    // The marker moves forward, so the same upgrade is not announced twice.
    expect(readSeenBuild()).toEqual({ version: "0.2.0", sha: "bbbbbbb" })
  })

  it("does not spend the notice when the backend could not reach GitHub", async () => {
    writeSeenBuild(SEEN)
    // No `changelog` key at all: the backend omits it when it could not ask, as opposed to
    // answering with an empty array when it could and found nothing.
    mockBackend(versionResponse())

    const { result } = renderHook(() => useReleaseNotes(true), { wrapper: wrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.data).toBeNull()
    // Still the old build, so the next page load tries again.
    expect(readSeenBuild()).toEqual(SEEN)
  })

  it("moves the marker on after an empty window, which is a real answer", async () => {
    writeSeenBuild(SEEN)
    mockBackend(versionResponse([]))

    const { result } = renderHook(() => useReleaseNotes(true), { wrapper: wrapper() })

    await waitFor(() => expect(result.current.isSuccess).toBe(true))
    expect(result.current.data).toBeNull()
    expect(readSeenBuild()).toEqual({ version: "0.2.0", sha: "bbbbbbb" })
  })

  it("asks GitHub nothing when disabled", async () => {
    writeSeenBuild(SEEN)
    mockBackend(versionResponse([ENTRY]))

    const { result } = renderHook(() => useReleaseNotes(false), { wrapper: wrapper() })

    await waitFor(() => expect(result.current.fetchStatus).toBe("idle"))
    // `useServerInfo` still runs — `/info` is public and fetched app-wide by the Footer — but the
    // changelog request itself must not.
    expect(requestedPaths().some((path) => path.startsWith("/version"))).toBe(false)
    expect(readSeenBuild()).toEqual(SEEN)
  })
})
