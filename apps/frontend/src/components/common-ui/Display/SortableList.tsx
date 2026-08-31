import {
  closestCenter,
  DndContext,
  type DragEndEvent,
  DragOverlay,
  type DragStartEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core"
import {
  arrayMove,
  horizontalListSortingStrategy,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable"
import { CSS } from "@dnd-kit/utilities"
import Lenis from "lenis"
import { useEffect, useRef, useState } from "react"

/** Props a caller's `renderItem` must spread onto the grab handle so clicking interactive
 * controls elsewhere in the row doesn't fight the pointer sensor. Absent for `DragOverlay`. */
export interface DragHandleProps {
  attributes?: ReturnType<typeof useSortable>["attributes"]
  listeners?: ReturnType<typeof useSortable>["listeners"]
  isDragging: boolean
}

/** One sortable row. While dragging, this row becomes an invisible placeholder — the real
 * dragged content renders inside `SortableList`'s own `DragOverlay` instead. */
function SortableRow<T>({
  id,
  item,
  renderItem,
}: {
  id: string
  item: T
  renderItem: (item: T, dragHandleProps: DragHandleProps) => React.ReactNode
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id })
  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    // Not display: none, which would collapse layout and jump everything below.
    visibility: isDragging ? "hidden" : "visible",
  }

  return (
    <div ref={setNodeRef} style={style}>
      {renderItem(item, { attributes, listeners, isDragging })}
    </div>
  )
}

/** Drag-and-drop reorderable list (dnd-kit), generic over any item type. Uses `DragOverlay` for
 * the dragged row's real content, since rows of differing heights otherwise visually overlap. */
export function SortableList<T>({
  items,
  getId,
  onReorder,
  renderItem,
  direction = "vertical",
}: {
  items: T[]
  getId: (item: T) => string
  /** Called once after a drop changes the order — receives the complete new ordered id list. */
  onReorder: (newOrder: string[]) => void
  renderItem: (item: T, dragHandleProps: DragHandleProps) => React.ReactNode
  /** `'horizontal'` reorders left/right in a scrolling flex row instead of vertically. */
  direction?: "vertical" | "horizontal"
}) {
  const [activeId, setActiveId] = useState<string | null>(null)
  const byId = new Map(items.map((item) => [getId(item), item]))
  const ids = items.map(getId)
  const containerRef = useRef<HTMLDivElement>(null)
  const lenisRef = useRef<Lenis | null>(null)

  // Horizontal mode: Lenis handles wheel-to-horizontal with damp.
  useEffect(() => {
    const el = containerRef.current
    if (!el || direction !== "horizontal") return
    const lenis = new Lenis({
      wrapper: el,
      content: el,
      orientation: "horizontal",
      gestureOrientation: "both",
      wheelMultiplier: 4.5,
      lerp: 0.1,
      autoRaf: true,
    })
    lenisRef.current = lenis
    return () => { lenis.destroy(); lenisRef.current = null }
  }, [direction])

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  )

  function handleDragStart(event: DragStartEvent) {
    setActiveId(String(event.active.id))
    lenisRef.current?.stop()
  }

  function handleDragEnd(event: DragEndEvent) {
    setActiveId(null)
    lenisRef.current?.start()
    const { active, over } = event
    if (!over || active.id === over.id) return
    const oldIndex = ids.indexOf(String(active.id))
    const newIndex = ids.indexOf(String(over.id))
    onReorder(arrayMove(ids, oldIndex, newIndex))
  }

  const activeItem = activeId ? byId.get(activeId) : undefined
  const items_ = (
    <>
      {items.map((item) => {
        const id = getId(item)
        return <SortableRow key={id} id={id} item={item} renderItem={renderItem} />
      })}
    </>
  )

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragStart={handleDragStart}
      onDragEnd={handleDragEnd}
      onDragCancel={() => { setActiveId(null); lenisRef.current?.start() }}
    >
      <SortableContext
        items={ids}
        strategy={direction === "horizontal" ? horizontalListSortingStrategy : verticalListSortingStrategy}
      >
        {direction === "horizontal" ? (
          <div ref={containerRef} className="hide-scrollbar" style={{ display: "flex", flexDirection: "row", overflowX: "auto" }}>
            {items_}
          </div>
        ) : (
          items_
        )}
      </SortableContext>
      <DragOverlay>{activeItem && renderItem(activeItem, { isDragging: true })}</DragOverlay>
    </DndContext>
  )
}
