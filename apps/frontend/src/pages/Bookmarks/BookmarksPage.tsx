import type { CSSProperties } from "react"
import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useInfiniteBookmarks } from "@/api/hooks"
import type { BookmarkSort } from "@/api/types"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"

import { BookmarkedArchiveHoverCard } from "./BookmarkedArchiveHoverCard"
import { HoverGridOrderSettingsMenu } from "./HoverGridOrderSettingsMenu"

const GRID_GAP = 16
/** Fixed column counts — desktop always shows 5, the `560px` breakpoint (matching `lrr.css`'s own
 * mobile breakpoint) always shows 2 — rather than however many `228px`-wide cards happen to fit.
 * Column *width* is then derived from the real container width (see `columnWidth` below) and fed
 * back into `div.id1`'s own CSS as `--bookmark-card-width` (`index.css`), so the card genuinely
 * scales to fill its column instead of the grid trying to match a card stuck at a fixed size. */
const DESKTOP_COLUMNS = 5
const MOBILE_COLUMNS = 2
const MOBILE_BREAKPOINT = 560

/** `div.id1`'s real rendered footprint at a given CSS `width` is wider than `width` itself — every
 * theme file (e.g. `themes/g.css`) gives it `margin: 3px 2px 2px 3px` plus a `content-box` `1px`
 * border, and `.id1` is `display: inline-block`, so its actual right edge lands `marginLeft +
 * borderBox width` past whatever contains it, i.e. `width + 5`, not just `width` (confirmed live:
 * setting `.id1`'s `width` to a column's full share bled its visible right edge 5px past the
 * grid's own column boundary). `cardWidthFor` backs that 5px out of the column's own share before
 * handing it to `.id1` as `--bookmark-card-width` (`index.css`), so `.id1`'s real visible edge —
 * not just its declared `width` — lands exactly on the column boundary, keeping the "line up with
 * the sort `<select>`" edge correct regardless of how wide each column ends up being. */
const CARD_SPILL = 5

/** Full width of one grid column/track, given how many columns the row is fixed to. */
function columnWidth(containerWidth: number, columns: number): number {
  return (containerWidth - (columns - 1) * GRID_GAP) / columns
}


/** Independent bookmarks page (replaces the old "link a category to be the bookmark" mechanism)
 * — lists every archive with at least one page-level bookmark, one card per archive, hovering a
 * card expands a thumbnail grid of that archive's own bookmarked pages
 * (`BookmarkedArchiveHoverCard`). Skeleton mirrors `pages/Activity/ActivityPage.tsx`, but paginates
 * via `useInfiniteBookmarks` + an `IntersectionObserver` sentinel (same `rootMargin: "240px"`
 * pattern as `ArchiveOverviewOverlayPanel.tsx::PagePlaceholder`) instead of Activity's own
 * cursor-stack Prev/Next — scroll-to-load-more reads more naturally for a card grid than paged
 * buttons do. Switching `sort` changes the query key, which resets `useInfiniteQuery` back to the
 * first page on its own; no manual state reset needed. */
