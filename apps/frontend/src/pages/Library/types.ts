import type { ArchiveMetadata } from "@/api/types"

export type CarouselMode = "ondeck" | "recent" | "random" | "inbox" | "untagged" | "bookmark"

/** What opening the archive menu needs from the gesture: a right-click's `MouseEvent` or a touch
 * long press's `PointerEvent` (`useLongPress`). */
export type ContextMenuTrigger = Pick<MouseEvent, "clientX" | "clientY" | "preventDefault">

export interface ContextMenuState {
  archive: ArchiveMetadata
  x: number
  y: number
  source: "grid" | "carousel"
}
