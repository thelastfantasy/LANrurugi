
import { useArchiveMetadata } from "@/api/hooks"
import type { ArchiveMetadata } from "@/api/types"

import { CarouselCard } from "./CarouselCard"
import type { ContextMenuTrigger } from "./types"

export function SelectedArchiveSlideContent({
  id,
  cropThumbs,
  onContextMenu,
  onRemove,
}: {
  id: string
  cropThumbs: boolean
  onContextMenu: (e: ContextMenuTrigger, archive: ArchiveMetadata) => void
  onRemove: (id: string) => void
}) {
  const metadata = useArchiveMetadata(id)
  if (!metadata.data) return null
  return (
    <CarouselCard
      archive={metadata.data}
      cropThumbs={cropThumbs}
      onContextMenu={onContextMenu}
      onOpen={() => onRemove(id)}
    />
  )
}
