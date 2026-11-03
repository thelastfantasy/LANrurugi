import { describe, expect, it } from "vitest"

import {
  buildGroups,
  buildTiles,
  estimateTextWidth,
  fitText,
  MAX_ARCHIVES,
  MAX_GROUPS,
  rankColorKeys,
  splitVisible,
  type StorageArchive,
  type StorageLevel,
  type StorageTile,
} from "@/pages/Stats/storageTreemap"

function archive(overrides: Partial<StorageArchive> & { id: string; bytes: number }): StorageArchive {
  return {
    title: overrides.id,
    extension: "zip",
    pages: 10,
    artist: "",
    category: "",
    ...overrides,
  }
}

const LABELS = { unknownArtist: "unknown artist", unknownCategory: "unknown category" }

function bytesOfTiles(tiles: StorageTile[]): number {
  return tiles.reduce((sum, tile) => sum + tile.value, 0)
}

describe("buildGroups", () => {
  it("groups by artist and orders groups and members by size", () => {
    const groups = buildGroups(
      [
        archive({ id: "a", bytes: 100, artist: "alice" }),
        archive({ id: "b", bytes: 300, artist: "bob" }),
        archive({ id: "c", bytes: 50, artist: "alice" }),
      ],
      "artist",
      LABELS,
    )

    expect(groups.map((group) => group.label)).toEqual(["bob", "alice"])
    expect(groups[1].bytes).toBe(150)
    expect(groups[1].archives.map((entry) => entry.id)).toEqual(["a", "c"])
    expect(groups[1].key).toBe("artist:alice")
    // The group's colour is looked up by the raw tag value, not the display label.
    expect(groups[1].colorKey).toBe("alice")
  })

  it("keeps an untagged archive in its own bucket rather than dropping it", () => {
    const groups = buildGroups([archive({ id: "a", bytes: 10 })], "artist", LABELS)
    expect(groups).toHaveLength(1)
    expect(groups[0].label).toBe(LABELS.unknownArtist)
    expect(groups[0].key).toBe("artist:")
    expect(groups[0].colorKey).toBe("")
  })

  it("groups by category tag", () => {
    const groups = buildGroups(
      [
        archive({ id: "a", bytes: 100, category: "manga" }),
        archive({ id: "b", bytes: 40, category: "doujinshi" }),
        archive({ id: "c", bytes: 10 }),
      ],
      "category",
      LABELS,
    )
    expect(groups.map((group) => group.label)).toEqual(["manga", "doujinshi", LABELS.unknownCategory])
    expect(groups.reduce((sum, group) => sum + group.bytes, 0)).toBe(150)
  })
})

describe("rankColorKeys", () => {
  it("ranks tag values by bytes and skips the untagged bucket", () => {
    const ranked = rankColorKeys(
      [
        archive({ id: "a", bytes: 100, artist: "alice" }),
        archive({ id: "b", bytes: 300, artist: "bob" }),
        archive({ id: "c", bytes: 50, artist: "alice" }),
        archive({ id: "d", bytes: 900 }),
      ],
      "artist",
    )
    expect(ranked).toEqual([
      { key: "bob", bytes: 300 },
      { key: "alice", bytes: 150 },
    ])
  })
})

describe("splitVisible", () => {
  const items = [300, 200, 100, 50, 25].map((bytes) => ({ bytes }))
  const bytesOf = (item: { bytes: number }) => item.bytes

  it("folds everything below the area-equivalent cutoff into the tail", () => {
    // 100px of 1000px of canvas, of 675 bytes total: a tile needs ~67.5 bytes to be drawn.
    const { visible, rest } = splitVisible(items, bytesOf, 1000, 100, 10)
    expect(visible.map((item) => item.bytes)).toEqual([300, 200, 100])
    expect(rest.map((item) => item.bytes)).toEqual([50, 25])
  })

  it("treats an area threshold of zero as count-only folding", () => {
    const { visible, rest } = splitVisible(items, bytesOf, 1000, 0, 10)
    expect(visible).toHaveLength(items.length)
    expect(rest).toHaveLength(0)
  })

  it("caps the count even when every tile would be big enough", () => {
    const { visible, rest } = splitVisible(items, bytesOf, 100000, 1, 2)
    expect(visible.map((item) => item.bytes)).toEqual([300, 200])
    expect(rest).toHaveLength(3)
  })

  it("shows the biggest items before the canvas has been measured", () => {
    const { visible, rest } = splitVisible(items, bytesOf, 0, 100, 3)
    expect(visible.map((item) => item.bytes)).toEqual([300, 200, 100])
    expect(rest).toHaveLength(2)
  })

  it("always draws the biggest tile so drilling into the folded tail cannot loop forever", () => {
    const { visible, rest } = splitVisible(items, bytesOf, 10, 100000, MAX_GROUPS)
    expect(visible.map((item) => item.bytes)).toEqual([300])
    expect(rest.map((item) => item.bytes)).toEqual([200, 100, 50, 25])
  })
})

