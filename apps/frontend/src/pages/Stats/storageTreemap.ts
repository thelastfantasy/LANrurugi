/** Pure data shaping for `/stats`'s storage treemap: group the library, then decide how much of a
 *  level is worth drawing. No React and no DOM here on purpose — the "有详有略" policy (which tiles
 *  are drawn, which are folded into one "other" tile, where a label starts overflowing) is the part
 *  of the chart worth unit-testing without a browser.
 *
 *  The treemap renders one level at a time: groups first (when a grouping is selected), then the
 *  archives inside whichever group was clicked. Folded tiles are not lost — an "other" tile carries
 *  the archives it swallowed and is itself drillable, so the tail is deferred until it has a whole
 *  canvas to be drawn in. */


/** What the root level shows. `file` is the flat library (one box per archive), which is also what
 *  "group by path" or "group by title" would collapse to here: the content folder has no
 *  subdirectories, and every title is unique. */
export type StorageDimension = "file" | "artist" | "category"

/** Which tag a tile's colour is taken from. Mirrors the dimension, except that the flat file view
 *  colours by artist — colouring by format would paint 214 of this library's 229 archives the same
 *  colour, and colour has to distinguish neighbours to be worth anything. */
export type StorageColorKey = "artist" | "category"

export interface StorageArchive {
  id: string
  title: string
  bytes: number
  extension: string
  pages: number
  /** Raw tag values this archive's colour and grouping are read from. `""` when the tag is absent —
   *  an untagged archive is drawn in the neutral colour rather than being dropped. */
  artist: string
  category: string
}

export interface StorageGroup {
  /** Stable identity (`artist:somebody`, `category:manga`). */
  key: string
  /** Already display-ready: the artist/category as tagged, or a caller-supplied fallback for the
   *  untagged case (the caller owns translation, this module owns grouping). */
  label: string
  /** The raw tag value the group's colour is looked up by; `""` for the untagged bucket. */
  colorKey: string
  bytes: number
  /** Descending by size, which every consumer here relies on. */
  archives: StorageArchive[]
}

/** A group smaller than this cannot carry a readable label, so its bytes move into the level's
 *  "other" tile instead. Archives are deliberately *not* filtered by area: a treemap whose small
 *  files are drawn as slivers is exactly the texture SpaceSniffer has, and folding them would turn
 *  the tail of a large library into one meaningless grey block. */
export const MIN_GROUP_AREA_PX = 1800

/** Hard caps on top of the area test: a very wide, very short canvas has room for hundreds of
 *  narrow-but-legal rectangles, and past this many the layout stops being readable at all. The
 *  group cap is set from what this library actually contains (184 distinct artists) — a lower cap
 *  folds so much of the long tail into one "other" tile that the tile, not the artists, dominates
 *  the picture. The archive cap is generous because a sliver costs nothing when it is not
 *  labelled. */
export const MAX_GROUPS = 48

export const MAX_ARCHIVES = 600

export type StorageLevel =
  | { kind: "groups"; title: string; groups: StorageGroup[]; colorBy: StorageColorKey }
  | { kind: "archives"; title: string; archives: StorageArchive[]; colorBy: StorageColorKey }

/** One rectangle. `value` is bytes for leaves; recharts sums children for a parent, so a group's
 *  children must add up to the group's own bytes exactly (they do: the folded tail is one extra
 *  child, never a silent omission). */
export type StorageTile = {
  name: string
  value: number
  tileKey: string
  tileKind: "group" | "archive" | "other"
  /** Raw tag value the fill is looked up by; empty means the neutral fill. */
  colorKey: string
  /** Level a click opens, when this tile is a group or a folded "other". */
  drillTo?: StorageLevel
  /** Archive a click selects, when this tile is an archive drawn at its own level. */
  select?: StorageArchive
}

/** Split `items` (descending by size) into the ones with enough room at `areaPx` to be drawn
 *  individually, and the tail that has to be folded.
 *
 *  Wall-clock cheap by construction: no layout, just the area a tile of `minAreaPx` would occupy
 *  relative to the whole level, converted into a byte cutoff. `minAreaPx = 0` means "fold by count
 *  only", which is what the archive level wants. */
