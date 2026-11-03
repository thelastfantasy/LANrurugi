import type React from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useLlmUsage } from "@/api/hooks"
import type { LlmUsageFeatureRow, LlmUsageTokens, LlmUsageWindow } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { RadioGroup, RadioItem } from "@/components/common-ui/Form"
import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

/** Per-feature AI usage over today or the last 7 days (`specs/014-ai-harness-hardening` US3). A
 * feature with no calls reads "no data" and an unreported token figure reads "not reported" —
 * never `0`, which would claim the feature cost nothing. */
export function LlmUsageSection() {
  const { t } = useTranslation()
  const [usageWindow, setUsageWindow] = useState<LlmUsageWindow>("day")
  const usage = useLlmUsage(usageWindow)

  return (
    <CollapsibleSection id="llm-usage" icon="fa-chart-bar" title={t("settings.llmUsage")}>
      <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center", padding: "0 12px" }}>
        {t("settings.llmUsageDescription")}
        <RadioGroup
          value={usageWindow}
          onValueChange={setUsageWindow}
          style={{ display: "flex", justifyContent: "center", gap: 16, margin: "8px 0" }}
        >
          <RadioItem value="day">{t("settings.llmUsageToday")}</RadioItem>
          <RadioItem value="week">{t("settings.llmUsageWeek")}</RadioItem>
        </RadioGroup>
      </div>

      {usage.isLoading && <UsageSkeleton />}
      {usage.isError && (
        <div style={{ fontSize: FONT_SIZE_SM, textAlign: "center" }}>{t("settings.llmUsageUnavailable")}</div>
      )}

      {usage.data && (
        <div className="settings-table-scroll">
          <UsageTable>
            {usage.data.features.map((row) => (
              <UsageRow key={row.feature} row={row} />
            ))}
          </UsageTable>
        </div>
      )}
    </CollapsibleSection>
  )
}

type Align = "left" | "right"

/** One definition for every column, so the header, the rows and the skeleton line up. Every cell
 * is one line; breakdowns (cache split, reasoning tokens) live in hover tooltips. */
const COLUMNS: { key: string; label: string; hint?: string; width: string; align: Align }[] = [
  { key: "feature", label: "settings.llmUsageFeature", width: "20%", align: "left" },
  { key: "requests", label: "settings.llmUsageRequests", width: "9%", align: "right" },
  { key: "local", label: "settings.llmUsageLocalCache", hint: "settings.llmUsageLocalCacheHint", width: "11%", align: "right" },
  { key: "input", label: "settings.llmUsageInputTokens", width: "13%", align: "right" },
  { key: "hitRate", label: "settings.llmUsageCacheHitRate", hint: "settings.llmUsageCacheHitRateHint", width: "13%", align: "right" },
  { key: "output", label: "settings.llmUsageOutputTokens", width: "12%", align: "right" },
  { key: "time", label: "settings.llmUsageAvgTime", width: "9%", align: "right" },
  { key: "failures", label: "settings.llmUsageFailures", width: "13%", align: "left" },
]

function cellStyle(align: Align): React.CSSProperties {
  return {
    textAlign: align,
    padding: "4px 10px",
    verticalAlign: "top",
    fontVariantNumeric: "tabular-nums",
  }
}

const MUTED: React.CSSProperties = { opacity: 0.55 }

