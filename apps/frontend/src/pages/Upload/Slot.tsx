import { CollisionPriority } from "@dnd-kit/abstract"
import { useDroppable } from "@dnd-kit/react"
import React from "react"

export function Slot({ id, children }: { id: string; children: React.ReactNode }) {
  const { ref } = useDroppable({ id, type: "column", accept: ["item"], collisionPriority: CollisionPriority.Low })
  return <div ref={ref} style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>{children}</div>
}