export function splitVisible<T>(
  items: readonly T[],
  bytesOf: (item: T) => number,
  areaPx: number,
  minAreaPx: number,
  maxItems: number,
): { visible: T[]; rest: T[] } {
  const usableArea = Number.isFinite(areaPx) && areaPx > 0 ? areaPx : 0
  const cap = Math.max(0, maxItems)
  // Before the first measurement lands there is no area to divide by; showing the biggest `cap`
  // items is both useful and honest (it is what the reader will see a frame later anyway).
  if (usableArea === 0) {
    const visible = items.slice(0, cap)
    return { visible, rest: items.slice(visible.length) }
  }
  const total = items.reduce((sum, item) => sum + bytesOf(item), 0)
  if (total <= 0) return { visible: items.slice(0, cap), rest: items.slice(cap) }
  const minBytes = (Math.max(0, minAreaPx) / usableArea) * total
  const visible: T[] = []
  for (const item of items) {
    if (visible.length >= cap) break
    // Sorted descending, so the first item that fails also fails for everything after it.
    if (bytesOf(item) < minBytes) break
    visible.push(item)
  }
  // Never fold *everything*: the "other" tile opens a level made of exactly these items, and a
  // level that is once again one "other" tile would let a reader click forever without ever seeing
  // anything new. Drawing the biggest one (even as a sliver) guarantees the drill-down progresses.
  if (visible.length === 0 && items.length > 0 && cap > 0) visible.push(items[0])
  return { visible, rest: items.slice(visible.length) }
}

/** The library grouped by `dimension`, biggest group first.
 *
 *  An archive with several tags of the same namespace belongs entirely to its first one —
 *  duplicating it into every tag would make the treemap's total disagree with the library's own
 *  size, and a treemap whose areas do not add up is worse than one that under-reports a
 *  multi-credit work. */
export function buildGroups(
  archives: readonly StorageArchive[],
  dimension: Exclude<StorageDimension, "file">,
  labels: { unknownArtist: string; unknownCategory: string },
): StorageGroup[] {
  const byKey = new Map<string, StorageGroup>()
  for (const archive of archives) {
    const value = dimension === "artist" ? archive.artist : archive.category
    const label = value || (dimension === "artist" ? labels.unknownArtist : labels.unknownCategory)
    const key = `${dimension}:${value}`
    let group = byKey.get(key)
    if (!group) {
      group = { key, label, colorKey: value, bytes: 0, archives: [] }
      byKey.set(key, group)
    }
    group.bytes += archive.bytes
    group.archives.push(archive)
  }
  const groups = [...byKey.values()]
  for (const group of groups) group.archives.sort((a, b) => b.bytes - a.bytes)
  groups.sort((a, b) => b.bytes - a.bytes)
  return groups
}

function archiveTile(archive: StorageArchive, colorBy: StorageColorKey): StorageTile {
  return {
    name: archive.title,
    value: archive.bytes,
    tileKey: `archive:${archive.id}`,
    tileKind: "archive",
    colorKey: colorBy === "artist" ? archive.artist : archive.category,
    select: archive,
  }
}

/** Split a level's children into the tiles that fit and (optionally) one folded "other" tile.
 *
 *  `areaPx` is the space the *level* owns, so an archive set that was one group's contents gets
 *  the whole canvas once it is drilled into — which is the "有详有略" mechanism: click a directory
 *  and its contents get bigger instead of being re-listed. */
