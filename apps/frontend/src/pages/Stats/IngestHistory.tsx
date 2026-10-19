import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Bar, BarChart, CartesianGrid, Cell, Tooltip, XAxis, YAxis } from "recharts"

import type { StatsBucketSelection } from "@/api/hooks"
import type { StatsOverview } from "@/api/types"
import { FormatBadge } from "@/components/Display"
import { routes } from "@/lib/routes"

import { BucketArchivesPanel } from "./BucketArchivesPanel"
import { useChartPalette } from "./chartPalette"
import { BarRow, ChartTooltip, MIN_BAR_PIXELS, StatsSection, StatsTable, useStatsPanel } from "./Charts"
import {
  formatBytes,
  formatPercent,
  formatTimestamp,
  ingestSourceLabelKey,
  timelineLabel,
  timelineUnitKey,
} from "./format"
import { extendWindow } from "./timeline"
import { InteractiveChartFrame, TimelineControls } from "./TimelineControls"
import { useTimelineView } from "./useTimelineView"

/** Where the library's archives came from, over time and by path.
 *
 *  The bars come from each archive's own `date_added` tag (exact and complete) over the reader's
 *  own chosen range and granularity — the range presets, ctrl/⌘+wheel zoom and drag-to-pan are all
 *  local re-bucketings of one daily series (`useTimelineView`), so nothing here costs a request
 *  after the first load. The source breakdown comes from the operator activity log's unified
 *  ingestion records instead, the only place the ingestion paths are distinguished, so it is
 *  honestly labelled with how many records it actually covers. */
export function IngestHistory({ ingest }: { ingest: StatsOverview["ingest"] }) {
  const { t } = useTranslation()
  // Which bar is drilled into, if any — the panel under the chart lists that bucket's archives.
  const [selected, setSelected] = useState<StatsBucketSelection | null>(null)
  // The bucket a Shift+click extends from — the last plain click, not the last selection (so
  // repeatedly Shift+clicking extends from the same place instead of walking along).
  const anchorRef = useRef<{ from: string; to: string } | null>(null)
  const selectBucket = (index: number, shiftKey: boolean) => {
    const bucket = view.buckets[index]
    if (!bucket) return
    const anchor = anchorRef.current
    if (shiftKey && anchor) {
      const span = extendWindow(anchor, bucket)
      setSelected({ metric: "ingest", from: span.from, to: span.to })
      return
    }
    anchorRef.current = { from: bucket.from, to: bucket.to }
    setSelected((current) =>
      current?.from === bucket.from && current?.metric === "ingest"
        ? null
        : { metric: "ingest", from: bucket.from, to: bucket.to },
    )
  }
  const palette = useChartPalette()
  const { muted } = useStatsPanel()
  const view = useTimelineView("ingest")
  const timelineTotal = view.buckets.reduce((sum, bucket) => sum + bucket.archives, 0)
  const sourceTotal = ingest.sources.reduce((sum, source) => sum + source.archives, 0)
  const maxSource = ingest.sources.reduce((max, source) => Math.max(max, source.archives), 0)

  const data = view.buckets.map((bucket) => ({
    ...bucket,
    label: timelineLabel(bucket.key, view.resolvedUnit),
  }))

  return (
    <StatsSection title={t("stats.ingestHistory")} icon="fa-upload" hint={t("stats.ingestHistoryHint")}>
      <TimelineControls view={view} />
      <InteractiveChartFrame height={220} view={view}>
        <BarChart data={data} margin={{ top: 8, right: 8, bottom: 4, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" opacity={0.2} vertical={false} />
          <XAxis dataKey="label" tick={{ fontSize: 11 }} interval="preserveStartEnd" />
          <YAxis allowDecimals={false} tick={{ fontSize: 11 }} width={36} />
          <Tooltip
            cursor={{ fill: "rgba(127,127,127,0.15)" }}
            content={<ChartTooltip formatValue={(value) => t("stats.archiveCountValue", { count: Number(value) })} />}
          />
          <Bar
            dataKey="archives"
            name={t("stats.ingested")}
            radius={[3, 3, 0, 0]}
            cursor="pointer"
            // Non-zero bars are drawn at least this tall so a one-archive bucket stays clickable:
            // at 1/79th of the plot height its true size is under a pixel, which is not a target a
            // cursor can land on. Zero buckets are still not drawn at all.
            minPointSize={MIN_BAR_PIXELS}
            onClick={(_entry, index, event) => selectBucket(index, event.shiftKey)}
          >
            {view.buckets.map((bucket) => (
              <Cell
                key={bucket.key}
                fill={
                  selected?.metric === "ingest" && selected.from === bucket.from
                    ? palette[5]
                    : palette[1]
                }
              />
            ))}
          </Bar>
        </BarChart>
      </InteractiveChartFrame>
      <BucketArchivesPanel
        selection={selected}
        title={t("stats.bucketArchivesIngest")}
        subtitle={
          selected
            ? t("stats.bucketRange", { from: selected.from, to: selected.to })
            : undefined
        }
        onClose={() => setSelected(null)}
      />
      <p style={{ ...muted, fontSize: "0.72rem", margin: "4px 0 14px" }}>
        {t("stats.ingestTimelineTotal", {
          count: timelineTotal,
          unit: t(timelineUnitKey(view.resolvedUnit)),
        })}
      </p>

      <div style={{ display: "flex", flexWrap: "wrap", gap: 24 }}>
        <div style={{ flex: "1 1 320px", minWidth: 280 }}>
          <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.ingestSources")}</h4>
          {sourceTotal === 0 ? (
            <p style={{ ...muted, fontSize: "0.78rem", margin: 0 }}>{t("stats.noIngestRecords")}</p>
          ) : (
            <>
              {ingest.sources.map((source, index) => (
                <BarRow
                  key={source.source}
                  label={t(ingestSourceLabelKey(source.source))}
                  value={source.archives}
                  max={maxSource}
                  valueLabel={`${source.archives.toLocaleString()} · ${formatPercent(source.archives, sourceTotal)}`}
                  color={palette[index % palette.length]}
                />
              ))}
              <p style={{ ...muted, fontSize: "0.72rem", margin: "8px 0 0" }}>
                {ingest.sources_sampled_only
                  ? t("stats.ingestSourcesSampledOnly", { count: ingest.sources_sampled })
                  : t("stats.ingestSourcesSampled", { count: ingest.sources_sampled })}
              </p>
            </>
          )}
        </div>

        <div style={{ flex: "1 1 420px", minWidth: 300 }}>
          <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.recentIngests")}</h4>
          {ingest.recent.length === 0 ? (
            <p style={{ ...muted, fontSize: "0.78rem", margin: 0 }}>{t("stats.noIngestRecords")}</p>
          ) : (
            <StatsTable
              head={[t("stats.title"), t("stats.added"), t("stats.size"), t("stats.pages")]}
              rows={ingest.recent.map((archive) => [
                <a key={archive.id} href={routes.reader(archive.id)} title={archive.title}>
                  {archive.title} <FormatBadge extension={archive.extension} />
                </a>,
                formatTimestamp(archive.timestamp),
                formatBytes(archive.bytes),
                archive.pages.toLocaleString(),
              ])}
            />
          )}
        </div>
      </div>
    </StatsSection>
  )
}
