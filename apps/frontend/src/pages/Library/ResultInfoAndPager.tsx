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
}: {
  rangeStart: number
  rangeEnd: number
  totalFiltered: number
  totalRecords: number
  page: number
  pageCount: number
  onPage: (page: number) => void
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
              href="#"
              className={`paginate_button${p === page ? " current" : ""}`}
              style={{ margin: "4px 0" }}
              onClick={(e) => {
                e.preventDefault()
                onPage(p)
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
