import { useCallback, useMemo, useState } from "react"

import { useStatsTimeline } from "@/api/hooks"
import type { StatsTimelineMetric } from "@/api/types"

import {
  bucketSeries,
  daysBetween,
  panWindow,
  presetWindow,
  type ResolvedUnit,
  resolveUnit,
  type TimelineBucket,
  type TimelineUnit,
  type Window,
  zoomWindow,
} from "./timeline"

/** Everything one interactive chart needs: the loaded daily series, the visible window, the
 *  granularity (locked or automatic), the buckets to draw, and the gestures that change them.
 *
 *  All of it is local state over one fetch per metric — a zoom or a granularity change never
 *  reaches the network, because the server already returned the finest series (daily) over the
 *  widest range it will serve. */
export function useTimelineView(metric: StatsTimelineMetric) {
  const query = useStatsTimeline(metric)
  // `null` means "follow the data": the effective window is derived below rather than copied into
  // state by an effect, so nothing has to be kept in sync (and no cascading render is triggered).
  const [windowOverride, setWindowOverride] = useState<Window | null>(null)
  const [unit, setUnit] = useState<TimelineUnit>("auto")

  const bounds = useMemo<Window>(
    () => ({
      from: query.data?.available_from ?? "",
      to: query.data?.available_to ?? "",
    }),
    [query.data],
  )

  // Until a gesture happens, the whole range is shown — which is also what makes every later
  // gesture local.
  const window = useMemo<Window | null>(
    () => windowOverride ?? (bounds.from ? bounds : null),
    [windowOverride, bounds],
  )

  const resolvedUnit: ResolvedUnit = window ? resolveUnit(unit, window.from, window.to) : "month"
  const buckets: TimelineBucket[] = useMemo(
    () => (window && query.data ? bucketSeries(query.data.points, resolvedUnit, window.from, window.to) : []),
    [query.data, resolvedUnit, window],
  )

  // Applied to whatever is on screen *right now* — a functional update, so a burst of wheel events
  // within one render still composes instead of all reading the same starting window.
  const updateWindow = useCallback(
    (transform: (current: Window) => Window) => {
      setWindowOverride((current) => transform(current ?? bounds))
    },
    [bounds],
  )

  const zoom = useCallback(
    (factor: number, anchorRatio: number) => {
      updateWindow((current) => zoomWindow(current, factor, anchorRatio, bounds))
    },
    [bounds, updateWindow],
  )

  const pan = useCallback(
    (deltaDays: number) => {
      updateWindow((current) => panWindow(current, deltaDays, bounds))
    },
    [bounds, updateWindow],
  )

  const setPreset = useCallback(
    (days: number | null) => {
      if (bounds.from) setWindowOverride(presetWindow(days, bounds))
    },
    [bounds],
  )

  const reset = useCallback(() => {
    setWindowOverride(null)
    setUnit("auto")
  }, [])

  const setRange = useCallback((next: Window) => {
    setWindowOverride({ from: next.from, to: next.to })
  }, [])

  const totalDays = bounds.from ? daysBetween(bounds.from, bounds.to) : 0
  const windowDays = window ? daysBetween(window.from, window.to) : 0
  // The server caps how wide a span it will materialize, so the page can be showing less history
  // than the library actually has — worth saying rather than silently truncating.
  const truncated = Boolean(query.data && query.data.from > query.data.available_from)

  return {
    query,
    bounds,
    window,
    unit,
    resolvedUnit,
    buckets,
    totalDays,
    windowDays,
    truncated,
    pagesSince: query.data?.pages_since,
    zoom,
    pan,
    setPreset,
    setUnit,
    setRange,
    reset,
  }
}

export type TimelineView = ReturnType<typeof useTimelineView>
