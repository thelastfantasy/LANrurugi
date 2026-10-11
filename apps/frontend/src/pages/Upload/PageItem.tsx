import { Feedback } from "@dnd-kit/dom"
import { useSortable } from "@dnd-kit/react/sortable"

import { THUMB_ASPECT_RATIO } from "./shared"

type Role = "source-drag" | "source-ghost" | "source-plain" | "target-thumb" | "target-drag"

export function PageItem({ id, role, group, index, queueItemId, side }: {
  id: string; role: Role; group: string; index: number; queueItemId: string; side: "a" | "b"
}) {
  const idx = Number(id.replace(/^(source|target)-/, ""))
  const src = `/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${side}&index=${idx}`

  const sortDisabled =
    role === "source-plain" ? true :
    role === "source-ghost" ? { draggable: true, droppable: true } :
    role === "target-thumb" ? { draggable: true } :
    false

  const { ref, isDragging } = useSortable({
    id: String(id), group, index, type: "item", accept: ["item"],
    disabled: sortDisabled,
    plugins: (role === "source-drag" || role === "target-drag") ? [Feedback.configure({ feedback: "clone" })] : undefined,
  })

  const isDrag = role === "source-drag" || role === "target-drag"

  return (
    <div ref={ref} style={{
      height: "100%", aspectRatio: THUMB_ASPECT_RATIO, boxSizing: "border-box",
      borderRadius: 4, overflow: "hidden", cursor: isDrag ? "grab" : "default",
      touchAction: isDrag ? "none" : undefined,
      border: isDrag ? "2px dashed rgba(255,152,0,0.9)" : "2px solid transparent",
      visibility: isDrag && isDragging ? "hidden" : "visible",
      opacity: role === "source-ghost" ? 0.4 : 1,
      flexShrink: 0,
    }}>
      <img src={src} alt=""
        style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
    </div>
  )
}