export function BookmarksPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  useApplyTheme()
  useDocumentTitle(t("bookmarks.pageTitle") ?? undefined)

  const [sort, setSort] = useState<BookmarkSort>("bookmarked_at")
  const bookmarks = useInfiniteBookmarks(sort)
  const entries = bookmarks.data?.pages.flatMap((page) => page.entries) ?? []
  const isEmpty = !bookmarks.isLoading && entries.length === 0

  const { hasNextPage, isFetchingNextPage, fetchNextPage } = bookmarks
  const sentinelRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = sentinelRef.current
    if (!el || !hasNextPage) return
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting && !isFetchingNextPage) void fetchNextPage()
      },
      { rootMargin: "240px" },
    )
    observer.observe(el)
    return () => observer.disconnect()
  }, [hasNextPage, isFetchingNextPage, fetchNextPage])

  // Column count is fixed per breakpoint (`DESKTOP_COLUMNS`/`MOBILE_COLUMNS`), not however many
  // fixed-width cards happen to fit (`repeat(auto-fill, ...)` — tried and rejected, see the grid's
  // own comment below). What's measured here, off `gridWrapperRef`'s own rendered width (a plain
  // block element, `width: auto`, fills `.ido`), is only the container width needed to turn that
  // fixed column count into an actual per-column pixel width (`cardWidthFor`).
  const gridWrapperRef = useRef<HTMLDivElement>(null)
  const [layout, setLayout] = useState({ columns: DESKTOP_COLUMNS, trackWidth: 0, cardWidth: 0 })
  const hasEntries = entries.length > 0
  // Deliberately depends on `hasEntries` (not `[]`) — `gridWrapperRef`'s div is only rendered once
  // `entries` is non-empty, so an empty-deps effect that runs once at mount (before the first
  // successful fetch resolves) would find `gridWrapperRef.current` still `null`, bail out via the
  // early `return`, and then never run again once the div actually exists — confirmed live as
  // exactly this: layout stuck at its initial value even once cards had loaded and the wrapper was
  // rendered at its real (much wider) size.
  useLayoutEffect(() => {
    const el = gridWrapperRef.current
    if (!el) return
    const observer = new ResizeObserver(([entry]) => {
      const width = entry.contentRect.width
      const columns = width < MOBILE_BREAKPOINT ? MOBILE_COLUMNS : DESKTOP_COLUMNS
      const trackWidth = columnWidth(width, columns)
      setLayout({ columns, trackWidth, cardWidth: trackWidth - CARD_SPILL })
    })
    observer.observe(el)
    return () => observer.disconnect()
  }, [hasEntries])

  return (
    <div className="ido" style={{ paddingLeft: 12, paddingRight: 12, boxSizing: "border-box" }}>
      <h1 className="ih">{t("bookmarks.pageTitle")}</h1>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("bookmarks.pageDescription")}</p>

      {/* `position: sticky` — stays visible while scrolling through a long card grid, rather than
          needing a scroll back to the top just to change either sort. `background: "inherit"`
          (not a hardcoded color) picks up `.ido`'s own theme-specific background — each of the 5
          real themes sets a different value for it (`themes/g.css`'s own `#EDEBDF`, etc.) — so
          cards scrolling underneath this bar don't show through it, without needing a per-theme
          override of this bar's own background the way a genuinely new color would (per this
          repo's own "custom colors must be theme-aware" rule — inheriting an already-theme-aware
          value isn't a new color at all). `zIndex: 1` clears the card grid below (`z-index: auto`
          by default) so a card's own thumbnail never paints over this bar during the scroll. */}
      <div
        style={{
          position: "sticky",
          top: 0,
          zIndex: 1,
          background: "inherit",
          display: "flex",
          justifyContent: "flex-end",
          alignItems: "center",
          gap: 8,
          marginTop: 8,
          paddingTop: 4,
          paddingBottom: 4,
        }}
      >
        <select
          className="favtag-btn"
          value={sort}
          onChange={(e) => setSort(e.target.value as BookmarkSort)}
        >
          <option value="bookmarked_at">{t("bookmarks.sortByBookmarkedAt")}</option>
          <option value="title">{t("bookmarks.sortByTitle")}</option>
          <option value="date_added">{t("bookmarks.sortByDateAdded")}</option>
        </select>
        <HoverGridOrderSettingsMenu />
      </div>

      {bookmarks.isLoading && (
        <div id="processing">
          <i className="fa fa-3x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {isEmpty && (
        <div style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-bookmark"></i>
          <p>{t("bookmarks.noBookmarksYet")}</p>
        </div>
      )}

      {entries.length > 0 && (
        // `gridWrapperRef` — a plain block div, `width: auto`, fills `.ido` — exists only to be
        // measured (see the `ResizeObserver` above): its rendered width is how `layout` gets
        // picked. The grid's own tracks are set to that same computed `trackWidth` in px
        // (`columnWidth`) rather than `1fr` — `1fr` was tried first and rejected: its resolved
        // track width didn't exactly match `columnWidth`'s own arithmetic (confirmed live, off by
        // a couple px — plausibly `1fr`'s own internal rounding), so `.id1`'s width (set from the
        // same `columnWidth` via `--bookmark-card-width`) ended up sized for a slightly different
        // track than the grid actually rendered. Using one shared computed number for *both* the
        // grid's `gridTemplateColumns` and `.id1`'s CSS variable removes that gap entirely — they
        // can't disagree if they're the same number.
        <div ref={gridWrapperRef}>
          <div
            className="bookmarks-grid"
            style={
              {
                display: "grid",
                gridTemplateColumns: `repeat(${layout.columns}, ${layout.trackWidth}px)`,
                gap: GRID_GAP,
                marginTop: 16,
                "--bookmark-card-width": `${layout.cardWidth}px`,
              } as CSSProperties
            }
          >
            {entries.map((entry) => (
              <BookmarkedArchiveHoverCard
                key={entry.archive.arcid}
                entry={entry}
                cropThumbs={true}
                onContextMenu={(e) => e.preventDefault()}
                onOpen={(id) => navigate(routes.reader(id))}
              />
            ))}
          </div>
        </div>
      )}

      {bookmarks.hasNextPage && <div ref={sentinelRef} style={{ height: 1 }} />}
      {bookmarks.isFetchingNextPage && (
        <div style={{ textAlign: "center", margin: "12px 0" }}>
          <i className="fa fa-2x fa-compact-disc fa-spin"></i>
        </div>
      )}

      {/* `position: sticky` + `bottom: 0` — stays reachable regardless of how far the infinite
          scroll has loaded, rather than sitting at the true end of an ever-growing card list
          (effectively unreachable without scrolling past everything). Same `background: "inherit"`/
          `zIndex: 1` reasoning as the sort bar's own sticky wrapper above — picks up `.ido`'s
          theme-specific background so cards scrolling underneath don't show through, and clears
          the card grid's default stacking order. */}
      <div
        style={{ position: "sticky", bottom: 0, zIndex: 1, background: "inherit", paddingTop: 12, paddingBottom: 12 }}
      >
        <div className="control-btn-group" style={{ justifyContent: "center" }}>
          <input
            type="button"
            className="stdbtn"
            value={t("common.returnToLibrary") ?? undefined}
            onClick={() => navigate(routes.library())}
          />
        </div>
      </div>
    </div>
  )
}
