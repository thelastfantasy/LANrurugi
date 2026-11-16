import { type ReactNode, useState } from "react"

/** Which subscription groups are folded away in a grouped list; all open by default. */
export function useCollapsedGroups() {
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const toggle = (id: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  return { isCollapsed: (id: string) => collapsed.has(id), toggle }
}

/** One subscription's heading row inside a subscription list's grid: a +/- that folds the group,
 *  an optional control (the approval list's select-all), and the name with its count. The whole row
 *  toggles, like the settings accordions; the control's own clicks stay with the control. */
export function SubscriptionGroupHead({
  name,
  count,
  collapsed,
  onToggle,
  control,
}: {
  name: string
  count: number
  collapsed: boolean
  onToggle: () => void
  control?: ReactNode
}) {
  return (
    <div
      className="preview-group-head"
      role="button"
      tabIndex={0}
      aria-expanded={!collapsed}
      onClick={onToggle}
      onKeyDown={(e) => {
        if (e.key !== "Enter" && e.key !== " ") return
        e.preventDefault()
        onToggle()
      }}
    >
      <i className={`fa ${collapsed ? "fa-plus" : "fa-minus"} preview-group-toggle`} aria-hidden="true" />
      {control && (
        <span
          className="history-title-state pending-check"
          onClick={(e) => e.stopPropagation()}
          onKeyDown={(e) => e.stopPropagation()}
        >
          {control}
        </span>
      )}
      <span className="preview-group-name">
        {name} ({count})
      </span>
    </div>
  )
}
