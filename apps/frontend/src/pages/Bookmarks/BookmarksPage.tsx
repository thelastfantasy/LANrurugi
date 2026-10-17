import type { CSSProperties } from "react"
import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate, useSearchParams } from "react-router-dom"

import { useInfiniteBookmarks } from "@/api/hooks"
import type { BookmarkSort } from "@/api/types"
import { IconButton, Input, InputGroup } from "@/components/common-ui/Form"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_MD, FONT_SIZE_SM, useApplyTheme } from "@/theme"

import { BookmarkedArchiveHoverCard } from "./BookmarkedArchiveHoverCard"
import { HoverGridOrderSettingsMenu } from "./HoverGridOrderSettingsMenu"

/** How long the search box waits after typing stops before firing a `q`-filtered request. */
const SEARCH_DEBOUNCE_MS = 300

const GRID_GAP = 16
/** Fixed column counts per breakpoint, not however many 228px cards happen to fit — column width
 * is then derived from real container width and fed to `.id1` as `--bookmark-card-width`. */
const DESKTOP_COLUMNS = 5
const MOBILE_COLUMNS = 2
const MOBILE_BREAKPOINT = 560

/** `.id1`'s real rendered footprint is 5px wider than its declared `width` (theme margin +
 * content-box border) — backed out here so its real edge lands on the column boundary. */
const CARD_SPILL = 5

/** Full width of one grid column/track, given how many columns the row is fixed to. */
function columnWidth(containerWidth: number, columns: number): number {
  return (containerWidth - (columns - 1) * GRID_GAP) / columns
}


/** Independent bookmarks page — one card per archive with at least one page-level bookmark;
 * hovering expands a thumbnail grid of that archive's bookmarked pages. */
export function BookmarksPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [searchParams, setSearchParams] = useSearchParams()
  useApplyTheme()
  useDocumentTitle(t("bookmarks.pageTitle") ?? undefined)

  const [sort, setSort] = useState<BookmarkSort>("bookmarked_at")
  const [queryInput, setQueryInput] = useState(() => searchParams.get("q") ?? "")
  const [debouncedQuery, setDebouncedQuery] = useState(() => searchParams.get("q")?.trim() ?? "")
  useEffect(() => {
    const id = window.setTimeout(() => setDebouncedQuery(queryInput.trim()), SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(id)
  }, [queryInput])

  // Reflects the debounced query into the URL via `replace` — never `push` — so typing doesn't
  // spam browser history with one entry per keystroke/debounce tick.
  useEffect(() => {
    setSearchParams(
      (prev) => {
        const next = new URLSearchParams(prev)
        if (debouncedQuery) next.set("q", debouncedQuery)
        else next.delete("q")
        return next
      },
      { replace: true },
    )
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [debouncedQuery])

  const bookmarks = useInfiniteBookmarks(sort, debouncedQuery || undefined)
  const entries = bookmarks.data?.pages.flatMap((page) => page.entries) ?? []
  const isEmpty = !bookmarks.isLoading && entries.length === 0
  const isSearching = debouncedQuery.length > 0

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

  const gridWrapperRef = useRef<HTMLDivElement>(null)
  const [layout, setLayout] = useState({ columns: DESKTOP_COLUMNS, trackWidth: 0, cardWidth: 0 })
  const hasEntries = entries.length > 0
  // Depends on hasEntries, not [] — gridWrapperRef's div only renders once entries is non-empty,
  // so an empty-deps effect running at mount would find it null and never re-run once it exists.
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
        <InputGroup
          // Fixed `width`, not `minWidth` — the group must not shrink-to-fit, or mounting/
          // unmounting the clear-button `endElement` (conditional on `queryInput`) changes the
          // box's own width (confirmed live: 200px empty vs 210px with text).
          style={{ width: 200 }}
          endElement={
            queryInput && (
              <IconButton
                variant="ghost-btn"
                size={18}
                icon="fa fa-times"
                aria-label={t("bookmarks.clearSearch") ?? undefined}
                title={t("bookmarks.clearSearch") ?? undefined}
                onClick={() => setQueryInput("")}
              />
            )
          }
        >
          <Input
            type="text"
            value={queryInput}
            placeholder={t("bookmarks.searchPlaceholder") ?? undefined}
            style={{
              width: "100%",
              // Always reserved, not conditional on `queryInput` — the slot's presence must not
              // change the input's own content box, only whether it's visibly occupied.
              paddingRight: 22,
              // Matches the adjacent `.favtag-btn` sort `<select>`'s look (outset border, bold,
              // rounded, taller) instead of `.stdinput`'s thinner default. Border/text color is
              // left to `.stdinput`'s own per-theme cascade — only the shape/weight is overridden.
              height: 25,
              borderWidth: 2,
              borderStyle: "outset",
              borderRadius: 3,
              fontSize: FONT_SIZE_MD,
              fontWeight: "bold",
            }}
            onChange={(e) => setQueryInput(e.target.value)}
          />
        </InputGroup>
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
          <p>{isSearching ? t("bookmarks.noMatchingBookmarks") : t("bookmarks.noBookmarksYet")}</p>
        </div>
      )}

      {entries.length > 0 && (
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
                highlightQuery={debouncedQuery}
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
