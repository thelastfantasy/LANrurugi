import { useTranslation } from "react-i18next"

import { promptDialog } from "@/dialog"
import { useCustomColumnNamespace } from "@/hooks/useCustomColumnNamespace"

import { SortableHeaderLink } from "./SortableHeaderLink"

export function CustomColumnHeader({
  index,
  sortby,
  order,
  onSort,
}: {
  index: number
  sortby: string
  order: "asc" | "desc"
  onSort: (key: string) => void
}) {
  const { t } = useTranslation()
  const [namespace, setNamespace] = useCustomColumnNamespace(index)
  const label = namespace.charAt(0).toUpperCase() + namespace.slice(1)
  return (
    <th id={`customheader${index}`} style={{ width: 100 }} className={sortby === namespace ? `sorting_${order}` : undefined}>
      <i
        className="fas fa-pencil-alt edit-header-btn"
        title={t("Edit this column") ?? undefined}
        style={{ cursor: "pointer" }}
        onClick={(e) => {
          e.stopPropagation()
          void (async () => {
            const next = await promptDialog(t("Tag namespace") ?? "", namespace)
            if (next?.trim()) setNamespace(next.trim())
          })()
        }}
      ></i>{" "}
      <SortableHeaderLink label={label} sortKey={namespace} onSort={onSort} />
    </th>
  )
}
