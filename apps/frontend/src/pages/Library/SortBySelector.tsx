import { useTranslation } from "react-i18next"

import type { StatTag } from "@/api/types"

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

  const namespaces = [
    ...new Set([
      ...(stats ?? []).map((s) => s.namespace).filter((n): n is string => !!n && n !== "date_added"),
      ...(sortby !== "title" && sortby !== "date_added" ? [sortby] : []),
    ]),
  ].sort()

  return (
    <div className="thumbnail-options" style={{ display: "flex", alignItems: "center" }}>
      {t("Sort by:")}{" "}
      <select
        className="favtag-btn"
        value={sortby}
        onChange={(e) => onSortBy(e.target.value)}
      >
        <option value="title">{t("Title")}</option>
        <option value="date_added">{t("Date")}</option>
        {namespaces.map((ns) => (
          <option key={ns} value={ns}>
            {ns.charAt(0).toUpperCase() + ns.slice(1)}
          </option>
        ))}
      </select>
      <a
        className={`fa fa-2x fa-sort-alpha-${order === "asc" ? "down" : "up"} table-option`}
        style={{ position: "relative", top: 6 }}
        href="#"
        title={t("Sort Order") ?? undefined}
        onClick={(e) => {
          e.preventDefault()
          onToggleOrder()
        }}
      ></a>
    </div>
  )
}
