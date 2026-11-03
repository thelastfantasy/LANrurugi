import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import type { TreemapNode } from "recharts"
import { ResponsiveContainer, Tooltip, Treemap } from "recharts"

import type { ArchiveMetadata } from "@/api/types"
import { FormatBadge } from "@/components/Display"
import { useIsNarrowViewport } from "@/hooks/useIsNarrowViewport"
import { routes } from "@/lib/routes"
import { splitTagsByNamespace } from "@/lib/tagFormat"

import { useChartPalette } from "./chartPalette"
import { ChartTooltip, StatsSection, useStatsPanel } from "./Charts"
import { formatBytes, percentOf } from "./format"
import {
  buildGroups,
  buildTiles,
  fitText,
  rankColorKeys,
  type StorageArchive,
  type StorageColorKey,
  type StorageDimension,
  type StorageLevel,
  type StorageTile,
} from "./storageTreemap"

// A phone-sized canvas gets a shorter box (the section is already tall), and the squarify target
// follows the canvas' shape so a landscape box is not laid out as if it were square.
const NARROW_HEIGHT = 260
const WIDE_HEIGHT = 380
const NODE_INSET = 1
const NODE_GAP = 1

/** Neutral grey is the one colour literal: it is the "folded / untagged" fill, and the page already
 *  uses the same translucent grey for every bar track (`BarRow`), which reads correctly on all five
 *  themes precisely because it has no hue. */
const NEUTRAL_FILL = "rgba(127,127,127,0.20)"
const NEUTRAL_HOVER_FILL = "rgba(127,127,127,0.36)"

const DIMENSIONS: { value: StorageDimension; labelKey: string }[] = [
  { value: "file", labelKey: "stats.storageTreemapByFile" },
  { value: "artist", labelKey: "stats.storageTreemapByArtist" },
  { value: "category", labelKey: "stats.storageTreemapByCategory" },
]

/** `ResizeObserver`-measured box of the chart frame. The treemap needs real pixel area to decide
 *  how many group tiles are drawn individually, and this is the only place that number exists —
 *  recharts' own `ResponsiveContainer` keeps its measurement to itself. */
function useMeasuredSize<T extends HTMLElement>() {
  const ref = useRef<T | null>(null)
  const [size, setSize] = useState({ width: 0, height: 0 })
  useEffect(() => {
    const element = ref.current
    if (!element) return
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect
      if (!rect) return
      setSize((current) =>
        Math.abs(current.width - rect.width) < 1 && Math.abs(current.height - rect.height) < 1
          ? current
          : { width: rect.width, height: rect.height },
      )
    })
    observer.observe(element)
    return () => observer.disconnect()
  }, [])
  return [ref, size] as const
}

/** How the library's bytes are spread across archives — the `/stats` counterpart of a disk-usage
 *  treemap.
 *
 *  One level at a time rather than one deeply nested picture: at thousands of archives the nested
 *  version is unreadable and unclickable on a phone, while the drill-down version keeps every tap
 *  target a real size and still shows the tail (folded into "其他", which opens the same way).
 *
 *  The default level is the flat library — one box per archive. That is also what "group by path"
 *  or "group by title" would collapse to here: the content folder has no subdirectories and titles
 *  are unique, so both are this view with an extra click. */
