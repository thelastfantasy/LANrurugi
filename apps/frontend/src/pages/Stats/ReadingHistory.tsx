import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Bar, CartesianGrid, Cell, ComposedChart, Line, Tooltip, XAxis, YAxis } from "recharts"

import type { StatsBucketSelection } from "@/api/hooks"
import type { StatsOverview } from "@/api/types"
import { routes } from "@/lib/routes"

import { BucketArchivesPanel } from "./BucketArchivesPanel"
import { useChartPalette } from "./chartPalette"
import { BarRow, ChartTooltip, LegendSwatch, MIN_BAR_PIXELS, StatsSection, StatsTable, useStatsPanel } from "./Charts"
import {
  formatPercent,
  formatProgress,
  formatTimestamp,
  progressBucketLabelKey,
  progressBucketSearchQuery,
  searchURL,
  timelineLabel,
} from "./format"
import { extendWindow } from "./timeline"
import { InteractiveChartFrame, TimelineControls } from "./TimelineControls"
import { useTimelineView } from "./useTimelineView"

/** Reading activity over the reader's own chosen window (presets, ctrl/⌘+wheel zoom, drag to pan),
 *  plus how far through the library is. The series is fetched once as daily points and re-bucketed
 *  locally, so changing the granularity or the range costs no request.
 *
 *  The chart carries two series that genuinely mean different things and are labelled as such:
 *  `pages` (bars) is exact but only exists from the day the per-day counter shipped; `last_read`
 *  (line) counts archives whose most recent read fell on that day, which is the only signal that
 *  reaches back before it. `StatsOverview.note` is what the page uses to explain a chart of
 *  zeroes when the server has never recorded a read at all. */
