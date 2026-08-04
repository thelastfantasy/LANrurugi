import type { ArchiveMetadata } from "@/api/types"

export type CarouselMode = "ondeck" | "random" | "inbox" | "untagged"

export interface ContextMenuState {
  archive: ArchiveMetadata
  x: number
  y: number
  source: "grid" | "carousel"
}
