import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useArchives, useServerInfo, useStats, useStatsOverview } from "@/api/hooks"
import { CollapsibleSection, TagCloud } from "@/components/Display"
import { sphereSizeRatio } from "@/components/Display/TagCloud"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { tagKey, useTagCloudHighlight } from "@/hooks/useTagCloudHighlight"
import { routes } from "@/lib/routes"
import { getTagSearchURL } from "@/lib/tagFormat"
import { useApplyTheme } from "@/theme"

import { StatsCard, useStatsPanel } from "./Charts"
import { formatBytes } from "./format"
import { FormatDistribution } from "./FormatDistribution"
import { IngestHistory } from "./IngestHistory"
import { Preferences } from "./Preferences"
import { ReadingHistory } from "./ReadingHistory"
import { SizeDistribution } from "./SizeDistribution"

// Mirrors legacy stats.html.tt2 + stats.js: `#tagCloud` (jQCloud sphere, bare tag text, no link)
// stays separate from the `#detailedStats` collapsible's `#tagList` (namespaced+linked entries).
//
// Beyond legacy's own four counters and tag cloud, this page carries a LANrurugi-only dashboard
// (`GET /stats/overview`): size/format distribution, ingestion history, daily reading history, and
// a weighted reading-preference analysis. Legacies' own blocks are kept verbatim below/around it
// rather than replaced — the extra sections are additive, and the legacy counters remain the
// quickest thing to read at the top of the page.

const STAT_VALUE_FONT_SIZE = 20

