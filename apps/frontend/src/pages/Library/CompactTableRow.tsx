import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { ArchiveMetadata } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { isPlainLinkClick,Link } from "@/components/common-ui/Link"
import { FormatBadge, StatusIcon } from "@/components/Display"
import { useLongPress } from "@/hooks/useLongPress"
import { routes } from "@/lib/routes"
import { isBookExtension } from "@/lib/utils/bookFormats"
import { coverUrl } from "@/lib/utils/coverUrl"

import { CustomColumnCell } from "./CustomColumnCell"
import { TagLine } from "./TagLine"
import type { ContextMenuTrigger } from "./types"

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
  onContextMenu: (e: ContextMenuTrigger, archive: ArchiveMetadata) => void
  onSearchTag: (namespacedTag: string) => void
}) {
  const { t } = useTranslation()
  const [rowEl, setRowEl] = useState<HTMLTableRowElement | null>(null)
  useLongPress(rowEl, (_, e) => onContextMenu(e, archive), { blockContextMenu: () => true })
  return (
    <tr
      ref={setRowEl}
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
              src={coverUrl(archive)}
              alt=""
              style={{ height: 300, display: "block" }}
            />
          }
          anchor="cursor"
          wrapperStyle={{ display: "inline" }}
        >
          <Link
            to={routes.reader(archive.arcid)}
            onClick={(e) => {
              // The browser keeps its own link gestures; a click that ended a text selection is
              // not a request to open the archive either.
              if (!isPlainLinkClick(e)) return
              e.preventDefault()
              if (multiSelect) onToggleSelected(archive.arcid)
              else onOpen(archive.arcid)
            }}
          >
            {archive.isnew && (
              <>
                <StatusIcon kind="new" title={t("library.new") ?? undefined} />{" "}
              </>
            )}
            {archive.title}
            {isBookExtension(archive.extension) && (
              <>
                {" "}
                <FormatBadge extension={archive.extension} />
              </>
            )}
          </Link>
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
