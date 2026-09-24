import { Feedback } from "@dnd-kit/dom"
import { useSortable } from "@dnd-kit/react/sortable"

import { THUMB_ASPECT_RATIO } from "./shared"

/** 目标行中的固定页面缩略图——参与排序但不可拖拽 */
export function TargetThumbSortable({ id, index, queueItemId, side }: {
  id: string; index: number; queueItemId: string; side: "a" | "b"
}) {
  const { ref } = useSortable({
    id: String(id), group: "target", index, type: "item", accept: ["item"],
    disabled: { draggable: true },
    plugins: [Feedback.configure({ feedback: "clone" })],
  })
  const idx = Number(id.replace("target-", ""))
  return (
    <div ref={ref} data-target-page={idx} style={{ flexShrink: 0, height: "100%", aspectRatio: THUMB_ASPECT_RATIO, borderRadius: 4, overflow: "hidden" }}>
      <img src={`/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${side}&index=${idx}`}
        alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
    </div>
  )
}

/** 目标行中的已放置 DragPage——可拖拽可排序 */
export function TargetDrag({ id, index, queueItemId, side }: {
  id: string; index: number; queueItemId: string; side: "a" | "b"
}) {
  const { ref, isDragging } = useSortable({
    id: String(id), group: "target", index, type: "item", accept: ["item"],
    plugins: [Feedback.configure({ feedback: "clone" })],
  })
  const idx = Number(id.replace("source-", ""))
  return (
    <div ref={ref} style={{
      height: "100%", aspectRatio: THUMB_ASPECT_RATIO, boxSizing: "border-box", borderRadius: 4, overflow: "hidden",
      cursor: "grab", touchAction: "none", border: "2px dashed rgba(255,152,0,0.9)",
      visibility: isDragging ? "hidden" : "visible", flexShrink: 0,
    }}>
      <img src={`/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${side}&index=${idx}`}
        alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
    </div>
  )
}
