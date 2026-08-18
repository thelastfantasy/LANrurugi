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

// Mirrors legacy's `~/LANraragi/templates/stats.html.tt2` + `public/js/stats.js` structure exactly
// — two genuinely distinct sections that an earlier version of this page had collapsed into one:
// `#tagCloud` (a real jQCloud word cloud — see `TagCloud.tsx`'s own docs for the full port),
// always visible once data loads, directly below the stat counters; and a separate `#detailedStats`
// collapsible (`CollapsibleSection`, titled "Detailed Stats" — *not* "Tag Cloud", which an earlier
// version of this page mislabeled it as) containing `#tagList`, the plain namespace-colored+linked
// list of every tag with its own weight in parentheses, sorted by weight descending. The real
// jQCloud words themselves are deliberately bare `tag.text` with no namespace prefix and no link —
// legacy's own `stats.js` passes the raw API response straight to `jQCloud()` with no `link`/
// namespace-prefixing step, only the separate `#tagList` loop builds real namespaced+linked
// `<a>`s — reproducing that exactly rather than "improving" the cloud words into links.

const STAT_VALUE_FONT_SIZE = 20

export function Stats() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const stats = useStats(2)
  const archives = useArchives()
  const info = useServerInfo()
  useApplyTheme()
  useDocumentTitle(t("stats.libraryStatistics") ?? undefined)
  // `TagCloud`'s own `onTagClick` target: opens `#detailedStats` (via `CollapsibleSection`'s real
  // controlled `open`), scrolls to the clicked tag's own `#tagList` row, and flashes it — all
  // state-driven (see the hook's own docs for why), no `document.querySelector`/`classList` here.
  const tagHighlight = useTagCloudHighlight()

  const sorted = [...(stats.data ?? [])].sort((a, b) => b.weight - a.weight)
  // Same curve `TagCloud.tsx`'s own `radius` already shrinks by for a small tag count (see that
  // function's own docs) — reused here so `#tagCloud`'s *container* shrinks in lockstep with the
  // sphere inside it, instead of a small sphere floating in the middle of an unchanged, still-large
  // square (live-reported: shrinking only the sphere just moved the "too much empty space"
  // complaint from inside the sphere to around it).
  const tagCloudSizeRatio = sphereSizeRatio(sorted.length)

  const archiveCount = archives.data?.length ?? 0
  const contentSizeGb = (archives.data ?? []).reduce((sum, a) => sum + a.size, 0) / 1e9
  const pagesRead = info.data?.total_pages_read ?? 0

  return (
    <div className="ido">
      {/* Legacy's real page title is `<h2 class="ih">` — the theme CSS only ever defines
          `h1.ih { font-size: 10pt; ... }` (verified: no generic `.ih` or `h2.ih` rule exists in
          any theme file), so a `<h2 class="ih">` title deliberately does NOT get that override and
          renders at the plain browser-default `h2` size, while the counters block just below (the
          real `<h1 class="ih">`) DOES match that selector and renders tiny by comparison — an
          inverted-looking but real legacy quirk. An earlier version of this page had the two
          swapped (title as `<h1 className="ih">`, counters as plain `<p>` tags with no `.ih` class
          at all), which put the *title* at the tiny 10pt size and the *counters* at this app's own
          unrelated default paragraph size — reported live from a legacy screenshot comparison. */}
      <h2 className="ih">{t("stats.libraryStatistics")}</h2>

      {/* The whole `#stats` counter block is one real `<h1 class="ih">` in legacy (`<br><br>`
          between each line, not separate `<p>` tags) — matters because `h1.ih`'s real `font-size:
          10pt` cascades to the label text and the trailing "Tag Cloud" line this way, which a
          `<p>` sibling (no `.ih` class of its own) never picked up. The four numeric values keep
          their own separate `font-size: 20px` inline override (a real, distinct legacy value,
          layered on top of the 10pt base rather than replacing it). */}
      <div id="stats">
        <h1 className="ih">
          <i className="fa fa-book fa-2x"></i> <span style={{ fontSize: STAT_VALUE_FONT_SIZE }}>{archiveCount}</span>{" "}
          {t("stats.archivesOnRecord")}
          <br />
          <br />
          <i className="fa fa-tags fa-2x"></i>{" "}
          <span style={{ fontSize: STAT_VALUE_FONT_SIZE }}>
            {/* Legacy shows a spinner in place of the tag count until its own AJAX call resolves,
                then swaps in the real count (`data.length`, i.e. the same already-`minweight`/
                `hide_excluded_namespaces`-filtered set `#tagList`/`#tagCloud` both use) — matched
                here instead of silently reading as "0 tags" for the whole loading window. */}
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
          {/* `aspect-ratio: 1` (not a fixed `height: 500`, the old 2D jQCloud port's own value) —
              `TagCloud.tsx`'s own 3D sphere is always circular regardless of its container's shape
              (see that component's own docs on why `radius` is derived from `Math.min(width,
              height)`), so a wide-but-short container (this page's real `width: 80%` on a typical
              desktop viewport is noticeably wider than the old fixed 500px tall) left the sphere
              constrained to the short side with a lot of dead horizontal space on either side —
              live-reported, confirmed via a real `getBoundingClientRect()` measurement (a 960×500
              container only ever fit a 450×450 sphere). `maxHeight: '70vh'` — a plain unclamped
              square on a wide desktop viewport (`width: 80%` of a wide window is itself wide) grew
              tall enough to need real scrolling to reach `#detailedStats` below it, also
              live-reported. `vh` (viewport-relative, not a fixed px cap like an earlier version of
              this used) scales with the actual screen rather than clamping to one arbitrary number
              that reads differently tall/short depending on the visitor's own display — `TagCloud`'s
              own `RADIUS_RATIO` already accounts for the sphere's own internal ~75%-of-box spread
              factor, so the tags' real visible extent still reaches close to whichever edge (width-
              or height-constrained) ends up shorter here. `width` itself is scaled by
              `tagCloudSizeRatio` (min 80% * 0.3 = 24%) — `aspect-ratio: 1` then derives a matching
              smaller height automatically, so the container shrinks in lockstep with the sphere
              inside it for a small tag count rather than leaving a small sphere floating inside an
              unchanged, still-large square. */}
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
              {/* Legacy's own real value here is a column-flex-wrap layout with a fixed,
                  content-count-independent height (`max-width: 80vw; display: flex; height:
                  calc(2048px - 25vw); flex-direction: column; flex-wrap: wrap`) — designed to
                  spread a *large* tag list into several newspaper-style columns. Reported live as
                  looking wrong with a modest tag count (this test library's 23 tags): the fixed
                  ~1700px column height is never actually filled, so `flex-wrap` never triggers a
                  second column at all — one single sparse column with a large dead space below,
                  not a real multi-column effect. Row-wrap (items flow left-to-right, wrapping to a
                  new row once they run out of horizontal space) degrades gracefully at any tag
                  count instead — few tags fill only as many rows as they need with no leftover
                  space, many tags still end up genuinely multi-column/row rather than needing a
                  height tuned for one specific expected tag count. `maxHeight` + `overflow-y: auto`
                  (rather than legacy's own unconstrained `overflow: auto` on an already-huge fixed
                  height) keeps a very large real library's list from growing the whole accordion
                  body to match. */}
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
                  // A row's own `.tag-list-highlighted` outline sits 2px *outside* its box
                  // (`outline-offset`) — with no padding here, a highlighted row landing on the
                  // very first flex line had that outline's top edge clipped by this container's
                  // own `overflow-y: auto` boundary (live-reported, visible in a screenshot as a
                  // flat-cut top edge only the first row ever showed). This padding gives the
                  // outline room on every side regardless of which row gets highlighted.
                  padding: 4,
                }}
              >
                {sorted.map((tag) => {
                  const key = tagKey(tag)
                  const isHighlighted = tagHighlight.highlightedKey === key
                  return (
                    <a
                      key={key}
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
