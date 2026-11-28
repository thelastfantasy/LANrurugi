import { useTranslation } from "react-i18next"

import type { StatsOverview } from "@/api/types"
import { buildSearchToken } from "@/lib/tagFormat"

import { useChartPalette } from "./chartPalette"
import { BarRow, LegendSwatch, StatsSection, StatsTable, useStatsPanel } from "./Charts"
import { formatPercent, ratingColor, seriesColor } from "./format"

/** i18n key for a promoted namespace's own heading; anything else falls back to the raw tag
 *  namespace name, exactly how the rest of the app namespaces tags. */
function namespaceLabelKey(namespace: string): string {
  return `stats.prefNamespace.${namespace}`
}

/** The archives behind a tag's count here: only the read ones, as the statistics count them, not
 *  every archive in the library carrying the tag. */
function readTagSearchURL(namespace: string, tag: string): string {
  return `/?q=${encodeURIComponent(`${buildSearchToken(namespace, tag, true)} is:read`)}`
}

/** A fixed line height for the column headings: with `normal`, a Latin-only label ("Coser") sat a
 *  few pixels shorter than a CJK one, so its column started higher than its neighbours. */
const HEADING_LINE_HEIGHT = 1.4

/** Which tags, authors and categories the reading history actually points at.
 *
 *  A deliberately simple, explainable weighted count rather than anything learned: every archive
 *  the server has a reading signal for contributes its weight to each of its content tags and to
 *  each category holding it, where the weight rises with completion, recency, the reader's own
 *  rating, bookmarks and how often the work has been returned to (the model is spelled out in
 *  `crates/lanrurugi-api/src/stats.rs::preference_weights`). The weights are relative, not
 *  probabilities, so they are shown as ranking plus a bar length — never as a percentage of
 *  "taste". */
export function Preferences({ preferences }: { preferences: StatsOverview["preferences"] }) {
  const { t } = useTranslation()
  const palette = useChartPalette()
  const { muted } = useStatsPanel()

  if (preferences.read_archives === 0) {
    return (
      <StatsSection title={t("stats.preferenceAnalysis")} icon="fa-star" hint={t("stats.preferenceAnalysisHint")}>
        <p style={{ ...muted, fontSize: "0.8rem", margin: 0 }}>{t("stats.noReadingRecords")}</p>
      </StatsSection>
    )
  }

  return (
    <StatsSection
      title={t("stats.preferenceAnalysis")}
      icon="fa-star"
      hint={t("stats.preferenceAnalysisModel", { count: preferences.read_archives })}
    >
      <div style={{ marginTop: 16 }}>
        <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.topTagsPerNamespace")}</h4>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 18 }}>
          {preferences.by_namespace.map((ns, nsIndex) => {
            const maxTag = ns.tags.reduce((max, tag) => Math.max(max, tag.weight), 0)
            const total = ns.tags.reduce((sum, tag) => sum + tag.weight, 0)
            return (
              <div key={ns.namespace} style={{ flex: "1 1 260px", minWidth: 240 }} data-namespace={ns.namespace}>
                <div style={{ fontSize: "0.9rem", lineHeight: HEADING_LINE_HEIGHT, marginBottom: 6 }}>
                  <LegendSwatch color={seriesColor(palette, nsIndex)} />
                  <b>{t(namespaceLabelKey(ns.namespace), { defaultValue: ns.namespace })}</b>
                </div>
                {ns.tags.map((tag) => (
                  <BarRow
                    key={tag.text}
                    label={tag.text}
                    value={tag.weight}
                    max={maxTag}
                    valueLabel={t("stats.readArchivesValue", { count: tag.archives })}
                    hint={formatPercent(tag.weight, total)}
                    color={seriesColor(palette, nsIndex)}
                    href={readTagSearchURL(ns.namespace, tag.text)}
                    linkTitle={t("stats.searchReadWithTag", { tag: `${ns.namespace}:${tag.text}` })}
                  />
                ))}
              </div>
            )
          })}
        </div>
      </div>

      <div style={{ marginTop: 16 }}>
        <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.ratingAmongRead")}</h4>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 24 }}>
          <div style={{ flex: "1 1 320px", minWidth: 280 }}>
            {preferences.ratings.map((tier) => (
              <BarRow
                key={tier.stars}
                label={tier.stars === 0 ? t("stats.unrated") : t("stats.starsValue", { count: tier.stars })}
                value={tier.archives}
                max={preferences.ratings.reduce((max, entry) => Math.max(max, entry.archives), 0)}
                valueLabel={`${tier.archives.toLocaleString()} · ${formatPercent(
                  tier.archives,
                  preferences.read_archives,
                )}`}
                color={ratingColor(palette, tier.stars)}
              />
            ))}
          </div>
          <p style={{ flex: "1 1 260px", minWidth: 240, ...muted, fontSize: "0.78rem", margin: 0 }}>
            {t("stats.ratingAmongReadHint", { count: preferences.read_archives })}
          </p>
        </div>
      </div>

      <div style={{ marginTop: 16 }}>
        <h4 style={{ fontSize: "0.9rem", margin: "0 0 8px" }}>{t("stats.categoryReadRates")}</h4>
        {preferences.categories.length === 0 ? (
          <p style={{ ...muted, fontSize: "0.78rem", margin: 0 }}>{t("stats.noCategories")}</p>
        ) : (
          <>
            <StatsTable
              head={[
                t("stats.category"),
                t("stats.archives"),
                t("stats.readArchives"),
                t("stats.readShare"),
              ]}
              rows={preferences.categories.map((category) => [
                <a key={category.id} href={`/?c=${encodeURIComponent(category.id)}`}>
                  {category.name}
                </a>,
                category.total.toLocaleString(),
                category.read.toLocaleString(),
                formatPercent(category.read, category.total),
              ])}
            />
            {preferences.categories_truncated ? (
              <p style={{ ...muted, fontSize: "0.72rem", marginTop: 8 }}>
                {t("stats.categoriesTruncated")}
              </p>
            ) : null}
          </>
        )}
      </div>
    </StatsSection>
  )
}