export function Stats() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const stats = useStats(2)
  const archives = useArchives()
  const overview = useStatsOverview()
  const info = useServerInfo()
  useApplyTheme()
  useDocumentTitle(t("stats.libraryStatistics") ?? undefined)
  const tagHighlight = useTagCloudHighlight()
  const { muted } = useStatsPanel()

  const sorted = [...(stats.data ?? [])].sort((a, b) => b.weight - a.weight)
  const tagCloudSizeRatio = sphereSizeRatio(sorted.length)

  const archiveCount = archives.data?.length ?? 0
  const contentSizeGb = (archives.data ?? []).reduce((sum, a) => sum + a.size, 0) / 1e9
  const pagesRead = info.data?.total_pages_read ?? 0
  const totals = overview.data?.totals

  // `localprogress` keeps reader progress in the browser and never calls `update_progress` at all,
  // so every reading section below would be legitimately empty — say so once, here, rather than
  // letting five separate charts each look broken.
  const localProgressOnly = info.data?.server_tracks_progress === false

  return (
    // The inline side padding overrides `div.ido`'s own — legacy gives that outer container only
    // 5px (`g.css`/`ex.css`) or literally none (`modern*.css`), which was fine for centered text
    // counters but leaves a stack of bordered panels nearly touching the container's border.
    // `border-box` is required with it: `.ido` sets `width: 99%`, and this page's global
    // `box-sizing` is `content-box` (the legacy stylesheets load after Tailwind's preflight and
    // override it), so padding would otherwise widen the container past its own parent. Same fix
    // `ActivityPage`/`BookmarksPage`/`JobsPage` already apply to their own `.ido`.
    <div className="ido" style={{ paddingLeft: 16, paddingRight: 16, boxSizing: "border-box" }}>
      <h2 className="ih">{t("stats.libraryStatistics")}</h2>

      <div id="stats">
        <h1 className="ih">
          <i className="fa fa-book fa-2x"></i> <span style={{ fontSize: STAT_VALUE_FONT_SIZE }}>{archiveCount}</span>{" "}
          {t("stats.archivesOnRecord")}
          <br />
          <br />
          <i className="fa fa-tags fa-2x"></i>{" "}
          <span style={{ fontSize: STAT_VALUE_FONT_SIZE }}>
            {stats.isLoading ? <i className="fa fa-virus fa-spin"></i> : sorted.length}
          </span>{" "}
          {t("stats.differentTagsExisting")}
          <br />
          <br />
          <i className="fa fa-folder-open fa-2x"></i>{" "}
          <span style={{ fontSize: STAT_VALUE_FONT_SIZE }}>{contentSizeGb.toFixed(2)} GB</span> {t("stats.inContentFolder")}
          <br />
          <br />
          <i className="fa fa-book-reader fa-2x"></i> <span style={{ fontSize: STAT_VALUE_FONT_SIZE }}>{pagesRead}</span>{" "}
          {t("stats.pagesRead")}
          <br />
          <br />
          <br />
        </h1>
      </div>

      {overview.isLoading ? (
        <p style={{ ...muted, textAlign: "center" }}>
          <i className="fa fa-dharmachakra fa-2x fa-spin"></i>
          <br />
          {t("stats.loadingOverview")}
        </p>
      ) : overview.isError || !overview.data ? (
        <p style={{ ...muted, textAlign: "center" }}>{t("stats.overviewUnavailable")}</p>
      ) : (
        <div id="statsOverview" style={{ margin: "16px 0" }}>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 10, textAlign: "left" }}>
            <StatsCard
              icon="fa-file-text-o"
              label={t("stats.totalPages")}
              value={totals?.pages.toLocaleString() ?? "-"}
              hint={t("stats.averagePagesValue", { count: totals?.average_pages ?? 0 })}
            />
            <StatsCard
              icon="fa-hdd-o"
              label={t("stats.averageArchiveSize")}
              value={formatBytes(totals?.average_bytes ?? 0)}
              hint={t("stats.largestArchiveValue", { size: formatBytes(totals?.largest_bytes ?? 0) })}
            />
            <StatsCard
              icon="fa-bookmark-o"
              label={t("stats.readingStatus")}
              value={`${(totals?.unread ?? 0).toLocaleString()} / ${(totals?.finished ?? 0).toLocaleString()}`}
              hint={t("stats.unreadFinishedHint")}
            />
            <StatsCard
              icon="fa-spinner"
              label={t("stats.inProgress")}
              value={(totals?.reading ?? 0).toLocaleString()}
              hint={t("stats.newBadgesValue", { count: totals?.new ?? 0 })}
            />
            <StatsCard
              icon="fa-star-half-o"
              label={t("stats.ratedArchives")}
              value={(totals?.rated ?? 0).toLocaleString()}
            />
            <StatsCard
              icon="fa-bookmark"
              label={t("stats.bookmarkCount")}
              value={(totals?.bookmarks ?? 0).toLocaleString()}
            />
            <StatsCard
              icon="fa-clone"
              label={t("stats.tankoubonCount")}
              value={(totals?.tankoubons ?? 0).toLocaleString()}
            />
            <StatsCard
              icon="fa-folder"
              label={t("stats.categoryCount")}
              value={(totals?.categories ?? 0).toLocaleString()}
            />
          </div>

          {localProgressOnly ? (
            <p style={{ ...muted, fontSize: "0.78rem", marginTop: 10 }}>
              <i className="fa fa-info-circle" style={{ marginRight: 6 }} />
              {t("stats.localProgressNotice")}
            </p>
          ) : overview.data.note === "no_server_side_reading" ? (
            <p style={{ ...muted, fontSize: "0.78rem", marginTop: 10 }}>
              <i className="fa fa-info-circle" style={{ marginRight: 6 }} />
              {t("stats.noServerSideReadingNotice")}
            </p>
          ) : null}

          <SizeDistribution buckets={overview.data.size_buckets} />
          <FormatDistribution formats={overview.data.formats} />
          <IngestHistory ingest={overview.data.ingest} />
          <ReadingHistory reading={overview.data.reading} />
          <Preferences preferences={overview.data.preferences} />
        </div>
      )}

      <h3 className="ih" style={{ marginTop: 20 }}>
        {t("stats.tagCloud")}
      </h3>

      {stats.isLoading ? (
        <div id="statsLoading" style={{ width: "80%", marginLeft: "auto", marginRight: "auto", textAlign: "center" }}>
          <p>
            <i className="fa fa-dharmachakra fa-4x fa-spin"></i>
          </p>
          {t("stats.askingTheGreatPowersThat")}
        </div>
      ) : (
        <>
          {/* aspect-ratio: 1 keeps the container circular to match TagCloud's sphere; width
              scales with `tagCloudSizeRatio` so it shrinks with the sphere for small tag counts. */}
          <div
            id="tagCloud"
            style={{
              width: `${80 * tagCloudSizeRatio}%`,
              aspectRatio: "1",
              maxHeight: "70vh",
              marginLeft: "auto",
              marginRight: "auto",
              transition: "width 0.3s ease",
            }}
          >
            <TagCloud tags={sorted} onTagClick={tagHighlight.highlightTag} />
          </div>

          <ul
            className="collapsible extensible with-right-caret"
            id="detailedStats"
            style={{ width: "80%", marginLeft: "auto", marginRight: "auto" }}
          >
            <CollapsibleSection
              id="detailed-stats"
              icon="fa-chart-bar"
              title={t("stats.detailedStats")}
              open={tagHighlight.detailedStatsOpen}
              onOpenChange={tagHighlight.onDetailedStatsOpenChange}
            >
              <div
                id="tagList"
                style={{
                  maxWidth: "80vw",
                  display: "flex",
                  flexDirection: "row",
                  flexWrap: "wrap",
                  alignItems: "flex-start",
                  alignContent: "flex-start",
                  gap: "4px 16px",
                  maxHeight: 500,
                  overflowY: "auto",
                  padding: 4,
                }}
              >
                {sorted.map((tag) => {
                  const key = tagKey(tag)
                  const isHighlighted = tagHighlight.highlightedKey === key
                  return (
                    <a
                      key={key}
                      data-tag-key={key}
                      ref={tagHighlight.highlightedRowRef(key)}
                      href={getTagSearchURL(tag.namespace ?? "", tag.text)}
                      title={tag.namespace ? `${tag.namespace}:${tag.text}` : tag.text}
                      className={[tag.namespace ? `${tag.namespace}-tag` : null, isHighlighted ? "tag-list-highlighted" : null]
                        .filter(Boolean)
                        .join(" ")}
                      style={{ maxWidth: "95%", display: "flex" }}
                    >
                      <span style={{ textOverflow: "ellipsis", whiteSpace: "nowrap", overflow: "hidden", minWidth: 0, maxWidth: "100%" }}>
                        {tag.namespace ? `${tag.namespace}:${tag.text}` : tag.text}
                      </span>
                      &nbsp;
                      <b>({tag.weight})</b>
                    </a>
                  )
                })}
              </div>
              <br />
              {t("stats.theseStatisticsOnlyShowTags")}
            </CollapsibleSection>
          </ul>
        </>
      )}

      <br />
      <input type="button" id="goback" className="stdbtn" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}
