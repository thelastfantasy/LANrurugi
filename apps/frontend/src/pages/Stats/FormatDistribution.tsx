import { useTranslation } from "react-i18next"
import { Cell, Pie, PieChart, Tooltip } from "recharts"

import type { StatsOverview } from "@/api/types"

import { useChartPalette } from "./chartPalette"
import { ChartFrame, ChartTooltip, LegendSwatch, StatsSection, StatsTable, useStatsPanel } from "./Charts"
import { formatBytes, formatPercent, seriesColor } from "./format"

/** A donut of container formats (by archive count) plus the mime type each one actually is — the
 *  Zipline-style "image/webp 534" view, translated to what an archive library stores: the
 *  container, since the page formats *inside* every zip are not known without opening all of them
 *  (a scan's worth of libarchive reads, deliberately not paid here). */
export function FormatDistribution({ formats }: { formats: StatsOverview["formats"] }) {
  const { t } = useTranslation()
  const palette = useChartPalette()
  const { muted } = useStatsPanel()
  const total = formats.reduce((sum, format) => sum + format.archives, 0)
  if (total === 0) return null

  // The chart is capped to the eight largest groups so the ramp stays distinguishable; the table
  // below always lists every group, so nothing is hidden — only the pie's resolution is capped.
  const charted = formats.slice(0, palette.length)
  const paletteFor = (index: number) => seriesColor(palette, index)

  return (
    <StatsSection title={t("stats.formatDistribution")} icon="fa-file-archive-o" hint={t("stats.formatDistributionHint")}>
      <div style={{ display: "flex", flexWrap: "wrap", gap: 16, alignItems: "center" }}>
        <div style={{ flex: "1 1 260px", minWidth: 240 }}>
          <ChartFrame height={240}>
            <PieChart>
              <Pie
                data={charted}
                dataKey="archives"
                nameKey="mime"
                innerRadius="45%"
                outerRadius="78%"
                paddingAngle={1}
                stroke="none"
              >
                {charted.map((format, index) => (
                  <Cell key={format.extension} fill={paletteFor(index)} />
                ))}
              </Pie>
              <Tooltip
                content={
                  <ChartTooltip
                    formatValue={(value, entry) =>
                      `${Number(value).toLocaleString()} · ${formatPercent(
                        Number(value),
                        total,
                      )} · ${formatBytes(Number(entry.payload?.bytes ?? 0))}`
                    }
                  />
                }
              />
            </PieChart>
          </ChartFrame>
        </div>

        <ul style={{ flex: "1 1 320px", listStyle: "none", padding: 0, margin: 0, fontSize: "0.8rem" }}>
          {formats.map((format, index) => (
            <li
              key={format.extension}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                padding: "3px 0",
                opacity: index < palette.length ? 1 : 0.75,
              }}
            >
              <LegendSwatch color={paletteFor(index)} />
              <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                {format.extension ? `.${format.extension}` : t("stats.noExtension")}
                <span style={{ ...muted }}> · {format.mime}</span>
              </span>
              <span style={{ marginLeft: "auto", whiteSpace: "nowrap" }}>
                {format.archives.toLocaleString()} · {formatPercent(format.archives, total)}
              </span>
            </li>
          ))}
        </ul>
      </div>

      <div style={{ marginTop: 12 }}>
        <StatsTable
          head={[t("stats.format"), t("stats.mimeType"), t("stats.archives"), t("stats.pages"), t("stats.storageUsed")]}
          rows={formats.map((format) => [
            format.extension ? `.${format.extension}` : t("stats.noExtension"),
            format.mime,
            format.archives.toLocaleString(),
            format.pages.toLocaleString(),
            formatBytes(format.bytes),
          ])}
        />
      </div>
    </StatsSection>
  )
}
