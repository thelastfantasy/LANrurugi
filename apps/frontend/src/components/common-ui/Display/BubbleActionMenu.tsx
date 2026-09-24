import type { ReactElement, ReactNode } from "react"

import { IconButton } from "@/components/common-ui/Form"

import { Popover } from "./Popover"
import { Tooltip } from "./Tooltip"

/** Horizontal icon-only action bar inside a {@link Popover} bubble, click-triggered from
 * `trigger`. Distinct from `Menu` (vertical, text rows, no arrow) — use this for a compact 2-4
 * icon action set anchored to a small trigger element (e.g. a toggled icon). */
export function BubbleActionMenu({
  trigger,
  children,
  open,
  onOpenChange,
}: {
  trigger: ReactElement
  children: ReactNode
  open?: boolean
  onOpenChange?: (open: boolean) => void
}) {
  return (
    <Popover trigger={trigger} open={open} onOpenChange={onOpenChange}>
      <div style={{ display: "flex", alignItems: "center", gap: 2, padding: 4 }}>{children}</div>
    </Popover>
  )
}

/** One icon-only button inside a {@link BubbleActionMenu} — a hover tooltip supplies the visible
 * label a vertical `MenuItem`'s own text would otherwise carry; `label` also doubles as the
 * button's `aria-label`. Caller's `onClick` decides whether/how the popover closes (e.g. via the
 * `onOpenChange` passed to `BubbleActionMenu` itself). */
export function BubbleActionItem({
  icon,
  label,
  onClick,
}: {
  icon: ReactNode
  label: string
  onClick: () => void
}) {
  return (
    <Tooltip label={label} anchor="cursor">
      <IconButton
        variant="ghost-btn"
        icon={icon}
        aria-label={label}
        size={28}
        style={{ borderRadius: 4 }}
        onClick={onClick}
      />
    </Tooltip>
  )
}
