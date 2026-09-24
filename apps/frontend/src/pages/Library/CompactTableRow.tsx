import type { MouseEvent } from "react"

import type { ArchiveMetadata } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { routes } from "@/lib/routes"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"

import { CustomColumnCell } from "./CustomColumnCell"
import { TagLine } from "./TagLine"

export function CompactTableRow({
  archive,
  columns,
  selectedIds,
  multiSelect,
  onToggleSelected,
  onOpen,
  onContextMenu,
  onSearchTag,
}: {
  archive: ArchiveMetadata
  columns: number
  selectedIds: string[]
  multiSelect: boolean
  onToggleSelected: (id: string) => void
  onOpen: (id: string) => void
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onSearchTag: (namespacedTag: string) => void
}) {
  return (
    <tr
      key={archive.arcid}
      className={selectedIds.includes(archive.arcid) ? "msm-selected" : undefined}
      onContextMenu={(e) => onContextMenu(e, archive)}
    >
      <td className="itd title" style={{ textAlign: "left" }}>
        {multiSelect && (
          <input
            type="checkbox"
            checked={selectedIds.includes(archive.arcid)}
            onChange={() => onToggleSelected(archive.arcid)}
            style={{ marginRight: 6 }}
          />
        )}
        <Tooltip
          label={
            <img
              src={
                isTankoubonId(archive.arcid)
                  ? `/api/tankoubons/${archive.arcid}/thumbnail?no_fallback=true`
                  : `/api/archives/${archive.arcid}/thumbnail?no_fallback=true`
              }
              alt=""
              style={{ height: 300, display: "block" }}
            />
          }
          anchor="cursor"
          wrapperStyle={{ display: "inline" }}
        >
          <a
            href={routes.reader(archive.arcid)}
            onClick={(e) => {
              e.preventDefault()
              if (multiSelect) onToggleSelected(archive.arcid)
              else onOpen(archive.arcid)
            }}
          >
            {archive.isnew && "🆕 "}
            {archive.title}
          </a>
        </Tooltip>
      </td>
      {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
        <CustomColumnCell key={i} index={i} tags={archive.tags} onSearchTag={onSearchTag} />
      ))}
      <td className="itd tags" style={{ textAlign: "left" }}>
        <TagLine tags={archive.tags} onSearchTag={onSearchTag} />
      </td>
    </tr>
  )
}
