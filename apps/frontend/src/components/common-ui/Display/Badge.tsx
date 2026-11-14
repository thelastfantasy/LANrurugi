import type { CSSProperties, ReactNode } from "react"

/**
 * Small pill label for a short marker (`L`/`R`, a count, a state word).
 *
 * The tint is derived from `currentColor` rather than a hardcoded palette, so the badge follows
 * whatever surface and theme it is dropped into — a dark OSD sheet and a light settings row get a
 * readable badge without a per-theme rule.
 */
export function Badge({
  children,
  className,
  style,
  title,
}: {
  children: ReactNode
  className?: string
  style?: CSSProperties
  title?: string
}) {
  return (
    <span
      className={className ? `ui-badge ${className}` : "ui-badge"}
      style={style}
      title={title}
    >
      {children}
    </span>
  )
}