export function StorageTreemap({ archives }: { archives: ArchiveMetadata[] | undefined }) {
  const { t } = useTranslation()
  const series = useChartPalette()
  const { palette: chrome, muted } = useStatsPanel()
  const narrow = useIsNarrowViewport("(max-width: 768px)")
  const [dimension, setDimension] = useState<StorageDimension>("file")
  const [stack, setStack] = useState<StorageLevel[]>([])
  const [selected, setSelected] = useState<StorageArchive | null>(null)
  const [hoveredKey, setHoveredKey] = useState<string | null>(null)
  const [frameRef, frameSize] = useMeasuredSize<HTMLDivElement>()

  const entries = useMemo<StorageArchive[]>(
    () =>
      (archives ?? [])
        .filter((archive) => archive.size > 0)
        .map((archive) => {
          const tags = splitTagsByNamespace(archive.tags)
          return {
            id: archive.arcid,
            title: archive.title || archive.filename,
            bytes: archive.size,
            extension: archive.extension ?? "",
            pages: archive.pagecount,
            artist: tags.artist?.[0] ?? "",
            category: tags.category?.[0] ?? "",
          }
        })
        // Biggest first: recharts lays tiles out in array order, and the file level wants its
        // largest rectangles where the eye lands first.
        .sort((a, b) => b.bytes - a.bytes),
    [archives],
  )

  const totalBytes = useMemo(() => entries.reduce((sum, entry) => sum + entry.bytes, 0), [entries])

  // One ranked colour map per tag namespace, shared by every level: an artist keeps their colour
  // whether the reader is looking at the whole library or inside one category.
  const colorMaps = useMemo(() => {
    const build = (colorBy: StorageColorKey) => {
      const colors = new Map<string, string>()
      rankColorKeys(entries, colorBy).forEach(({ key }, index) => {
        colors.set(key, series[index % series.length])
      })
      return colors
    }
    return { artist: build("artist"), category: build("category") }
  }, [entries, series])

  const colorFor = (colorBy: StorageColorKey, key: string) => colorMaps[colorBy].get(key) ?? NEUTRAL_FILL

  const rootLevel = useMemo<StorageLevel>(() => {
    const title = t("stats.storageTreemapAll")
    if (dimension === "file") return { kind: "archives", title, archives: entries, colorBy: "artist" }
    return {
      kind: "groups",
      title,
      groups: buildGroups(entries, dimension, {
        unknownArtist: t("stats.storageTreemapNoArtist"),
        unknownCategory: t("stats.storageTreemapNoCategory"),
      }),
      colorBy: dimension,
    }
  }, [dimension, entries, t])

  const level = stack.length > 0 ? stack[stack.length - 1] : rootLevel

  const height = narrow ? NARROW_HEIGHT : WIDE_HEIGHT
  // Before the first resize callback there is no measured area; a nominal desktop canvas keeps the
  // first paint from drawing every group of a large library.
  const areaPx = frameSize.width > 0 && frameSize.height > 0 ? frameSize.width * frameSize.height : 900 * height

  const tiles = useMemo(
    () => buildTiles(level, areaPx, (count) => t("stats.storageTreemapOther", { n: count })),
    [areaPx, level, t],
  )

  function openLevel(next: StorageLevel) {
    setStack((current) => [...current, next])
    setSelected(null)
    setHoveredKey(null)
  }

  function selectDimension(next: StorageDimension) {
    if (next === dimension) return
    setDimension(next)
    setStack([])
    setSelected(null)
    setHoveredKey(null)
  }

  function renderTile(props: TreemapNode) {
    const tile = props as unknown as TreemapNode & StorageTile
    const { x, y, width, height: tileHeight, depth } = tile
    // Depth 0 is recharts' synthetic root (the canvas itself), not one of our tiles.
    if (depth === 0 || width <= 0 || tileHeight <= 0) return <g />
    const isOther = tile.tileKind === "other"
    const hovered = hoveredKey === tile.tileKey
    const fill = isOther ? (hovered ? NEUTRAL_HOVER_FILL : NEUTRAL_FILL) : colorFor(level.colorBy, tile.colorKey)
    const clickable = Boolean(tile.drillTo || tile.select)
    return (
      <g>
        <rect
          x={x}
          y={y}
          width={width}
          height={tileHeight}
          fill={fill}
          fillOpacity={isOther ? 1 : hovered ? 0.82 : 0.62}
          stroke={isOther ? chrome.separator : "none"}
          strokeDasharray={isOther ? "3 2" : undefined}
          style={{ pointerEvents: "all", cursor: clickable ? "pointer" : "default" }}
          onClick={(event) => {
            if (!clickable) return
            // Selecting is this component's business, not recharts' tooltip state: without this a
            // tap would also pin the tooltip open on top of the detail panel it just opened.
            event.stopPropagation()
            if (tile.drillTo) openLevel(tile.drillTo)
            else if (tile.select) setSelected(tile.select)
          }}
          onMouseEnter={() => setHoveredKey(tile.tileKey)}
          onMouseLeave={() => setHoveredKey((current) => (current === tile.tileKey ? null : current))}
        />
        {renderLabel(tile, x, y, width, tileHeight)}
      </g>
    )
  }

  function renderLabel(tile: StorageTile, x: number, y: number, width: number, height: number) {
    const text = chrome.text
    if (tile.tileKind === "other") {
      // The folded tail has no name of its own, so it gets the one label that fits anywhere: a
      // count and its bytes, centred.
      if (width < 40 || height < 18) return null
      return (
        <g style={{ pointerEvents: "none" }}>
          <text x={x + width / 2} y={y + height / 2} textAnchor="middle" fontSize={11} fill={text}>
            {fitText(tile.name, width - 10, 11)}
          </text>
          {height >= 34 ? (
            <text
              x={x + width / 2}
              y={y + height / 2 + 13}
              textAnchor="middle"
              fontSize={10}
              fill={text}
              opacity={0.75}
            >
              {fitText(formatBytes(tile.value), width - 10, 10)}
            </text>
          ) : null}
        </g>
      )
    }
    // Every other tile is a name over a size, in the top-left corner — the read SpaceSniffer has,
    // and the only one that survives a tile being 60px wide.
    if (width < 46 || height < 18) return null
    const name = fitText(tile.name, width - 10, 11)
    if (!name) return null
    return (
      <g style={{ pointerEvents: "none" }}>
        <text
          x={x + 5}
          y={y + 14}
          fontSize={11}
          fontWeight={tile.tileKind === "group" ? 600 : 400}
          fill={text}
        >
          {name}
        </text>
        {height >= 34 && width >= 54 ? (
          <text x={x + 5} y={y + 27} fontSize={10} fill={text} opacity={0.75}>
            {formatBytes(tile.value)}
          </text>
        ) : null}
      </g>
    )
  }

  if (entries.length === 0) return null

  const selectedColorKey =
    dimension === "category" ? selected?.category : selected?.artist
  const selectedShare = selected ? percentOf(selected.bytes, totalBytes) : 0
  const crumbs = [
    { title: t("stats.storageTreemapAll"), index: 0 },
    ...stack.map((entry, index) => ({ title: entry.title, index: index + 1 })),
  ]
  const currentCount =
    level.kind === "groups"
      ? level.groups.reduce((sum, group) => sum + group.archives.length, 0)
      : level.archives.length
  const currentBytes =
    level.kind === "groups"
      ? level.groups.reduce((sum, group) => sum + group.bytes, 0)
      : level.archives.reduce((sum, archive) => sum + archive.bytes, 0)

  return (
    <StatsSection title={t("stats.storageTreemap")} icon="fa-chart-simple" hint={t("stats.storageTreemapHint")}>
      <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8, marginBottom: 8 }}>
        <span style={{ display: "flex", flexWrap: "wrap", gap: 4 }}>
          {DIMENSIONS.map((option) => (
            <button
              key={option.value}
              type="button"
              onClick={() => selectDimension(option.value)}
              aria-pressed={dimension === option.value}
              style={{
                background: dimension === option.value ? chrome.hoverBg : "transparent",
                color: dimension === option.value ? chrome.hoverText : chrome.text,
                border: `1px solid ${chrome.separator}`,
                borderRadius: 4,
                padding: "2px 7px",
                fontSize: "0.72rem",
                cursor: "pointer",
                opacity: dimension === option.value ? 1 : 0.8,
              }}
            >
              {t(option.labelKey)}
            </button>
          ))}
        </span>
        {/* Breadcrumb — also the only reliable way back on a phone, where the group frames that
            drilled in are no longer on screen. */}
        <span style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 2, minWidth: 0 }}>
          {crumbs.map((crumb, crumbIndex) => {
            const current = crumbIndex === crumbs.length - 1
            return (
              <span
                key={`${crumb.index}-${crumb.title}`}
                style={{ display: "flex", alignItems: "center", gap: 2, minWidth: 0 }}
              >
                {crumbIndex > 0 ? (
                  <i className="fa fa-angle-right" style={{ opacity: 0.5, fontSize: "0.7rem" }} />
                ) : null}
                {current ? (
                  <span
                    style={{
                      ...muted,
                      fontSize: "0.74rem",
                      maxWidth: "42vw",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {crumb.title}
                  </span>
                ) : (
                  <button
                    type="button"
                    onClick={() => {
                      setStack((existing) => existing.slice(0, crumb.index))
                      setSelected(null)
                      setHoveredKey(null)
                    }}
                    style={{
                      background: "transparent",
                      border: "none",
                      color: chrome.hoverText,
                      padding: 0,
                      fontSize: "0.74rem",
                      cursor: "pointer",
                      maxWidth: "42vw",
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {crumb.title}
                  </button>
                )}
              </span>
            )
          })}
        </span>
      </div>

      <div ref={frameRef} style={{ width: "100%", height, touchAction: "manipulation" }}>
        <ResponsiveContainer width="100%" height="100%">
          <Treemap
            data={tiles}
            dataKey="value"
            nameKey="name"
            type="flat"
            aspectRatio={narrow ? 1 : 1.5}
            nodeInset={NODE_INSET}
            nodeGap={NODE_GAP}
            // Hundreds of tiles animating in from off-canvas is exactly the kind of thing that
            // stutters on a phone, and drilling should feel like a zoom, not a wait.
            isAnimationActive={false}
            isUpdateAnimationActive={false}
            content={renderTile}
          >
            <Tooltip content={<ChartTooltip formatValue={(value) => formatBytes(Number(value))} />} cursor={false} />
          </Treemap>
        </ResponsiveContainer>
      </div>

      {selected ? (
        <div
          style={{
            display: "flex",
            flexWrap: "wrap",
            alignItems: "center",
            gap: "4px 10px",
            border: `1px solid ${chrome.separator}`,
            borderRadius: 4,
            padding: "6px 8px",
            marginTop: 8,
            fontSize: "0.76rem",
          }}
        >
          <a
            href={routes.reader(selected.id)}
            title={selected.title}
            style={{
              minWidth: 0,
              flex: "1 1 160px",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
              color: colorFor(level.colorBy, selectedColorKey ?? ""),
            }}
          >
            <i className="fa fa-book" style={{ marginRight: 6, opacity: 0.6 }} />
            {selected.title} <FormatBadge extension={selected.extension} />
          </a>
          <span style={{ ...muted, whiteSpace: "nowrap" }}>
            {formatBytes(selected.bytes)} · {t("stats.storageTreemapShare", { percent: selectedShare.toFixed(1) })}
            {selected.pages > 0 ? ` · ${t("stats.pages")} ${selected.pages}` : ""}
          </span>
          <button
            type="button"
            onClick={() => setSelected(null)}
            title={t("common.close")}
            aria-label={t("common.close")}
            style={{ background: "transparent", border: "none", color: "inherit", cursor: "pointer", opacity: 0.7 }}
          >
            <i className="fa fa-times" />
          </button>
        </div>
      ) : null}

      <p style={{ ...muted, fontSize: "0.72rem", marginTop: 8 }}>
        {t("stats.storageTreemapTotal", { n: currentCount, size: formatBytes(currentBytes) })}
        {" · "}
        {level.colorBy === "artist"
          ? t("stats.storageTreemapColorsByArtist")
          : t("stats.storageTreemapColorsByCategory")}
      </p>
    </StatsSection>
  )
}
