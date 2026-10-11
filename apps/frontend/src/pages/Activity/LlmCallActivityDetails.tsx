import type { CSSProperties, ReactNode } from "react"
import { useTranslation } from "react-i18next"

import type { ActivityEntry } from "@/api/types"
import { FONT_SIZE_SM } from "@/theme"

import { readLlmCallAfter } from "./OperationDescription"

/** `translation.llm_call`'s own `after` (issue #100) reads far better as a labeled row list —
 * provider/model/page/language, then a token-usage breakdown, then latency/cost — than as the
 * generic raw-JSON code block every other action type falls back to; see
 * `DownloadActivityDetails`'s own docs for the precedent this follows. */

const LABEL_STYLE: CSSProperties = { opacity: 0.65, whiteSpace: "nowrap" }
const VALUE_STYLE: CSSProperties = { margin: 0, wordBreak: "break-word" }

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt style={LABEL_STYLE}>{label}</dt>
      <dd style={VALUE_STYLE}>{children}</dd>
    </>
  )
}

export function LlmCallActivityDetails({ entry }: { entry: ActivityEntry }) {
  const { t } = useTranslation()
  const after = readLlmCallAfter(entry.after)
  if (!after) return null

  const rows: ReactNode[] = []
  const addRow = (label: string, value: ReactNode) => {
    if (value === null || value === undefined || value === "") return
    rows.push(
      <Row key={label} label={label}>
        {value}
      </Row>,
    )
  }

  addRow(
    t("activity.llmCallProvider"),
    after.provider ? (after.model ? `${after.provider} · ${after.model}` : after.provider) : undefined,
  )
  addRow(t("activity.llmCallPage"), typeof after.page === "number" ? t("bookmarks.pageLabel", { page: after.page }) : undefined)
  addRow(t("activity.llmCallTargetLanguage"), after.targetLanguage)

  if (typeof after.totalTokens === "number") {
    const parts: string[] = [after.totalTokens.toLocaleString()]
    const breakdown: string[] = []
    if (typeof after.promptTokens === "number") breakdown.push(`${t("activity.llmCallPromptTokens")} ${after.promptTokens.toLocaleString()}`)
    if (typeof after.cachedPromptTokens === "number") {
      breakdown.push(`${t("activity.llmCallCachedTokens", { count: after.cachedPromptTokens })}`)
    }
    if (typeof after.cacheCreationTokens === "number" && after.cacheCreationTokens > 0) {
      breakdown.push(`${t("activity.llmCallCacheCreationTokens")} ${after.cacheCreationTokens.toLocaleString()}`)
    }
    if (typeof after.completionTokens === "number") breakdown.push(`${t("activity.llmCallCompletionTokens")} ${after.completionTokens.toLocaleString()}`)
    addRow(
      t("activity.llmCallTokens"),
      <>
        {parts.join("")}
        {breakdown.length > 0 && <span style={{ opacity: 0.7 }}> ({breakdown.join(", ")})</span>}
      </>,
    )
  }

  addRow(t("activity.llmCallLatency"), typeof after.providerLatencyMs === "number" ? `${after.providerLatencyMs.toLocaleString()} ms` : undefined)
  addRow(
    t("activity.llmCallEstimatedCost"),
    typeof after.estimatedCostUsd === "number" ? `$${after.estimatedCostUsd.toFixed(4)}` : t("activity.llmCallCostUnavailable"),
  )

  if (rows.length === 0) return null

  return (
    <dl
      style={{
        display: "grid",
        gridTemplateColumns: "auto 1fr",
        alignItems: "baseline",
        columnGap: 12,
        rowGap: 6,
        fontSize: FONT_SIZE_SM,
        margin: "16px 0 0",
      }}
    >
      {rows}
    </dl>
  )
}
