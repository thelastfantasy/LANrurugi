import { type ReactElement, type ReactNode,useEffect, useRef } from "react"
import { useTranslation } from "react-i18next"
import { ResponsiveContainer } from "recharts"

import { useStatsPanel } from "./Charts"
import { daysBetween, panWindow, type TimelineUnit } from "./timeline"
import type { TimelineView } from "./useTimelineView"

/** Range presets, in days; `null` = the whole available range. Kept as a short list rather than a
 *  date picker: the interesting question on this page is "has this grown lately", and the wheel /
 *  drag gestures cover everything between the presets. */
const PRESETS: { days: number | null; labelKey: string }[] = [
  { days: 7, labelKey: "stats.range.7d" },
  { days: 30, labelKey: "stats.range.30d" },
  { days: 90, labelKey: "stats.range.90d" },
  { days: 365, labelKey: "stats.range.1y" },
  { days: null, labelKey: "stats.range.all" },
]

const UNITS: TimelineUnit[] = ["auto", "day", "week", "month"]

function Chip({
  active,
  onClick,
  children,
  title,
}: {
  active: boolean
  onClick: () => void
  children: ReactNode
  title?: string
}) {
  const { palette } = useStatsPanel()
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      aria-pressed={active}
      style={{
        background: active ? palette.hoverBg : "transparent",
        color: active ? palette.hoverText : palette.text,
        border: `1px solid ${palette.separator}`,
        borderRadius: 4,
        padding: "2px 7px",
        fontSize: "0.72rem",
        cursor: "pointer",
        opacity: active ? 1 : 0.8,
      }}
    >
      {children}
    </button>
  )
}

/** The range/granularity row above a timeline chart, plus the hint that tells a reader the chart is
 *  interactive at all (a chart that silently zooms is a chart nobody zooms). */
export function TimelineControls({ view }: { view: TimelineView }) {
  const { t } = useTranslation()
  const { muted } = useStatsPanel()
  // A preset reads as active when the window currently spans exactly what it would select. Several
  // presets can be *capped by the data* into the same width (a 90-day request on a 84-day library
  // is the whole range, exactly like "1 year" and "all") — so only the narrowest match highlights,
  // and a hand-zoomed window highlights none rather than lying about which preset it is.
  const windowWidth = view.window ? daysBetween(view.window.from, view.window.to) : -1
  const presetWidths = PRESETS.map((preset) =>
    preset.days === null ? view.totalDays : Math.min(preset.days, view.totalDays),
  )
  // Showing the whole range always reads as "all", even when a narrower preset happens to be
  // capped to that same width — otherwise clicking "all" on a two-month-old library would light up
  // "90 days" instead, which is the opposite of what was clicked.
  const activePresetIndex =
    windowWidth === view.totalDays ? PRESETS.length - 1 : presetWidths.indexOf(windowWidth)
  return (
    <div
      style={{
        display: "flex",
        flexWrap: "wrap",
        alignItems: "center",
        gap: 10,
        margin: "0 0 6px",
      }}
    >
      <span style={{ display: "flex", gap: 4 }}>
        {PRESETS.map((preset, index) => (
          <Chip
            key={preset.labelKey}
            active={index === activePresetIndex}
            onClick={() => view.setPreset(preset.days)}
          >
            {t(preset.labelKey)}
          </Chip>
        ))}
      </span>
      <span style={{ display: "flex", gap: 4, alignItems: "center" }}>
        <span style={{ ...muted, fontSize: "0.72rem" }}>{t("stats.granularity")}</span>
        {UNITS.map((unit) => (
          <Chip
            key={unit}
            active={view.unit === unit}
            onClick={() => view.setUnit(unit)}
            title={unit === "auto" ? t("stats.granularityAutoHint") : undefined}
          >
            {t(`stats.unit.${unit}`)}
          </Chip>
        ))}
      </span>
      <Chip active={false} onClick={view.reset} title={t("stats.resetRangeHint")}>
        <i className="fa fa-undo" style={{ marginRight: 4 }} />
        {t("stats.resetRange")}
      </Chip>
      <span style={{ ...muted, fontSize: "0.72rem" }}>{t("stats.zoomHint")}</span>
      {view.truncated ? (
        <span style={{ ...muted, fontSize: "0.72rem" }}>{t("stats.rangeTruncated")}</span>
      ) : null}
    </div>
  )
}

/** A chart frame that turns ctrl/⌘+wheel into zoom and a drag into pan.
 *
 *  Deliberately *not* React's `onWheel`: React attaches wheel listeners at the root as passive
 *  ones, where `preventDefault()` is ignored — so the browser's own ctrl+wheel page zoom would fire
 *  alongside this one. A native listener with `passive: false` is the only way to own the gesture.
 *  Plain wheel (no modifier) is left alone entirely, so the page keeps scrolling normally. */
export function InteractiveChartFrame({
  height,
  view,
  children,
}: {
  height: number
  view: TimelineView
  children: ReactElement
}) {
  const containerRef = useRef<HTMLDivElement | null>(null)
  const dragRef = useRef<{ startX: number; window: { from: string; to: string } } | null>(null)
  // The wheel handler is attached once; this ref keeps it reading the current view instead of
  // capturing the first render's callbacks.
  const viewRef = useRef(view)
  viewRef.current = view

  useEffect(() => {
    const element = containerRef.current
    if (!element) return
    const onWheel = (event: WheelEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return
      event.preventDefault()
      const rect = element.getBoundingClientRect()
      const ratio = (event.clientX - rect.left) / Math.max(1, rect.width)
      viewRef.current.zoom(event.deltaY < 0 ? 1.25 : 1 / 1.25, ratio)
    }
    element.addEventListener("wheel", onWheel, { passive: false })
    return () => element.removeEventListener("wheel", onWheel)
  }, [])

  // Capture is taken only once a drag actually starts, never on pointerdown: a plain click on a
  // bar is a gesture this frame must not interfere with, and pointer capture retargets the
  // compatibility mouse events a `click` is derived from.
  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    if (!view.window) return
    dragRef.current = { startX: event.clientX, window: view.window }
  }

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag) return
    if (event.currentTarget.hasPointerCapture(event.pointerId) === false) {
      // Below the threshold this is still a click in progress, not a pan.
      if (Math.abs(event.clientX - drag.startX) < 3) return
      event.currentTarget.setPointerCapture(event.pointerId)
    }
    const rect = event.currentTarget.getBoundingClientRect()
    const span = Math.max(1, daysBetween(drag.window.from, drag.window.to))
    const daysPerPixel = span / Math.max(1, rect.width)
    // Dragging left moves the window forward in time, the same direction as the data.
    const deltaDays = Math.round((drag.startX - event.clientX) * daysPerPixel)
    viewRef.current.setRange(panWindow(drag.window, deltaDays, viewRef.current.bounds))
  }

  const endDrag = () => {
    dragRef.current = null
  }

  return (
    <div
      ref={containerRef}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={endDrag}
      onPointerCancel={endDrag}
      style={{ width: "100%", height, cursor: view.window ? "grab" : "default", touchAction: "pan-y" }}
    >
      {/* Same sized box `ChartFrame` provides — a recharts chart with neither a
          `ResponsiveContainer` nor explicit dimensions renders nothing at all. Recharts keeps its
          own pointer handling on top of it (tooltips are the only way to read one bar's value):
          pan gestures ride along on the *bubbled* pointer events this frame sees, rather than
          being carved out with `pointer-events: none`. */}
      <ResponsiveContainer width="100%" height="100%">
        {children}
      </ResponsiveContainer>
    </div>
  )
}
