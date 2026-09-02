import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useArchives, useServerInfo, useStats } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display"
import { TagCloud } from "@/components/Display"
import { sphereSizeRatio } from "@/components/Display/TagCloud"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { tagKey, useTagCloudHighlight } from "@/hooks/useTagCloudHighlight"
import { routes } from "@/lib/routes"
import { getTagSearchURL } from "@/lib/tagFormat"
import { useApplyTheme } from "@/theme"

// Mirrors legacy stats.html.tt2 + stats.js: `#tagCloud` (jQCloud sphere, bare tag text, no link)
// stays separate from the `#detailedStats` collapsible's `#tagList` (namespaced+linked entries).

const STAT_VALUE_FONT_SIZE = 20

export function Stats() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const stats = useStats(2)
  const archives = useArchives()
  const info = useServerInfo()
  useApplyTheme()
  useDocumentTitle(t("stats.libraryStatistics") ?? undefined)
  const tagHighlight = useTagCloudHighlight()

  const sorted = [...(stats.data ?? [])].sort((a, b) => b.weight - a.weight)
  const tagCloudSizeRatio = sphereSizeRatio(sorted.length)

  const archiveCount = archives.data?.length ?? 0
  const contentSizeGb = (archives.data ?? []).reduce((sum, a) => sum + a.size, 0) / 1e9
  const pagesRead = info.data?.total_pages_read ?? 0

  return (
    <div className="ido">
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
          {t("stats.tagCloud")}
          <br />
        </h1>
      </div>

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
