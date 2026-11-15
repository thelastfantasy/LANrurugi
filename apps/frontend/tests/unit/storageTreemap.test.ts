import { describe, expect, it } from "vitest"

import {
  buildGroups,
  buildTiles,
  estimateTextWidth,
  fitText,
  MAX_ARCHIVES,
  MAX_GROUPS,
  progressBandKey,
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
    lastReadPage: 0,
    lastReadTime: 0,
    ...overrides,
  }
}

const LABELS = {
  unknownArtist: "unknown artist",
  unknownCategory: "unknown category",
  progressBand: (key: string) => `band ${key}`,
}

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

describe("progressBandKey", () => {
  it("separates the two states that are not a percentage", () => {
    expect(progressBandKey(archive({ id: "a", bytes: 1 }))).toBe("unread")
    // A timestamp alone is the server's other half of "has been opened", and it lands in the
    // lowest percentage band (not "unread") because the ratio is still zero — same as the server.
    expect(progressBandKey(archive({ id: "b", bytes: 1, lastReadTime: 1_700_000_000 }))).toBe("1_10")
    expect(progressBandKey(archive({ id: "c", bytes: 1, pages: 10, lastReadPage: 10 }))).toBe("finished")
  })

  it("puts a round tenth on the lower band, not the one above", () => {
    // 1 of 10 pages is exactly 10%; 0.1 * 100 is 10.000000000000002 in floating point, which is
    // why the band comes from the page integers instead.
    expect(progressBandKey(archive({ id: "a", bytes: 1, pages: 10, lastReadPage: 1 }))).toBe("1_10")
    expect(progressBandKey(archive({ id: "b", bytes: 1, pages: 10, lastReadPage: 9 }))).toBe("81_90")
    expect(progressBandKey(archive({ id: "c", bytes: 1, pages: 100, lastReadPage: 91 }))).toBe("91_99")
  })

  it("tiles the whole range without a gap", () => {
    const ids = [1, 2, 5, 11, 12, 23, 45, 55, 66, 77, 88, 99]
    expect(ids.map((page) => progressBandKey(archive({ id: "x", bytes: 1, pages: 100, lastReadPage: page }))))
      .toEqual(["1_10", "1_10", "1_10", "11_20", "11_20", "21_30", "41_50", "51_60", "61_70", "71_80", "81_90", "91_99"])
  })

  it("treats an opened archive with no page count as half read, like the server", () => {
    expect(progressBandKey(archive({ id: "a", bytes: 1, pages: 0, lastReadPage: 3 }))).toBe("41_50")
  })
})

describe("buildGroups", () => {
  it("puts every read archive under one 已读 group, bands hanging off it", () => {
    const groups = buildGroups(
      [
        archive({ id: "a", bytes: 100, pages: 10, lastReadPage: 1 }),
        archive({ id: "b", bytes: 300, pages: 10, lastReadPage: 1 }),
        archive({ id: "c", bytes: 50 }),
        archive({ id: "d", bytes: 200, pages: 10, lastReadPage: 10 }),
      ],
      "progress",
      LABELS,
    )
    // 已读 first (600 bytes), 未读 second (50): the state is the root, not the band.
    expect(groups.map((group) => group.key)).toEqual(["progress:read", "progress:unread"])
    expect(groups[0].label).toBe("band read")
    expect(groups[0].bytes).toBe(600)
    expect(groups[0].archives.map((entry) => entry.id)).toEqual(["b", "d", "a"])
    expect(groups[1].label).toBe("band unread")
    expect(groups[1].archives.map((entry) => entry.id)).toEqual(["c"])
  })

  it("gives 已读 one band per ten percent, with 100% its own band", () => {
    const groups = buildGroups(
      [
        archive({ id: "a", bytes: 100, pages: 10, lastReadPage: 1 }),
        archive({ id: "b", bytes: 300, pages: 10, lastReadPage: 10 }),
        archive({ id: "c", bytes: 50, pages: 100, lastReadPage: 95 }),
        archive({ id: "d", bytes: 50 }),
      ],
      "progress",
      LABELS,
    )
    const read = groups.find((group) => group.key === "progress:read")
    // Biggest band first, and only bands that actually hold something are listed.
    expect(read?.bands?.map((band) => band.key)).toEqual([
      "progress:read:finished",
      "progress:read:1_10",
      "progress:read:91_99",
    ])
    expect(read?.bands?.find((band) => band.key === "progress:read:finished")?.bytes).toBe(300)
    // The read group itself is coloured as its biggest band, so the tile matches what is one
    // click below it.
    expect(read?.colorKey).toBe("finished")
  })

  it("drops the 已读 group when nothing has been opened", () => {
    const groups = buildGroups([archive({ id: "a", bytes: 100 })], "progress", LABELS)
    expect(groups.map((group) => group.key)).toEqual(["progress:unread"])
    expect(groups[0].bands).toBeUndefined()
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

  it("opens the 已读 tile as its bands, not as every read archive at once", () => {
    const progressLibrary = [
      archive({ id: "a", bytes: 100, pages: 10, lastReadPage: 1 }),
      archive({ id: "b", bytes: 300, pages: 10, lastReadPage: 10 }),
      archive({ id: "c", bytes: 50 }),
    ]
    const level: StorageLevel = {
      kind: "groups",
      title: "All",
      groups: buildGroups(progressLibrary, "progress", LABELS),
      colorBy: "progress",
    }
    const tiles = buildTiles(level, 1_000_000, label)
    const read = tiles.find((tile) => tile.tileKey === "progress:read")
    expect(read?.drillTo?.kind).toBe("groups")
    expect(read?.drillTo?.kind === "groups" ? read.drillTo.groups.map((group) => group.key) : []).toEqual([
      "progress:read:finished",
      "progress:read:1_10",
    ])
    // 未读 still opens its archives directly — there is nothing to subdivide.
    expect(tiles.find((tile) => tile.tileKey === "progress:unread")?.drillTo?.kind).toBe("archives")
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
