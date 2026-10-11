import { Feedback } from "@dnd-kit/dom"
import { useSortable } from "@dnd-kit/react/sortable"

import { THUMB_ASPECT_RATIO } from "./shared"

/** 源行中可拖拽的未放置页面 */
export function SourceDrag({ id, index, queueItemId, side }: {
  id: string; index: number; queueItemId: string; side: "a" | "b"
}) {
  const pageIdx = Number(id.replace("source-", ""))
  const { ref, isDragging } = useSortable({
    id: String(id), group: "source", index, type: "item", accept: ["item"],
    plugins: [Feedback.configure({ feedback: "clone" })],
  })
  return (
    <div ref={ref} data-source-page={pageIdx} style={{
      height: "100%", aspectRatio: THUMB_ASPECT_RATIO, boxSizing: "border-box", borderRadius: 4, overflow: "hidden",
      cursor: "grab", touchAction: "none", border: "2px dashed rgba(255,152,0,0.9)",
      visibility: isDragging ? "hidden" : "visible", flexShrink: 0,
    }}>
      <img src={`/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${side}&index=${pageIdx}`}
        alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
    </div>
  )
}

/** 源行中已放置的虚影（不可交互） */
export function SourceGhost({ index, queueItemId, side }: {
  index: number; queueItemId: string; side: "a" | "b"
}) {
  return (
    <div data-source-page={index} style={{ flexShrink: 0, height: "100%", aspectRatio: THUMB_ASPECT_RATIO, borderRadius: 4, overflow: "hidden", opacity: 0.4 }}>
      <img src={`/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${side}&index=${index}`}
        alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
    </div>
  )
}

/** 源行中普通匹配页（不可交互） */
export function SourcePlain({ index, queueItemId, side }: {
  index: number; queueItemId: string; side: "a" | "b"
}) {
  return (
    <div data-source-page={index} style={{ flexShrink: 0, height: "100%", aspectRatio: THUMB_ASPECT_RATIO, borderRadius: 4, overflow: "hidden" }}>
      <img src={`/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${side}&index=${index}`}
        alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
    </div>
  )
}