export function buildTiles(
  level: StorageLevel,
  areaPx: number,
  otherLabel: (count: number) => string,
): StorageTile[] {
  if (level.kind === "archives") {
    const { visible, rest } = splitVisible(level.archives, (archive) => archive.bytes, areaPx, 0, MAX_ARCHIVES)
    const tiles = visible.map((archive) => archiveTile(archive, level.colorBy))
    if (rest.length > 0) {
      const restBytes = rest.reduce((sum, archive) => sum + archive.bytes, 0)
      const title = otherLabel(rest.length)
      tiles.push({
        name: title,
        value: restBytes,
        tileKey: "other:archives",
        tileKind: "other",
        colorKey: "",
        drillTo: { kind: "archives", title, archives: rest, colorBy: level.colorBy },
      })
    }
    return tiles
  }

  const { visible, rest } = splitVisible(
    level.groups,
    (group) => group.bytes,
    areaPx,
    MIN_GROUP_AREA_PX,
    MAX_GROUPS,
  )
  const tiles: StorageTile[] = visible.map((group) => ({
    name: group.label,
    value: group.bytes,
    tileKey: group.key,
    tileKind: "group" as const,
    colorKey: group.colorKey,
    drillTo: {
      kind: "archives" as const,
      title: group.label,
      archives: group.archives,
      colorBy: level.colorBy,
    },
  }))
  if (rest.length > 0) {
    const restBytes = rest.reduce((sum, group) => sum + group.bytes, 0)
    const title = otherLabel(rest.length)
    tiles.push({
      name: title,
      value: restBytes,
      tileKey: "other:groups",
      tileKind: "other",
      colorKey: "",
      drillTo: { kind: "groups", title, groups: rest, colorBy: level.colorBy },
    })
  }
  return tiles
}

/** Node name → colour, ranked by bytes so the library's biggest artists (or categories) get the
 *  first palette entries and the assignment is stable across drill levels. Untagged archives are
 *  deliberately absent: they take the neutral fill. */
export function rankColorKeys(
  archives: readonly StorageArchive[],
  colorBy: StorageColorKey,
): { key: string; bytes: number }[] {
  const totals = new Map<string, number>()
  for (const archive of archives) {
    const key = colorBy === "artist" ? archive.artist : archive.category
    if (!key) continue
    totals.set(key, (totals.get(key) ?? 0) + archive.bytes)
  }
  return [...totals.entries()]
    .map(([key, bytes]) => ({ key, bytes }))
    .sort((a, b) => b.bytes - a.bytes)
}

const ELLIPSIS = "…"

/** Approximate rendered width of `text`, in px, for a proportional UI font.
 *
 *  SVG has no `text-overflow`, so a label that does not fit has to be measured and cut *before* it
 *  is drawn — and `getComputedStyle`/`measureText` are not available where this is also used (unit
 *  tests, and before the first paint). Per-character advances are the usual blend of a sans font's
 *  own metrics, deliberately erring wide for CJK (they are square) and for capitals. */
export function estimateTextWidth(text: string, fontSizePx: number): number {
  let em = 0
  for (const char of text) em += charWidthEm(char)
  return em * fontSizePx
}

function charWidthEm(char: string): number {
  const code = char.codePointAt(0) ?? 0
  if (code === 0x20) return 0.3
  // CJK, kana, hangul and full-width punctuation are one em wide.
  if (code >= 0x2e80) return 1.0
  if (code >= 0x41 && code <= 0x5a) return 0.65 // A-Z
  if (code >= 0x30 && code <= 0x39) return 0.56 // 0-9
  if (code >= 0x61 && code <= 0x7a) return 0.54 // a-z
  return 0.42
}

/** `text` cut to fit `maxWidthPx`, with a trailing ellipsis; "" when even the ellipsis cannot fit.
 *  A 2px safety margin is built in because the estimate above is not the real font's metrics. */
export function fitText(text: string, maxWidthPx: number, fontSizePx: number): string {
  const budget = maxWidthPx - 2
  if (!text || budget <= 0) return ""
  if (estimateTextWidth(text, fontSizePx) <= budget) return text
  const ellipsisWidth = estimateTextWidth(ELLIPSIS, fontSizePx)
  if (budget < ellipsisWidth) return ""
  let width = 0
  let out = ""
  for (const char of text) {
    width += charWidthEm(char) * fontSizePx
    if (width + ellipsisWidth > budget) break
    out += char
  }
  return out ? `${out.trimEnd()}${ELLIPSIS}` : ""
}