/** The "有详有略" contract: at any canvas size the level's tiles must still add up to the level's
 *  bytes, and whatever was folded away must be reachable by clicking the "other" tile. */
describe("buildTiles", () => {
  const library = [
    archive({ id: "zip-big", bytes: 4000, artist: "alice", category: "manga" }),
    archive({ id: "zip-2", bytes: 900, artist: "alice", category: "manga" }),
    archive({ id: "pdf-1", bytes: 5000, artist: "bob", category: "doujinshi" }),
    archive({ id: "rar-1", bytes: 700, artist: "carol", category: "cosplay" }),
    archive({ id: "rar-2", bytes: 300, artist: "carol", category: "cosplay" }),
  ]
  const label = (count: number) => `other ${count}`

  it("draws every archive at the file level, slivers included", () => {
    const level: StorageLevel = { kind: "archives", title: "All", archives: library, colorBy: "artist" }
    // A canvas far too small for five tiles: the archive level folds by count only, so the mosaic
    // keeps its slivers instead of collapsing into one grey block.
    const tiles = buildTiles(level, 100, label)
    expect(tiles.map((tile) => tile.name)).toEqual(library.map((entry) => entry.id))
    expect(bytesOfTiles(tiles)).toBe(10900)
    expect(tiles.every((tile) => tile.drillTo === undefined)).toBe(true)
    expect(tiles[0].colorKey).toBe("alice")
  })

  it("folds the archive tail once the count cap is reached, and the tail stays openable", () => {
    const many = Array.from({ length: MAX_ARCHIVES + 5 }, (_, index) =>
      archive({ id: `a${index}`, bytes: 10_000 - index }),
    )
    const level: StorageLevel = { kind: "archives", title: "All", archives: many, colorBy: "artist" }
    const tiles = buildTiles(level, 100, label)
    expect(tiles).toHaveLength(MAX_ARCHIVES + 1)
    const other = tiles[tiles.length - 1]
    expect(other.tileKey).toBe("other:archives")
    expect(other.drillTo?.kind).toBe("archives")
    // Opening it must recover the folded archives rather than losing them.
    const reopened = buildTiles(other.drillTo as StorageLevel, 100, label)
    expect(reopened).toHaveLength(5)
    expect(bytesOfTiles(tiles)).toBe(many.reduce((sum, entry) => sum + entry.bytes, 0))
    expect(bytesOfTiles(reopened)).toBe(other.value)
  })

  it("folds small groups into an other tile that can be opened", () => {
    const level: StorageLevel = {
      kind: "groups",
      title: "All",
      groups: buildGroups(library, "artist", LABELS),
      colorBy: "artist",
    }
    // A tiny canvas makes the area test fold everything except the biggest group.
    const tiles = buildTiles(level, 100, label)
    expect(tiles[0].tileKind).toBe("group")
    expect(tiles[0].drillTo?.kind).toBe("archives")
    const other = tiles.find((tile) => tile.tileKey === "other:groups")
    expect(other).toBeDefined()
    expect(other?.drillTo?.kind).toBe("groups")
    expect(bytesOfTiles(tiles)).toBe(10900)
  })
})

describe("fitText", () => {
  it("leaves text that fits untouched", () => {
    expect(fitText("abc", 200, 11)).toBe("abc")
  })

  it("truncates with an ellipsis instead of overflowing the tile", () => {
    const fitted = fitText("a very long archive title that will not fit", 80, 11)
    expect(fitted.endsWith("…")).toBe(true)
    expect(estimateTextWidth(fitted, 11)).toBeLessThanOrEqual(78)
  })

  it("accounts for CJK characters being a full em wide", () => {
    const fitted = fitText("这是一个非常长的中文标题需要被截断", 60, 11)
    expect(fitted.endsWith("…")).toBe(true)
    expect(estimateTextWidth(fitted, 11)).toBeLessThanOrEqual(58)
    // Three CJK glyphs plus the ellipsis is about all a 60px-wide tile can hold.
    expect([...fitted].length).toBeLessThanOrEqual(5)
  })

  it("returns nothing when even the ellipsis cannot fit", () => {
    expect(fitText("anything", 0, 11)).toBe("")
    expect(fitText("anything", 3, 11)).toBe("")
  })
})
