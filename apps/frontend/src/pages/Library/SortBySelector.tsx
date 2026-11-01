import { useTranslation } from "react-i18next"
import { FaArrowDownAZ, FaArrowUpAZ } from "react-icons/fa6"

import type { StatTag } from "@/api/types"
import { IconButton } from "@/components/common-ui/Form"
import { RELEVANCE_SORT } from "@/lib/sortOrder"

export function SortBySelector({
  sortby,
  order,
  stats,
  onSortBy,
  onToggleOrder,
}: {
  sortby: string
  order: "asc" | "desc"
  stats: StatTag[] | undefined
  onSortBy: (key: string) => void
  onToggleOrder: () => void
}) {
  const { t } = useTranslation()

  /** Fixed sorts are translated; tag namespaces (`artist`, `category`, …) stay as-is because they
   *  are literal search-syntax names, not UI words. */
  const label = (key: string) => {
    if (key === "title") return t("common.title")
    if (key === "date_added") return t("library.date")
    if (key === RELEVANCE_SORT) return t("library.relevance")
    return key.charAt(0).toUpperCase() + key.slice(1)
  }

  const namespaces = [
    ...new Set([
      ...(stats ?? []).map((s) => s.namespace).filter((n): n is string => !!n && n !== "date_added"),
      ...(sortby !== "title" && sortby !== "date_added" && sortby !== RELEVANCE_SORT ? [sortby] : []),
    ]),
  ].sort()

  const options = ["title", "date_added", RELEVANCE_SORT, ...namespaces]

  return (
    <div className="thumbnail-options" style={{ display: "flex", alignItems: "center" }}>
      {t("library.sortBy")}{" "}
      <select
        className="favtag-btn"
        value={sortby}
        onChange={(e) => onSortBy(e.target.value)}
        style={{ margin: 0 }}
      >
        {options.map((key) => (
          <option key={key} value={key}>
            {label(key)}
          </option>
        ))}
      </select>
      {/* Relevance is always most-relevant-first — there is no reverse to toggle, so the control
          is hidden rather than left showing a direction the request ignores. */}
      {sortby !== RELEVANCE_SORT && (
        <IconButton
          icon={order === "asc" ? <FaArrowDownAZ size={18} /> : <FaArrowUpAZ size={18} />}
          size={25}
          title={t("library.sortOrder") ?? undefined}
          style={{ border: "none", background: "transparent" }}
          onClick={onToggleOrder}
        />
      )}
    </div>
  )
}
