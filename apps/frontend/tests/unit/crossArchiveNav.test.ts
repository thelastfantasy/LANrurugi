import { beforeEach, describe, expect, it, vi } from "vitest"

import { resolveAdjacentArchive, setupArchiveNavigation } from "@/pages/Reader/crossArchiveNav"

describe("resolveAdjacentArchive", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("returns null when there is no cached id list", () => {
    expect(resolveAdjacentArchive({ ids: [], index: -1 }, "next")).toBeNull()
  })

  it("steps to the next id within the cached list", () => {
    const nav = { ids: ["a", "b", "c"], index: 0 }
    expect(resolveAdjacentArchive(nav, "next")).toBe("b")
  })

  it("steps to the previous id within the cached list", () => {
    const nav = { ids: ["a", "b", "c"], index: 2 }
    expect(resolveAdjacentArchive(nav, "prev")).toBe("b")
  })

  it("returns null at the next-edge when no prefetched next page is cached", () => {
    const nav = { ids: ["a", "b", "c"], index: 2 }
    expect(resolveAdjacentArchive(nav, "next")).toBeNull()
  })

  it("shifts the cache window forward using the prefetched next page at the edge", () => {
    localStorage.setItem("nextArchiveIds", JSON.stringify(["d", "e"]))
    localStorage.setItem("currDatatablesPage", "1")

    const nav = { ids: ["a", "b", "c"], index: 2 }
    const result = resolveAdjacentArchive(nav, "next")

    expect(result).toBe("d")
    expect(localStorage.getItem("nextArchiveIds")).toBeNull()
    expect(JSON.parse(localStorage.getItem("currArchiveIds") ?? "[]")).toEqual(["d", "e"])
    expect(JSON.parse(localStorage.getItem("previousArchiveIds") ?? "[]")).toEqual(["a", "b", "c"])
    expect(localStorage.getItem("currDatatablesPage")).toBe("2")
  })

  it("shifts the cache window backward using the prefetched previous page at the edge", () => {
    localStorage.setItem("previousArchiveIds", JSON.stringify(["x", "y"]))
    localStorage.setItem("currDatatablesPage", "2")

    const nav = { ids: ["a", "b", "c"], index: 0 }
    const result = resolveAdjacentArchive(nav, "prev")

    expect(result).toBe("y")
    expect(localStorage.getItem("previousArchiveIds")).toBeNull()
    expect(JSON.parse(localStorage.getItem("currArchiveIds") ?? "[]")).toEqual(["x", "y"])
    expect(JSON.parse(localStorage.getItem("nextArchiveIds") ?? "[]")).toEqual(["a", "b", "c"])
    expect(localStorage.getItem("currDatatablesPage")).toBe("1")
  })
})

describe("setupArchiveNavigation", () => {
  beforeEach(() => {
    localStorage.clear()
    sessionStorage.clear()
    vi.stubGlobal("fetch", vi.fn())
  })

  it("returns empty state for a direct navigation (no referrer)", async () => {
    Object.defineProperty(document, "referrer", { value: "", configurable: true })
    const result = await setupArchiveNavigation("archive-1")
    expect(result).toEqual({ ids: [], index: -1 })
  })

  it("returns empty state when the navigation-state handoff is missing", async () => {
    Object.defineProperty(document, "referrer", {
      value: `${window.location.origin}/`,
      configurable: true,
    })
    const result = await setupArchiveNavigation("archive-1")
    expect(result).toEqual({ ids: [], index: -1 })
  })

  it("resolves the current archive index within the recorded search results", async () => {
    Object.defineProperty(document, "referrer", {
      value: `${window.location.origin}/`,
      configurable: true,
    })
    sessionStorage.setItem("navigationState", "datatables")
    localStorage.setItem("currArchiveIds", JSON.stringify(["archive-1", "archive-2", "archive-3"]))

    const result = await setupArchiveNavigation("archive-2")
    expect(result).toEqual({ ids: ["archive-1", "archive-2", "archive-3"], index: 1 })
  })

  it("returns empty state when the archive id is not found in the recorded list", async () => {
    Object.defineProperty(document, "referrer", {
      value: `${window.location.origin}/`,
      configurable: true,
    })
    sessionStorage.setItem("navigationState", "datatables")
    localStorage.setItem("currArchiveIds", JSON.stringify(["archive-1", "archive-2"]))

    const result = await setupArchiveNavigation("not-in-list")
    expect(result).toEqual({ ids: [], index: -1 })
  })
})
