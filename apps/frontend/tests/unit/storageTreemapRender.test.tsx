import { render } from "@testing-library/react"
import { Treemap } from "recharts"
import { describe, expect, it } from "vitest"

import { buildGroups, buildTiles, type StorageArchive, type StorageLevel } from "@/pages/Stats/storageTreemap"

// The chart is what turns the tile list into boxes. What is worth pinning down here is that
// recharts accepts our tiles (they carry extra fields) and draws exactly one rect per tile — the
// rest of the interaction (drill, select) is our own handlers on those rects.

function archive(id: string, bytes: number, artist: string): StorageArchive {
  return { id, title: id, bytes, extension: "zip", pages: 10, artist, category: "" }
}

function renderTiles(level: StorageLevel) {
  const tiles = buildTiles(level, 400_000, (count) => `other ${count}`)
  const { container } = render(
    <Treemap
      width={600}
      height={400}
      data={tiles}
      dataKey="value"
      nameKey="name"
      type="flat"
      nodeInset={1}
      nodeGap={1}
      isAnimationActive={false}
      isUpdateAnimationActive={false}
      content={(props) => (
        <rect data-depth={props.depth} x={props.x} y={props.y} width={props.width} height={props.height} />
      )}
    />,
  )
  return { container, tiles }
}

describe("storage treemap layout", () => {
  const library = [archive("a", 5000, "alice"), archive("b", 3000, "alice"), archive("c", 2000, "bob")]

  it("renders one box per tile at the file level", () => {
    const { container, tiles } = renderTiles({
      kind: "archives",
      title: "All",
      archives: library,
      colorBy: "artist",
    })
    // +1 for recharts' synthetic root, which the real renderer skips.
    expect(tiles).toHaveLength(3)
    expect(container.querySelectorAll("rect[data-depth]")).toHaveLength(tiles.length + 1)
  })

  it("renders one box per tile at the group level", () => {
    const { container, tiles } = renderTiles({
      kind: "groups",
      title: "All",
      groups: buildGroups(library, "artist", { unknownArtist: "-", unknownCategory: "-" }),
      colorBy: "artist",
    })
    expect(tiles).toHaveLength(2)
    expect(container.querySelectorAll("rect[data-depth]")).toHaveLength(tiles.length + 1)
  })
})
