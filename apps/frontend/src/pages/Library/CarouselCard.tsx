
import type { ArchiveMetadata } from "@/api/types"

import { ArchiveCard } from "./ArchiveCard"
import type { ContextMenuTrigger } from "./types"

export function CarouselCard({
  archive,
  cropThumbs,
  onContextMenu,
  onOpen,
  onSearchTag,
  highlightQuery,
}: {
  archive: ArchiveMetadata
  cropThumbs: boolean
  onContextMenu: (e: ContextMenuTrigger, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  onSearchTag?: (namespacedTag: string) => void
  highlightQuery?: string
}) {
  return (
    <ArchiveCard
      archive={archive}
      multiSelect={false}
      selected={false}
      cropThumbs={cropThumbs}
      onToggleSelect={() => {}}
      onContextMenu={onContextMenu}
      onOpen={onOpen}
      onSearchTag={onSearchTag ?? (() => {})}
      highlightQuery={highlightQuery}
    />
  )
}
