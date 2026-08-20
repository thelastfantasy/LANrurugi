import { useTranslation } from "react-i18next"

/** DataTables' own `pagingType: "simple_numbers"` window — the default it falls back to since
 * `index_datatables.js` never sets `pagingType` explicitly. Always includes the first and last
 * page, a run of pages centered on the current one, and `null` markers (rendered as "…") for any
 * gap wider than one page. `page` and the returned numbers are 0-based; only the rendered label
 * is +1. */
export function pagingWindow(page: number, pageCount: number): (number | null)[] {
  const windowSize = 4
  const result: (number | null)[] = []
  let prev: number | null = null
  for (let p = 0; p < pageCount; p++) {
    const show = p === 0 || p === pageCount - 1 || Math.abs(p - page) <= windowSize
    if (!show) continue
    if (prev !== null && p - prev > 1) result.push(null)
    result.push(p)
    prev = p
  }
  return result
}

/** The count text (`.dataTables_info`) and the numbered pager (`.dataTables_paginate`) as ONE
 * indivisible unit — merged into a single component so they always move/wrap together as a block
 * inside a shared flex row, instead of being two independent flex children that can drift apart
 * and re-order on narrow viewports. */
export function ResultInfoAndPager({
  rangeStart,
  rangeEnd,
  totalFiltered,
  totalRecords,
  page,
  pageCount,
  onPage,
  buildHref,
}: {
  rangeStart: number
  rangeEnd: number
  totalFiltered: number
  totalRecords: number
  page: number
  pageCount: number
  onPage: (page: number) => void
  /** Builds a real, page-specific URL (the app's own `?p=N&...` query string, other params
   * preserved) for a given 0-based page — so each pager link's `href` is something meaningful to
   * right-click-copy/middle-click-open/hover-preview, not the placeholder `href="#"` this
   * previously used (which right-click-copied as just the current page's own URL with a bare
   * trailing `#`, carrying no page number at all). */
  buildHref: (page: number) => string
}) {
  const { t } = useTranslation()
  return (
    <div>
      <div style={{ textAlign: "center", opacity: 0.7 }}>
        {t("library.showingStartToEndOf", {
          start: rangeStart,
          end: rangeEnd,
          total: totalFiltered,
        })}
        {totalRecords > totalFiltered && ` (filtered from ${totalRecords} total entries)`}
      </div>
      <div style={{ textAlign: "center" }}>
        {pagingWindow(page, pageCount).map((p, i) =>
          p === null ? (
            <span key={`ellipsis-${i}`} className="ellipsis">
              …
            </span>
          ) : (
            <a
              key={p}
              href={buildHref(p)}
              className={`paginate_button${p === page ? " current" : ""}`}
              style={{ margin: "4px 0", ...(p === page ? { cursor: "default" } : undefined) }}
              // The current page's own link stays a real, right-click-copyable/middle-click-
              // openable `href` (its own URL) — only the plain-left-click "navigate" behavior is
              // suppressed, since re-applying the page you're already on is a no-op that would
              // otherwise still flash the library through a full re-fetch for nothing.
              onClick={(e) => {
                e.preventDefault()
                if (p !== page) onPage(p)
              }}
            >
              {p + 1}
            </a>
          ),
        )}
      </div>
    </div>
  )
}