export function ReadingHistory({ reading }: { reading: StatsOverview["reading"] }) {
  const { t } = useTranslation()
  const palette = useChartPalette()
  const { muted } = useStatsPanel()
  const view = useTimelineView("reading")
  // Both series address the same bucket, and only one of them *is* a set of archives ("last read
  // on that day" — the line): a day's page count is page turns, with no list of archives behind
  // it. So clicking either the bar or the line opens that bucket's archives-last-read list, and
  // the panel says so.
  const [selected, setSelected] = useState<StatsBucketSelection | null>(null)
  // The bucket a Shift+click extends from (see `IngestHistory`'s own note on why this is the last
  // *plain* click rather than the last selection).
  const anchorRef = useRef<{ from: string; to: string } | null>(null)
  /** The line's points are click targets too — a bucket should be openable by clicking the series a
   *  reader is actually looking at, not only the bar in the same column. Each point gets a generous
   *  transparent circle (an 8px radius is comfortable to hit) plus, while the series is sparse
   *  enough to read as dots rather than as a dotted line, a small visible one; the hover dot
   *  recharts already draws is what shows which point is about to be clicked. */
  const renderLineDot = (props: { cx?: number; cy?: number; index?: number }) => {
    const { cx = 0, cy = 0, index = 0 } = props
    const bucket = view.buckets[index]
    const isSelected = Boolean(bucket && selected?.metric === "read" && selected.from === bucket.from)
    return (
      <g
        key={index}
        onClick={(event) => selectBucket(index, event.shiftKey)}
        style={{ cursor: "pointer" }}
        role="button"
        aria-label={bucket ? `${bucket.from} – ${bucket.to}` : undefined}
      >
        <circle cx={cx} cy={cy} r={8} fill="transparent" />
        {(view.buckets.length <= 31 || isSelected) && (
          <circle cx={cx} cy={cy} r={3} fill={palette[3]} stroke="none" />
        )}
      </g>
    )
  }

  const selectBucket = (index: number, shiftKey: boolean) => {
    const bucket = view.buckets[index]
    if (!bucket) return
    const anchor = anchorRef.current
    if (shiftKey && anchor) {
      const span = extendWindow(anchor, bucket)
      setSelected({ metric: "read", from: span.from, to: span.to })
      return
    }
    anchorRef.current = { from: bucket.from, to: bucket.to }
    setSelected((current) =>
      current?.from === bucket.from && current.metric === "read"
        ? null
        : { metric: "read", from: bucket.from, to: bucket.to },
    )
  }

  const pagesTotal = view.buckets.reduce((sum, bucket) => sum + bucket.pages, 0)
  const lastReadTotal = view.buckets.reduce((sum, bucket) => sum + bucket.last_read, 0)
  const progressTotal = reading.progress.reduce((sum, bucket) => sum + bucket.archives, 0)
  const maxProgress = reading.progress.reduce((max, bucket) => Math.max(max, bucket.archives), 0)

  const data = view.buckets.map((bucket) => ({ ...bucket, label: timelineLabel(bucket.key, view.resolvedUnit) }))

  return (
    <StatsSection title={t("stats.readingHistory")} icon="fa-book-reader" hint={t("stats.readingHistoryHint")}>
      <TimelineControls view={view} />
      <InteractiveChartFrame height={240} view={view}>
        <ComposedChart data={data} margin={{ top: 8, right: 8, bottom: 4, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" opacity={0.2} vertical={false} />
          <XAxis dataKey="label" tick={{ fontSize: 11 }} interval="preserveStartEnd" />
          <YAxis yAxisId="pages" allowDecimals={false} tick={{ fontSize: 11 }} width={36} />
          <YAxis
            yAxisId="archives"
            orientation="right"
            allowDecimals={false}
            tick={{ fontSize: 11 }}
            width={36}
          />
          <Tooltip
            cursor={{ fill: "rgba(127,127,127,0.15)" }}
            content={<ChartTooltip formatValue={(value) => Number(value).toLocaleString()} />}
          />
          <Bar
            yAxisId="pages"
            dataKey="pages"
            name={t("stats.pagesReadPerDay")}
            radius={[3, 3, 0, 0]}
            cursor="pointer"
            minPointSize={MIN_BAR_PIXELS}
            onClick={(_entry, index, event) => selectBucket(index, event.shiftKey)}
          >
            {view.buckets.map((bucket) => (
              <Cell
                key={bucket.key}
                fill={selected?.metric === "read" && selected.from === bucket.from ? palette[6] : palette[0]}
              />
            ))}
          </Bar>
          <Line
            yAxisId="archives"
            type="monotone"
            dataKey="last_read"
            name={t("stats.archivesLastReadPerDay")}
            stroke={palette[3]}
            strokeWidth={2}
            dot={renderLineDot}
          />
        </ComposedChart>
      </InteractiveChartFrame>

      <div style={{ display: "flex", gap: 18, flexWrap: "wrap", fontSize: "0.75rem", margin: "6px 0 0" }}>
        <span>
          <LegendSwatch color={palette[0]} />
          {t("stats.pagesReadInWindow", { count: pagesTotal, days: view.windowDays })}
        </span>
        <span>
          <LegendSwatch color={palette[3]} />
          {t("stats.archivesLastReadInWindow", { count: lastReadTotal, days: view.windowDays })}
        </span>
      </div>
      <p style={{ ...muted, fontSize: "0.72rem", margin: "6px 0 14px" }}>
        {view.pagesSince
          ? t("stats.pagesCounterSince", { date: view.pagesSince })
          : t("stats.pagesCounterNoData")}
      </p>

      <BucketArchivesPanel
        selection={selected}
        title={t("stats.bucketArchivesRead")}
        subtitle={selected ? t("stats.bucketRange", { from: selected.from, to: selected.to }) : undefined}
        onClose={() => setSelected(null)}
      />

      <div style={{ display: "flex", flexWrap: "wrap", gap: 24 }}>
        <div style={{ flex: "1 1 320px", minWidth: 280 }}>
          <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.readingProgress")}</h4>
          {reading.progress.map((bucket, index) => (
            <BarRow
              key={bucket.key}
              label={t(progressBucketLabelKey(bucket.key))}
              value={bucket.archives}
              max={maxProgress}
              valueLabel={`${bucket.archives.toLocaleString()} · ${formatPercent(bucket.archives, progressTotal)}`}
              color={palette[index % palette.length]}
              // The bucket's own members, as a search — the point of the row is "which of these
              // are they", and the answer should be one click away.
              href={searchURL(progressBucketSearchQuery(bucket.key))}
              linkTitle={t("stats.searchThisQuery", { query: progressBucketSearchQuery(bucket.key) })}
            />
          ))}
        </div>

        <div style={{ flex: "1 1 320px", minWidth: 280 }}>
          <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.ratingDistribution")}</h4>
          {reading.ratings.map((rating, index) => (
            <BarRow
              key={rating.stars}
              label={
                rating.stars === 0
                  ? t("stats.unrated")
                  : t("stats.starsValue", { count: rating.stars })
              }
              value={rating.archives}
              max={reading.ratings.reduce((max, entry) => Math.max(max, entry.archives), 0)}
              valueLabel={rating.archives.toLocaleString()}
              color={palette[(index + 2) % palette.length]}
            />
          ))}
        </div>
      </div>

      <div style={{ display: "flex", flexWrap: "wrap", gap: 24, marginTop: 16 }}>
        <div style={{ flex: "1 1 420px", minWidth: 300 }}>
          <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.recentlyRead")}</h4>
          {reading.recent.length === 0 ? (
            <p style={{ ...muted, fontSize: "0.78rem", margin: 0 }}>{t("stats.noReadingRecords")}</p>
          ) : (
            <StatsTable
              head={[t("stats.title"), t("stats.lastRead"), t("stats.progress")]}
              rows={reading.recent.map((archive) => [
                <a key={archive.id} href={routes.reader(archive.id)} title={archive.title}>
                  {archive.title}
                </a>,
                formatTimestamp(archive.last_read),
                formatProgress(archive.page, archive.pages),
              ])}
            />
          )}
        </div>

        <div style={{ flex: "1 1 420px", minWidth: 300 }}>
          <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.mostReread")}</h4>
          {reading.most_reread.length === 0 ? (
            <p style={{ ...muted, fontSize: "0.78rem", margin: 0 }}>{t("stats.noRereadRecords")}</p>
          ) : (
            <StatsTable
              head={[t("stats.title"), t("stats.readEvents"), t("stats.progress")]}
              rows={reading.most_reread.map((archive) => [
                <a key={archive.id} href={routes.reader(archive.id)} title={archive.title}>
                  {archive.title}
                </a>,
                archive.reads.toLocaleString(),
                formatProgress(archive.page, archive.pages),
              ])}
            />
          )}
        </div>
      </div>
    </StatsSection>
  )
}
