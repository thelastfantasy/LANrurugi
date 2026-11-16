import { type ReactNode, useEffect, useRef, useState } from "react"

/** A scrolling list that only mounts the rows in view (plus a margin), for lists of thousands of
 *  one-line rows, where mounting every row made the page stall. Rows must share one fixed height.
 *  `onEndReached` fires when the last rows come into view, for loading the next page. */
export function WindowedList<T>({
  items,
  rowHeight,
  itemKey,
  renderRow,
  onEndReached,
  className,
  style,
}: {
  items: readonly T[]
  rowHeight: number
  itemKey: (item: T) => string
  renderRow: (item: T) => ReactNode
  onEndReached?: () => void
  className?: string
  style?: React.CSSProperties
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [scrollTop, setScrollTop] = useState(0)
  const [height, setHeight] = useState(0)

  useEffect(() => {
    const el = ref.current
    if (!el) return
    const observer = new ResizeObserver(() => setHeight(el.clientHeight))
    observer.observe(el)
    setHeight(el.clientHeight)
    return () => observer.disconnect()
  }, [])

  const overscan = 10
  const first = Math.max(0, Math.floor(scrollTop / rowHeight) - overscan)
  const last = Math.min(items.length, Math.ceil((scrollTop + height) / rowHeight) + overscan)

  // Near the end of what is loaded: ask for more. Also fires when the first page does not fill
  // the box, so a short first page still leads to the next one.
  const nearEnd = items.length > 0 && last >= items.length - overscan
  useEffect(() => {
    if (nearEnd) onEndReached?.()
  }, [nearEnd, items.length, onEndReached])

  return (
    <div
      ref={ref}
      className={className}
      style={{ overflowY: "auto", position: "relative", ...style }}
      onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
    >
      <div style={{ height: items.length * rowHeight, position: "relative" }}>
        {items.slice(first, last).map((item, i) => (
          <div
            key={itemKey(item)}
            style={{ position: "absolute", top: (first + i) * rowHeight, left: 0, right: 0, height: rowHeight }}
          >
            {renderRow(item)}
          </div>
        ))}
      </div>
    </div>
  )
}
