import { useTranslation } from "react-i18next"
import { FaArrowDownAZ, FaArrowUpAZ } from "react-icons/fa6"

import type { StatTag } from "@/api/types"
import { IconButton } from "@/components/common-ui/Form"

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
      {t("library.sortBy")}{" "}
      <select
        className="favtag-btn"
        value={sortby}
        onChange={(e) => onSortBy(e.target.value)}
        style={{ margin: 0 }}
      >
        <option value="title">{t("common.title")}</option>
        <option value="date_added">{t("library.date")}</option>
        {namespaces.map((ns) => (
          <option key={ns} value={ns}>
            {ns.charAt(0).toUpperCase() + ns.slice(1)}
          </option>
        ))}
      </select>
      <IconButton
        icon={order === "asc" ? <FaArrowDownAZ size={18} /> : <FaArrowUpAZ size={18} />}
        size={25}
        title={t("library.sortOrder") ?? undefined}
        style={{ border: "none", background: "transparent" }}
        onClick={onToggleOrder}
      />
    </div>
  )
}
