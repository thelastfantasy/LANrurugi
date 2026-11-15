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
/** A band this bundle has no label for (an older UI against a newer backend, or the reverse)
 *  still gets a readable label from the bounds the response carries — leaking
 *  `stats.sizeBucket.gte_500mb` into the axis and table is worse than an English-shaped one. */
function fallbackBandLabel(bucket: StatsOverview["size_buckets"][number]): string {
  if (bucket.max_bytes === undefined) return `≥ ${formatBytes(bucket.min_bytes)}`
  return bucket.min_bytes === 0
    ? `< ${formatBytes(bucket.max_bytes)}`
    : `${formatBytes(bucket.min_bytes)} - ${formatBytes(bucket.max_bytes)}`
}

export function SizeDistribution({ buckets }: { buckets: StatsOverview["size_buckets"] }) {
  const { t, i18n } = useTranslation()
  const palette = useChartPalette()
  const { muted } = useStatsPanel()
  const [selected, setSelected] = useState<StatsBucketSelection | null>(null)
  // Twelve band labels only fit side by side on a wide chart: rotated, the longest one
  // ("500 MB - 1 GB") is ~70px, so twelve need ~840px of axis plus the card's own padding. Below
  // ~1100px adjacent labels start touching — measured on a 390px-wide phone, where every adjacent
  // pair overlapped — so narrower charts label every other band (every third on a phone) and lean
  // on the table below, which still spells out every band exactly.
  const narrowViewport = useIsNarrowViewport("(max-width: 768px)")
  const denseAxis = useIsNarrowViewport("(max-width: 1100px)")
  const total = buckets.reduce((sum, bucket) => sum + bucket.archives, 0)
  if (total === 0) return null

  const data = buckets.map((bucket) => {
    const key = sizeBucketLabelKey(bucket.key)
    return {
      ...bucket,
      label: i18n.exists(key) ? t(key) : fallbackBandLabel(bucket),
    }
  })
  // Thinned labels always keep the last band: an unlabeled open-ended "≥ 2 GB" bar would read as
  // "everything else", which is the one band the chart exists to show.
  const labels = data.map((bucket) => bucket.label)
  const tickStep = !denseAxis ? 1 : narrowViewport ? 3 : 2
  const ticks = labels.filter((_, index) => index % tickStep === 0)
  if (ticks[ticks.length - 1] !== labels[labels.length - 1]) {
    ticks.push(labels[labels.length - 1])
  }

  return (
    <StatsSection title={t("stats.sizeDistribution")} icon="fa-database" hint={t("stats.sizeDistributionHint")}>
      <ChartFrame height={240}>
        <BarChart data={data} margin={{ top: 8, right: 8, bottom: 4, left: 0 }}>
          <CartesianGrid strokeDasharray="3 3" opacity={0.2} vertical={false} />
          {/* Explicit ticks rather than recharts' own "fit what you can": its estimate assumes the
              unrotated label width, which made it keep too many. See `tickStep` above for the
              per-width step and why the last band is always kept. */}
          <XAxis
            dataKey="label"
            tick={{ fontSize: narrowViewport ? 10 : 11 }}
            ticks={ticks}
            interval={0}
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
