import type { MouseEvent } from "react"

import type { ArchiveMetadata } from "@/api/types"

import { ArchiveCard } from "./ArchiveCard"

export function CarouselCard({
  archive,
  cropThumbs,
  onContextMenu,
  onOpen,
  onSearchTag,
}: {
  archive: ArchiveMetadata
  cropThumbs: boolean
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  onSearchTag?: (namespacedTag: string) => void
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
    />
  )
}
