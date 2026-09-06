import type { MouseEvent } from "react"

import type { ArchiveMetadata } from "@/api/types"

import { CompactTableHeader } from "./CompactTableHeader"
import { CompactTableRow } from "./CompactTableRow"

export function CompactTable({
  shown,
  columns,
  selectedIds,
  multiSelect,
  sortby,
  order,
  onSort,
  onSearchTag,
  onToggleSelected,
  onOpen,
  onContextMenu,
}: {
  shown: ArchiveMetadata[]
  columns: number
  selectedIds: string[]
  multiSelect: boolean
  sortby: string
  order: "asc" | "desc"
  onSort: (key: string) => void
  onSearchTag: (namespacedTag: string) => void
  onToggleSelected: (id: string) => void
  onOpen: (id: string) => void
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
}) {
  return (
    <table className="itg" style={{ width: "100%" }}>
      <thead>
        <CompactTableHeader columns={columns} sortby={sortby} order={order} onSort={onSort} />
      </thead>
      <tbody>
        {shown.map((archive) => (
          <CompactTableRow
            key={archive.arcid}
            archive={archive}
            columns={columns}
            selectedIds={selectedIds}
            multiSelect={multiSelect}
            onToggleSelected={onToggleSelected}
            onOpen={onOpen}
            onContextMenu={onContextMenu}
            onSearchTag={onSearchTag}
          />
        ))}
      </tbody>
    </table>
  )
}
