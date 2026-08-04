import { useTranslation } from "react-i18next"

import { CustomColumnHeader } from "./CustomColumnHeader"
import { SortableHeaderLink } from "./SortableHeaderLink"

export function CompactTableHeader({
  columns,
  sortby,
  order,
  onSort,
}: {
  columns: number
  sortby: string
  order: "asc" | "desc"
  onSort: (key: string) => void
}) {
  const { t } = useTranslation()
  return (
    <tr>
      <th id="titleheader" className={sortby === "title" ? `sorting_${order}` : undefined}>
        <SortableHeaderLink label={t("Title")} sortKey="title" onSort={onSort} />
      </th>
      {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
        <CustomColumnHeader key={i} index={i} sortby={sortby} order={order} onSort={onSort} />
      ))}
      <th id="tagsheader">{t("Tags")}</th>
    </tr>
  )
}
