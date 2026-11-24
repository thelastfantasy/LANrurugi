import { type KeyboardEvent, type MouseEvent, type ReactNode, useRef } from "react"

/** One tab. `label` is a node so an icon-only tab can pass its own accessible `ariaLabel`. */
export interface TabItem {
  key: string
  label: ReactNode
  disabled?: boolean
  ariaLabel?: string
}

/** Site-wide tab strip.
 *
 *  The active/inactive palette is the download queue's own `.queue-tab` pair (reused verbatim: the
 *  theme's `.stdbtn` colours, `opacity: 0.6` at rest, full opacity when active/hovered), plus a 3px
 *  `currentColor` bottom border for the selected tab. Height matches `.stdbtn` at 32px so it sits on
 *  the same line as every other control. */
export function Tabs({
  items,
  activeKey,
  onChange,
  ariaLabel,
  className,
  onClick,
}: {
  items: TabItem[]
  activeKey: string
  onChange: (key: string) => void
  /** Accessible name for the tab list. */
  ariaLabel?: string
  /** Container classes; callers use them for their own width/stretch rules. */
  className?: string
  /** Lets a caller stop a click from also triggering an enclosing row's handler. */
  onClick?: (event: MouseEvent<HTMLDivElement>) => void
}) {
  const refs = useRef<Record<string, HTMLButtonElement | null>>({})

  const move = (direction: 1 | -1) => {
    const enabled = items.filter((item) => !item.disabled)
    if (enabled.length < 2) return
    const current = enabled.findIndex((item) => item.key === activeKey)
    const next = enabled[(current + direction + enabled.length) % enabled.length]
    onChange(next.key)
    refs.current[next.key]?.focus()
  }

  const onKeyDown = (event: KeyboardEvent<HTMLButtonElement>) => {
    if (event.key === "ArrowRight") {
      event.preventDefault()
      move(1)
    } else if (event.key === "ArrowLeft") {
      event.preventDefault()
      move(-1)
    }
  }

  return (
    <div
      className={className ? `common-tabs ${className}` : "common-tabs"}
      role="tablist"
      aria-label={ariaLabel}
      onClick={onClick}
    >
      {items.map((item) => {
        const active = item.key === activeKey
        return (
          <button
            key={item.key}
            ref={(element) => {
              refs.current[item.key] = element
            }}
            type="button"
            role="tab"
            aria-selected={active}
            aria-label={item.ariaLabel}
            disabled={item.disabled}
            tabIndex={active ? 0 : -1}
            className={`stdbtn queue-tab common-tab${active ? " queue-tab-active common-tab--active" : ""}`}
            onClick={() => onChange(item.key)}
            onKeyDown={onKeyDown}
          >
            {item.label}
          </button>
        )
      })}
    </div>
  )
}

Tabs.displayName = "Tabs"
