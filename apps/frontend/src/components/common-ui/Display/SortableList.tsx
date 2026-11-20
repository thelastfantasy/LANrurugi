import { Plugin } from "@dnd-kit/abstract"
import { type DragDropManager, PointerActivationConstraints, PointerSensor, Scroller } from "@dnd-kit/dom"
import { move } from "@dnd-kit/helpers"
import { DragDropProvider } from "@dnd-kit/react"
import { useSortable } from "@dnd-kit/react/sortable"
import Lenis from "lenis"
import { useEffect, useRef, useState } from "react"

/** Props a caller's `renderItem` must attach to the element that starts a drag — the grip, so
 * clicking controls elsewhere in the row doesn't begin one. */
export interface DragHandleProps {
  handleRef: (element: Element | null) => void
  isDragging: boolean
}

/** One sortable row. `@dnd-kit/dom`'s `Feedback` plugin (on by default) lifts the dragged element
 * itself into the top layer — it sets `popover="manual"` and calls `showPopover()` — so it follows
 * the pointer even inside a drawer, whose own `transform`/`overflow: hidden` would clip a plain
 * `position: fixed` overlay. What stays behind in its slot is an empty, `inert` placeholder that
 * merely holds the space, not a second copy of the content. */
function SortableRow<T>({
  id,
  index,
  item,
  renderItem,
}: {
  id: string
  index: number
  item: T
  renderItem: (item: T, dragHandleProps: DragHandleProps) => React.ReactNode
}) {
  // No `feedback: "clone"` and no manual `visibility` here: the element being dragged is this one,
  // so hiding or cloning it either hides the drag or leaves a second visible copy in the slot.
  const { ref, handleRef, isDragging } = useSortable({ id, index })
  return <div ref={ref}>{renderItem(item, { handleRef, isDragging })}</div>
}

/** True when `element` sits inside a fixed-position overlay (a drawer or modal), i.e. a layer whose
 * own scroller is the only thing a drag may move. */
function hasFixedOverlayAncestor(element: Element | null): boolean {
  for (let node = element?.parentElement ?? null; node; node = node.parentElement) {
    if (getComputedStyle(node).position === "fixed") return true
  }
  return false
}

/** Keeps `AutoScroller` out of the page behind a drawer or modal. `Scroller` walks the scrollable
 * ancestors of whatever sits under the pointer and always includes `document.scrollingElement`, so
 * once the drawer's own list has reached the end it is dragged toward, the autoscroll carries on
 * into the page behind it (dragging an item to the top row slides the whole page up). Dropping that
 * one entry while the drag started inside an overlay keeps the drawer's own scrolling intact and
 * leaves the page alone. */
class ConfineAutoScroll extends Plugin<DragDropManager> {
  constructor(manager: DragDropManager) {
    super(manager)
    const scroller = manager.registry.plugins.get(Scroller)
    if (!scroller) return
    const scrollableElements = scroller.getScrollableElements
    scroller.getScrollableElements = () => {
      const elements = scrollableElements()
      const source = manager.dragOperation.source?.element
      const page = document.scrollingElement
      if (!elements || !source || !page || !hasFixedOverlayAncestor(source)) return elements
      if (!elements.has(page)) return elements
      const confined = new Set(elements)
      confined.delete(page)
      return confined
    }
  }
}

/** Drag-and-drop reorderable list, generic over any item type. Order lives in this component's
 * own state and is recomputed with `move()` on every `dragover`, so the siblings visibly give way
 * to the dragged item instead of only reordering once on drop. */
export function SortableList<T>({
  items,
  getId,
  onReorder,
  renderItem,
  direction = "vertical",
  gridMinItemWidth = 120,
  onScroller,
}: {
  items: T[]
  getId: (item: T) => string
  /** Called once after a drop changes the order — receives the complete new ordered id list. */
  onReorder: (newOrder: string[]) => void
  renderItem: (item: T, dragHandleProps: DragHandleProps) => React.ReactNode
  /** `'horizontal'` reorders left/right in a scrolling flex row instead of vertically; `'grid'`
   * wraps items into columns at least `gridMinItemWidth` wide. */
  direction?: "vertical" | "horizontal" | "grid"
  gridMinItemWidth?: number
  /** Exposes the internal Lenis instance (horizontal mode only) so a caller can drive prev/next-
   * arrow buttons the same way `RecentlyAddedCarousel`'s own carousel does — a callback, not a
   * plain `RefObject`, so the caller can mirror it into its own state and actually re-render (a
   * ref write alone wouldn't) when this list's own scroll container is created or torn down. */
  onScroller?: (lenis: Lenis | null) => void
}) {
  const byId = new Map(items.map((item) => [getId(item), item]))
  const ids = items.map(getId)
  const [order, setOrder] = useState(ids)
  // Re-render-time reset (React's adjusting-state pattern), so a caller replacing the whole
  // selection doesn't keep dragging around the previous order for one paint.
  const idsKey = ids.join("\u0000")
  const previousIdsKey = useRef(idsKey)
  if (previousIdsKey.current !== idsKey) {
    previousIdsKey.current = idsKey
    setOrder(ids)
  }
  const orderRef = useRef(order)
  orderRef.current = order

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
    onScroller?.(lenis)
    return () => {
      lenis.destroy()
      lenisRef.current = null
      onScroller?.(null)
    }
  }, [direction, onScroller])

  // Lenis's own `ResizeObserver` watches `wrapper`/`content`'s own border-box, which here is the
  // same fixed-size flex element for both — it never fires just because the item count (and thus
  // `scrollWidth`) changed underneath it. Force a re-measure whenever the actual list changes so
  // e.g. selecting/deselecting archives on the Library page doesn't leave the arrow buttons'
  // `scrollTo` calls clamped to a stale, smaller `limit`.
  useEffect(() => {
    lenisRef.current?.resize()
  }, [items])

  const rows = order.map((id, index) => {
    const item = byId.get(id)
    if (item === undefined) return null
    return <SortableRow key={id} id={id} index={index} item={item} renderItem={renderItem} />
  })

  return (
    <DragDropProvider
      sensors={[
        PointerSensor.configure({
          activationConstraints: [new PointerActivationConstraints.Distance({ value: 4 })],
        }),
      ]}
      plugins={(defaults) => [...defaults, ConfineAutoScroll]}
      onDragStart={() => lenisRef.current?.stop()}
      onDragOver={(event) => {
        setOrder((current) => move(current, event))
      }}
      onDragEnd={(event) => {
        lenisRef.current?.start()
        if (event.canceled) {
          setOrder(ids)
          return
        }
        const next = move(orderRef.current, event)
        setOrder(next)
        if (next.join("\u0000") !== idsKey) onReorder(next)
      }}
    >
      {direction === "grid" ? (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: `repeat(auto-fill, minmax(${gridMinItemWidth}px, 1fr))`,
            gap: 10,
          }}
        >
          {rows}
        </div>
      ) : direction === "horizontal" ? (
        <div
          ref={containerRef}
          className="hide-scrollbar"
          style={{ display: "flex", flexDirection: "row", overflowX: "auto" }}
        >
          {rows}
        </div>
      ) : (
        rows
      )}
    </DragDropProvider>
  )
}
