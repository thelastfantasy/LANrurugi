import "./UsagePanel.css"

import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { fetchUsage, updateBudget } from "../api"
import type { UsageSnapshot } from "../types"

/**
 * Current consumption against the configured budget (T045, FR-014).
 *
 * All four granularities the requirement names are shown at once — the point is that a user can
 * check spend *at any time*, not only when a limit is hit, so this is a plain always-available
 * readout rather than a warning that appears at a threshold.
 *
 * The "chart-style visualization" in FR-014 is a SHOULD, not a MUST; the per-granularity bars below
 * give the same at-a-glance comparison without pulling in a charting dependency for four numbers.
 */
export function UsagePanel({ archiveId, page }: { archiveId?: string; page?: number }) {
  const { t } = useTranslation()
  const [usage, setUsage] = useState<UsageSnapshot | null>(null)
  const [limitDraft, setLimitDraft] = useState("")

  useEffect(() => {
    let cancelled = false
    void fetchUsage(archiveId, page)
      .then((snapshot) => {
        if (!cancelled) setUsage(snapshot)
      })
      .catch(() => {
        // Usage is informational; a fetch failure shouldn't disturb the rest of the settings page.
      })
    return () => {
      cancelled = true
    }
  }, [archiveId, page])

  if (!usage) return null

  if (!usage.provider) {
    return (
      <div className="translation-usage">
        <h3>{t("translation.usage.title")}</h3>
        <p className="helptext">{t("translation.usage.none")}</p>
      </div>
    )
  }

  const exhausted = usage.limit !== null && usage.consumptionToday >= usage.limit
  const rows: [string, number][] = [
    [t("translation.usage.currentPage"), usage.consumptionCurrentPage],
    [t("translation.usage.currentArchive"), usage.consumptionCurrentArchive],
    [t("translation.usage.today"), usage.consumptionToday],
    [t("translation.usage.currentWeek"), usage.consumptionCurrentWeek],
  ]
  // Scale bars against the largest value so the comparison stays readable at any absolute scale.
  const peak = Math.max(1, ...rows.map(([, value]) => value))

  const onSaveLimit = async () => {
    const parsed = Number(limitDraft)
    const next = Number.isFinite(parsed) && parsed > 0 ? parsed : null
    await updateBudget(next)
    setUsage({ ...usage, limit: next })
    setLimitDraft("")
  }

  return (
    <div className="translation-usage">
      <h3>{t("translation.usage.title")}</h3>

      <dl className="translation-usage__rows">
        {rows.map(([label, value]) => (
          <div key={label} className="translation-usage__row">
            <dt>{label}</dt>
            <dd>
              <span className="translation-usage__value">
                {t("translation.usage.tokens", { count: value })}
              </span>
              <span
                className="translation-usage__bar"
                /* Inline width is a genuinely runtime-computed value (a proportion of live data),
                 * which is exactly the case constitution Principle VII still permits inline — the
                 * static styling lives in the stylesheet. */
                style={{ width: `${Math.round((value / peak) * 100)}%` }}
                aria-hidden="true"
              />
            </dd>
          </div>
        ))}
      </dl>

      <p className={exhausted ? "translation-usage__exhausted" : "helptext"}>
        {usage.limit === null
          ? t("translation.usage.noLimit")
          : exhausted
            ? t("translation.usage.exhausted")
            : t("translation.usage.remaining", {
                count: Math.max(0, usage.limit - usage.consumptionToday),
              })}
      </p>

      <label className="translation-usage__limit">
        {t("translation.usage.limit")}
        <input
          type="number"
          min={0}
          className="number-input-no-native-spinner"
          value={limitDraft}
          placeholder={usage.limit === null ? "" : String(usage.limit)}
          onChange={(e) => setLimitDraft(e.target.value)}
        />
        <button type="button" className="stdbtn" onClick={() => void onSaveLimit()}>
          {t("translation.usage.setLimit")}
        </button>
      </label>
    </div>
  )
}

export default UsagePanel
