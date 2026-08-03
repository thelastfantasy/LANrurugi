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
} from '@dnd-kit/core'
import {
  arrayMove,
  horizontalListSortingStrategy,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable'
import { CSS } from '@dnd-kit/utilities'
import { useState } from 'react'

/** Props a caller's `renderItem` must spread onto whatever element should act as the grab handle
 * (`{...dragHandleProps.attributes} {...dragHandleProps.listeners}`) — kept distinct from the
 * whole row so clicking a row's own interactive controls (checkboxes, buttons, inputs) doesn't
 * fight the pointer sensor's activation constraint. `attributes`/`listeners` are absent for
 * `DragOverlay`'s render of `renderItem` — that copy is a purely visual stand-in, so it needs no
 * working drag listeners of its own. */
export interface DragHandleProps {
  attributes?: ReturnType<typeof useSortable>['attributes']
  listeners?: ReturnType<typeof useSortable>['listeners']
  isDragging: boolean
}

/** One sortable row — renders its own `renderItem` output, wired to dnd-kit's per-item position
 * tracking. While dragging, this row becomes an invisible placeholder rather than moving in
 * place: the real, fully-styled dragged content renders once, separately, inside `SortableList`'s
 * own `DragOverlay` — see that component's docs for why a same-flow-moving item caused visual
 * overlap against neighboring rows of a different height. */
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
    // The dragged row's own in-flow slot becomes invisible (not `display: none`, which would
    // collapse layout and cause everything below to jump) while `DragOverlay` shows the real,
    // undistorted content elsewhere — see this function's own docs.
    visibility: isDragging ? 'hidden' : 'visible',
  }

  return (
    <div ref={setNodeRef} style={style}>
      {renderItem(item, { attributes, listeners, isDragging })}
    </div>
  )
}

/** Drag-and-drop reorderable list (dnd-kit), generic over any item type — extracted from the
 * Plugins page's own per-type priority reordering so any future list needing "drag a row, persist
 * the new order" doesn't need to re-wire dnd-kit from scratch.
 *
 * Uses `DragOverlay` for the actively-dragged row's real rendered content rather than each row
 * moving in its own document-flow slot — necessary because rows can have very different heights
 * (e.g. a plugin card with a multi-line notice vs. a bare one-liner); without it, a tall row
 * dragged over a shorter one visually overlapped the shorter row's content (a real, observed bug).
 *
 * `items`/`getId`/`renderItem` mirror a typical virtualized-list API: `renderItem` receives the
 * item plus `DragHandleProps` to spread onto whichever element the caller wants as the grab
 * handle, so callers keep full control over their own row layout. */
export default function SortableList<T>({
  items,
  getId,
  onReorder,
  renderItem,
  direction = 'vertical',
}: {
  items: T[]
  getId: (item: T) => string
  /** Called once, after a drop actually changes the order — receives the complete new ordered id
   * list (not a single from/to delta), matching how most "persist this order" APIs expect a full
   * replacement rather than an incremental patch. */
  onReorder: (newOrder: string[]) => void
  renderItem: (item: T, dragHandleProps: DragHandleProps) => React.ReactNode
  /** `'vertical'` (default) relies on each row's own block-level element stacking normally — no
   * wrapping flex container needed, matching the Plugins page's original usage. `'horizontal'`
   * additionally wraps the rows in a `display: flex` row with `overflow-x: auto` (scrolls rather
   * than wraps when there isn't room, matching a card-row/carousel-style list) and switches
   * dnd-kit's own sorting strategy to match — items only reorder left/right, not vertically. */
  direction?: 'vertical' | 'horizontal'
}) {
  const [activeId, setActiveId] = useState<string | null>(null)
  const byId = new Map(items.map((item) => [getId(item), item]))
  const ids = items.map(getId)

  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  )

  function handleDragStart(event: DragStartEvent) {
    setActiveId(String(event.active.id))
  }

  function handleDragEnd(event: DragEndEvent) {
    setActiveId(null)
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
      onDragCancel={() => setActiveId(null)}
    >
      <SortableContext
        items={ids}
        strategy={direction === 'horizontal' ? horizontalListSortingStrategy : verticalListSortingStrategy}
      >
        {direction === 'horizontal' ? (
          <div style={{ display: 'flex', flexDirection: 'row', overflowX: 'auto' }}>{items_}</div>
        ) : (
          items_
        )}
      </SortableContext>
      <DragOverlay>{activeItem && renderItem(activeItem, { isDragging: true })}</DragOverlay>
    </DndContext>
  )
}
