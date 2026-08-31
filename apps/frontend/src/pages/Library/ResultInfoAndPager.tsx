import { useTranslation } from "react-i18next"

/** DataTables' `simple_numbers` pagination window: first/last page, a run centered on the
 * current page, `null` markers (rendered as "…") for gaps. `page` and results are 0-based. */
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

/** The count text and numbered pager as one unit, so they move/wrap together as a block. */
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
  /** Builds a real, page-specific `?p=N&...` URL so each link is right-click-copyable. */
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
              // href stays real/copyable; only left-click navigation to the current page is a no-op.
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
