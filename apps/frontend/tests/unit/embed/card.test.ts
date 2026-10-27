import { beforeEach, describe, expect, it, vi } from "vitest"

import { type CardData, renderCard } from "@/embed/card"
import { cardText } from "@/embed/text"

// The QR canvas draws asynchronously through a lazy import; the card's layout is what is tested.
vi.mock("@/share/qr", () => ({ renderQr: vi.fn(async () => {}) }))

const t = cardText()
const base: CardData = {
  status: "active",
  restricted: false,
  title: "A share",
  item_count: 2,
  covers: ["a", "b"],
  permissions: { import: false, read: true, download: true },
  token: "t",
}

let root: HTMLElement
const draw = (data: CardData | null, options?: { coverRows?: 1 | 2; failure?: "unavailable" | "retry" }) =>
  renderCard(root, "K7QM9XRT2H", data, t, options)
const links = () => [...root.querySelectorAll("a")].map((a) => a.textContent)
const text = () => root.textContent ?? ""

describe("share card", () => {
  beforeEach(() => {
    document.body.innerHTML = ""
    root = document.createElement("div")
    document.body.append(root)
  })

  it("offers only the actions the share permits", () => {
    draw({ ...base, permissions: { import: false, read: true, download: false } })
    expect(links()).toEqual([t.read])
    expect(root.querySelector("canvas")).toBeNull()
  })

  it("leads with the code for an import-only share", () => {
    draw({ ...base, permissions: { import: true, read: false, download: false } })
    expect(text()).toContain("lrrg:K7QM9-XRT2H")
    expect(root.querySelector("canvas")).not.toBeNull()
    expect(root.querySelector("img")).toBeNull()
    expect(links()).toEqual([])
  })

  it("shows nothing of a restricted share's content", () => {
    draw({ ...base, restricted: true })
    expect(text()).toContain(t.restricted)
    expect(text()).not.toContain("A share")
    expect(root.querySelector("img")).toBeNull()
  })

  it("shows a used-up share's content with its download gone", () => {
    draw({ ...base, status: "exhausted", permissions: { import: true, read: true, download: true } })
    expect(text()).toContain("A share")
    expect(root.querySelector(".share-status-exhausted")).not.toBeNull()
    expect(links()).toEqual([t.read])
  })

  it("shows a closed share's state and its actions disabled, and nothing of its content", () => {
    for (const status of ["revoked", "expired"] as const) {
      draw({ status, restricted: false, permissions: { import: true, read: true, download: true } })
      expect(text()).toContain(t.closed[status].title)
      expect(text()).not.toContain("A share")
      expect(root.querySelector("img, canvas")).toBeNull()
      const buttons = [...root.querySelectorAll("button")]
      expect(buttons.map((b) => b.textContent)).toEqual([t.read, t.download, t.copyCode])
      expect(buttons.every((b) => b.disabled)).toBe(true)
      expect(links()).toEqual([])
    }
  })

  it("shows the size beside the item count", () => {
    draw({ ...base, total_size: 1_234_567 })
    expect(text()).toContain(t.items(2))
    expect(text()).toContain("1.18 MB")
  })

  it("shows the size in the single-item layout too", () => {
    draw({ ...base, item_count: 1, covers: ["a"], cover_titles: ["A"], total_size: 2_000_000 })
    expect(text()).toContain("1.91 MB")
  })

  it("says only that a share it cannot find is unavailable", () => {
    draw(null)
    expect(text()).toBe(t.unavailable)
    expect(links()).toEqual([])
  })

  it("offers a retry when the card could not be loaded", () => {
    draw(null, { failure: "retry" })
    expect(text()).toContain(t.loadFailed)
    expect(links()).toEqual([t.retry])
  })

  it("closes the cover row with a +N tile that lists every item", () => {
    draw({
      ...base,
      item_count: 5,
      covers: ["a", "b", "c", "d"],
      items: [...["a", "b", "c", "d"].map((id) => ({ title: `Item ${id}`, cover: id })), { title: "Set", cover: "e", volumes: 3 }],
    })
    const more = root.querySelector<HTMLButtonElement>(".lrrg-card-more")
    expect(more?.textContent).toBe("+1")
    more?.click()
    const rows = [...root.querySelectorAll(".lrrg-card-list li")].map((li) => li.textContent)
    expect(rows).toHaveLength(5)
    expect(rows[4]).toBe(`Set${t.volumes(3)}`)
    window.dispatchEvent(new Event("blur"))
    expect(root.querySelector(".lrrg-card-list")).toBeNull()
  })

  it("fills two rows of covers for eight items or more", () => {
    const ids = ["a", "b", "c", "d", "e", "f", "g", "h"]
    draw({
      ...base,
      item_count: 8,
      covers: ids,
      items: ids.map((id) => ({ title: `Item ${id}`, cover: id })),
    })
    // Seven covers plus the "+1" tile: eight tiles, the second row full.
    expect(root.querySelectorAll(".lrrg-card-covers img")).toHaveLength(7)
    expect(root.querySelector(".lrrg-card-more")?.textContent).toBe("+1")
    expect(root.querySelectorAll(".lrrg-card-covers > *")).toHaveLength(8)
    expect(root.querySelector(".lrrg-card-covers")?.className).toContain("lrrg-card-covers-two-rows")
  })

  it("falls back to one row when the frame is too short for two", () => {
    const ids = ["a", "b", "c", "d", "e", "f", "g", "h"]
    draw({ ...base, item_count: 8, covers: ids, items: ids.map((id) => ({ title: id, cover: id })) }, { coverRows: 1 })
    expect(root.querySelectorAll(".lrrg-card-covers img")).toHaveLength(4)
    expect(root.querySelector(".lrrg-card-more")?.textContent).toBe("+4")
    expect(root.querySelector(".lrrg-card-covers")?.className).toContain("lrrg-card-covers-more")
  })

  it("keeps one row of four for seven items", () => {
    const ids = ["a", "b", "c", "d", "e", "f", "g"]
    draw({ ...base, item_count: 7, covers: ids, items: ids.map((id) => ({ title: id, cover: id })) })
    expect(root.querySelectorAll(".lrrg-card-covers img")).toHaveLength(4)
    expect(root.querySelector(".lrrg-card-more")?.textContent).toBe("+3")
    expect(root.querySelector(".lrrg-card-covers")?.className).not.toContain("two-rows")
  })

  it("switches the item list between list, icons and thumbnails, and remembers it", () => {
    localStorage.removeItem("lrrg-card-view")
    draw({ ...base, item_count: 5, covers: ["a"], items: [{ title: "x", cover: "a" }] })
    root.querySelector<HTMLButtonElement>(".lrrg-card-more")?.click()
    const list = () => root.querySelector(".lrrg-card-list ul")?.className
    expect(list()).toBe("lrrg-card-list-list")
    root.querySelector<HTMLButtonElement>(`.lrrg-card-view[aria-label="${t.viewThumbs}"]`)?.click()
    expect(list()).toBe("lrrg-card-list-thumbs")
    window.dispatchEvent(new Event("blur"))
    root.querySelector<HTMLButtonElement>(".lrrg-card-more")?.click()
    expect(list()).toBe("lrrg-card-list-thumbs")
    expect(root.querySelector(".lrrg-card-view[aria-pressed=\"true\"]")?.getAttribute("aria-label")).toBe(t.viewThumbs)
  })
})
