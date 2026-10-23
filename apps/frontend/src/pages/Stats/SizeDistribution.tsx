import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Bar, BarChart, CartesianGrid, Cell, Tooltip, XAxis, YAxis } from "recharts"

import type { StatsBucketSelection } from "@/api/hooks"
import type { StatsOverview } from "@/api/types"
import { useIsNarrowViewport } from "@/hooks/useIsNarrowViewport"

import { BucketArchivesPanel } from "./BucketArchivesPanel"
import { useChartPalette } from "./chartPalette"
import { ChartFrame, ChartTooltip, MIN_BAR_PIXELS, StatsSection, StatsTable, useStatsPanel } from "./Charts"
import { formatBytes, formatPercent, sizeBucketLabelKey } from "./format"

/** How many archives fall into each size band, as a bar chart plus the exact numbers — the chart
 *  answers "what does my library look like" at a glance, the table answers "how many exactly".
 *  Three representations of the same bucket list on purpose: the backend only sends counts and
 *  byte sums, so every label here (including the band boundaries) is localized client-side. */
export function SizeDistribution({ buckets }: { buckets: StatsOverview["size_buckets"] }) {
  const { t } = useTranslation()
  const palette = useChartPalette()
  const { muted } = useStatsPanel()
  const [selected, setSelected] = useState<StatsBucketSelection | null>(null)
  // All nine band labels only fit side by side on a wide chart: rotated, the longest one
  // ("500 MB or more") is ~65px, so nine of them need ~630px of axis. Below that they ran into
  // each other — measured on a 390px-wide phone, all eight adjacent pairs overlapped. Narrow
  // viewports therefore let recharts label whichever ticks fit, exactly like the two time-series
  // charts above already do; the table below still spells out every band.
  const narrowViewport = useIsNarrowViewport("(max-width: 768px)")
  const total = buckets.reduce((sum, bucket) => sum + bucket.archives, 0)
  if (total === 0) return null

  const data = buckets.map((bucket) => ({
    ...bucket,
    label: t(sizeBucketLabelKey(bucket.key)),
  }))

  return (
    <StatsSection title={t("stats.sizeDistribution")} icon="fa-database" hint={t("stats.sizeDistributionHint")}>
      <ChartFrame height={240}>
        <BarChart data={data} margin={{ top: 8, right: 8, bottom: 4, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" opacity={0.2} vertical={false} />
          {/* Nine band labels only fit side by side on a wide chart: rotated, the longest one
              ("500 MB or more") is ~65px, so nine need ~630px of axis. Below that they ran into
              each other — measured on a 390px-wide phone, all eight adjacent pairs overlapped —
              and letting recharts choose ("preserveStartEnd") did not help, because that estimate
              assumes the unrotated text width. A narrow viewport therefore labels every other band
              at a slightly smaller size (five legible labels) and leans on the table below, which
              still spells out every band exactly. */}
          <XAxis
            dataKey="label"
            tick={{ fontSize: narrowViewport ? 10 : 11 }}
            interval={narrowViewport ? 1 : 0}
            angle={-25}
            textAnchor="end"
            height={50}
          />
          <YAxis allowDecimals={false} tick={{ fontSize: 11 }} width={36} />
          <Tooltip
            cursor={{ fill: "rgba(127,127,127,0.15)" }}
            content={<ChartTooltip formatValue={(value) => t("stats.archiveCountValue", { count: Number(value) })} />}
          />
          <Bar
            dataKey="archives"
            name={t("stats.archives")}
            radius={[3, 3, 0, 0]}
            cursor="pointer"
            // The small bands hold one or two archives: without a floor their bars are a sliver a
            // cursor cannot hit, which is exactly the "clicking the bar does nothing" case.
            minPointSize={MIN_BAR_PIXELS}
            onClick={(_entry, index) => {
              const bucket = buckets[index]
              if (!bucket) return
              setSelected((current) =>
                current?.key === bucket.key && current.metric === "size"
                  ? null
                  : { metric: "size", key: bucket.key },
              )
            }}
          >
            {buckets.map((bucket) => (
              <Cell
                key={bucket.key}
                fill={selected?.metric === "size" && selected.key === bucket.key ? palette[4] : palette[0]}
              />
            ))}
          </Bar>
        </BarChart>
      </ChartFrame>

      <BucketArchivesPanel
        selection={selected}
        title={t("stats.bucketArchivesSize")}
        subtitle={selected?.key ? t(sizeBucketLabelKey(selected.key)) : undefined}
        onClose={() => setSelected(null)}
      />

      <StatsTable
        head={[t("stats.sizeBand"), t("stats.archives"), t("stats.share"), t("stats.storageUsed")]}
        rows={buckets.map((bucket) => [
          t(sizeBucketLabelKey(bucket.key)),
          bucket.archives.toLocaleString(),
          formatPercent(bucket.archives, total),
          formatBytes(bucket.bytes),
        ])}
      />
      <p style={{ ...muted, fontSize: "0.72rem", marginTop: 8 }}>
        {t("stats.sizeDistributionTotal", { count: total })}
      </p>
    </StatsSection>
  )
}