function UsageTable({ children }: { children: React.ReactNode }) {
  const { t } = useTranslation()
  return (
    <table className="itg" style={{ width: "100%", tableLayout: "fixed" }}>
      <colgroup>
        {COLUMNS.map((c) => (
          <col key={c.key} style={{ width: c.width }} />
        ))}
      </colgroup>
      <thead>
        <tr className="jtr0">
          {COLUMNS.map((c) => (
            <th key={c.key} style={cellStyle(c.align)}>
              {c.hint ? (
                <Tooltip label={t(c.hint)} wrapperStyle={{ display: "inline" }}>
                  <span style={{ textDecoration: "underline dotted", cursor: "help" }}>{t(c.label)}</span>
                </Tooltip>
              ) : (
                t(c.label)
              )}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>{children}</tbody>
    </table>
  )
}

/** One row per feature the endpoint always returns, so the table does not jump when it loads. */
const USAGE_SKELETON_ROWS = 12

/** Placeholder table shaped like the real one while the first answer loads. */
function UsageSkeleton() {
  const { t } = useTranslation()
  return (
    <div className="settings-table-scroll" role="status" aria-label={t("common.loading") ?? "Loading"}>
      <div aria-hidden="true">
        <UsageTable>
          {Array.from({ length: USAGE_SKELETON_ROWS }, (_, row) => (
            <tr className="gtr1 usage-skeleton-row" key={row}>
              {COLUMNS.map((c, col) => (
                <td key={c.key} style={cellStyle(c.align)}>
                  <span
                    className="skeleton-bar"
                    style={{
                      display: "inline-block",
                      width: `${(col === 0 ? 60 : 35) + ((row * 13 + col * 7) % 30)}%`,
                    }}
                  />
                </td>
              ))}
            </tr>
          ))}
        </UsageTable>
      </div>
    </div>
  )
}

function UsageRow({ row }: { row: LlmUsageFeatureRow }) {
  const { t } = useTranslation()
  const name = t(`settings.llmUsageFeature_${row.feature}`, { defaultValue: row.feature })
  if (row.status === "no_data") {
    return (
      <tr className="gtr1">
        <td style={cellStyle("left")}>{name}</td>
        <td style={{ ...cellStyle("right"), ...MUTED }}>{t("settings.llmUsageNoData")}</td>
        {COLUMNS.slice(2).map((c) => (
          <td key={c.key} style={{ ...cellStyle(c.align), ...MUTED }}>
            —
          </td>
        ))}
      </tr>
    )
  }
  const failures = Object.entries(row.failures).filter(([, count]) => count > 0)
  const problems = [
    ...failures.map(([cls, count]) => `${t(`settings.llmUsageClass_${cls}`, { defaultValue: cls })} ${count}`),
    ...(row.cancelled > 0 ? [`${t("settings.llmUsageCancelled")} ${row.cancelled}`] : []),
    ...((row.prefix_breaks ?? 0) > 0 ? [`${t("settings.llmUsagePrefixBreaks")} ${row.prefix_breaks}`] : []),
  ]
  const { cache_hit: hit, cache_miss: miss, reasoning } = row.tokens
  const cacheKnown = hit !== undefined || miss !== undefined
  const cacheTotal = (hit ?? 0) + (miss ?? 0)
  const cacheSplit = cacheKnown
    ? t("settings.llmUsageCacheSplit", { hit: (hit ?? 0).toLocaleString(), miss: (miss ?? 0).toLocaleString() })
    : undefined
  const dash = (align: Align) => <td style={{ ...cellStyle(align), ...MUTED }}>—</td>
  return (
    <tr className="gtr1">
      <td style={cellStyle("left")}>{name}</td>
      <td style={cellStyle("right")}>{row.requests.toLocaleString()}</td>
      {row.local_cache_hits > 0 ? <td style={cellStyle("right")}>{row.local_cache_hits.toLocaleString()}</td> : dash("right")}
      <td style={cellStyle("right")}>
        <WithTooltip label={cacheSplit}>
          <TokenFigure value={row.tokens.prompt} tokens={row.tokens} />
        </WithTooltip>
      </td>
      {cacheKnown && cacheTotal > 0 ? (
        <td style={cellStyle("right")}>
          <WithTooltip label={cacheSplit}>{`${((100 * (hit ?? 0)) / cacheTotal).toFixed(1)}%`}</WithTooltip>
        </td>
      ) : (
        dash("right")
      )}
      <td style={cellStyle("right")}>
        <WithTooltip label={reasoning !== undefined ? t("settings.llmUsageReasoning", { count: reasoning }) : undefined}>
          <TokenFigure value={row.tokens.completion} tokens={row.tokens} />
        </WithTooltip>
      </td>
      {row.requests > 0 ? (
        <td style={cellStyle("right")}>{(row.elapsed_ms / row.requests / 1000).toFixed(1)} s</td>
      ) : (
        dash("right")
      )}
      <td style={problems.length === 0 ? { ...cellStyle("left"), ...MUTED } : cellStyle("left")}>
        {problems.length === 0 ? "—" : problems.join("、")}
      </td>
    </tr>
  )
}

/** Wraps `children` in a hover tooltip when there is something to say, as-is otherwise. */
function WithTooltip({ label, children }: { label: string | undefined; children: React.ReactNode }) {
  if (!label) return <>{children}</>
  return (
    <Tooltip label={label} wrapperStyle={{ display: "inline" }}>
      <span style={{ cursor: "help" }}>{children}</span>
    </Tooltip>
  )
}

/** A token count, "not reported" when absent, and marked when some calls went unmeasured. */
function TokenFigure({ value, tokens }: { value: number | undefined; tokens: LlmUsageTokens }) {
  const { t } = useTranslation()
  if (value === undefined) return <span style={MUTED}>{t("settings.llmUsageNotReported")}</span>
  return (
    <span title={tokens.unreported_calls > 0 ? (t("settings.llmUsageUndercount", { count: tokens.unreported_calls }) ?? undefined) : undefined}>
      {value.toLocaleString()}
      {tokens.unreported_calls > 0 && "*"}
    </span>
  )
}
