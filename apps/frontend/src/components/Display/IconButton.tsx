import { cloneElement, isValidElement, type ReactNode } from "react"

import { Tooltip } from "./Tooltip"

/** Built-in presets — `"medium"` (32×21, legacy's own `.stdbtn`/`.searchbtn` icon-only box, the
 * default; e.g. `TankoubonEdit.tsx`'s AI Smart Rename button and the Library page's AI Smart
 * Create Tankoubon button) and `"small"` (24×18, for dense rows where medium's 21px height
 * doesn't fit — e.g. the per-chapter AI Chapter Name button). `"large"` (40×26) has no real call
 * site yet but rounds out the usual small/medium/large scale for whatever needs a bigger target
 * next. A caller needing a one-off size outside all three passes a plain number (px, applied to
 * both width and height) instead, via `IconButtonSize`. */
const PRESET_SIZES = {
  small: { width: 24, height: 18 },
  medium: { width: 32, height: 21 },
  large: { width: 40, height: 26 },
} as const

type IconButtonSizePreset = keyof typeof PRESET_SIZES

export type IconButtonSize = IconButtonSizePreset | number

function resolveSize(size: IconButtonSize): { width: number; height: number } {
  return typeof size === "number" ? { width: size, height: size } : PRESET_SIZES[size]
}

/** Renders `icon` as `<i className={icon} aria-hidden="true" />` when it's a plain FA class
 * string. When it's already an element (a caller's own `<i>`/SVG/emoji-wrapping element),
 * `cloneElement` merges `aria-hidden="true"` onto it — preserving whatever `style`/`className`/
 * other props that element already carries — rather than either dropping those props on the
 * floor or forcing the caller to remember to set `aria-hidden` themselves on every custom icon. */
function renderIcon(icon: string | ReactNode): ReactNode {
  if (typeof icon === "string") return <i className={icon} aria-hidden="true"></i>
  if (isValidElement(icon)) return cloneElement(icon, { "aria-hidden": "true" } as Record<string, unknown>)
  return icon
}

/** Small square icon-only button — fixed sizing (see `IconButtonSize`) so it always reads as a
 * compact icon control and never stretches into a normal text-width `stdbtn`/`searchbtn`, no
 * matter which of those two classes (or a caller's own) is passed in. `icon` is either a full
 * Font Awesome class string (e.g. `"fa fa-robot"`, `"fas fa-trash"`) — rendered as `<i
 * className={icon} />`, since callers mix the `fa`/`fas` prefix and any sizing modifier (`fa-2x`
 * etc.) themselves — or, for the rare case that needs something Font Awesome doesn't have (an
 * emoji, a custom SVG), an element of the caller's own. In the element case, `aria-hidden="true"`
 * is merged on via `cloneElement` (see `renderIcon`) so the caller's own `style`/`className`
 * survive untouched instead of being discarded by a naive string-only render path.
 *
 * `title`, if given, is a plain native `title` attribute (browser's own hover tooltip) — for a
 * one-word affordance (e.g. "Remove") that doesn't need the richer bold-title-plus-description
 * treatment `IconButtonWithTooltip` renders. Use that instead when the hint needs more than a
 * single line. */
export function IconButton({
  icon,
  onClick,
  disabled,
  className = "stdbtn",
  size = "medium",
  style,
  title,
}: {
  icon: string | ReactNode
  onClick: () => void
  disabled?: boolean
  className?: string
  size?: IconButtonSize
  style?: React.CSSProperties
  title?: string
}) {
  const { width, height } = resolveSize(size)
  return (
    <button
      type="button"
      className={className}
      style={{ width, height, minWidth: width, padding: 0, ...style }}
      disabled={disabled}
      onClick={onClick}
      title={title}
    >
      {renderIcon(icon)}
    </button>
  )
}

/** `IconButton` pre-wrapped in a `Tooltip` showing a bold title + description — the pattern every
 * real call site (`TankoubonEdit.tsx`'s AI Smart Rename/AI Chapter Name buttons, the Library
 * page's AI Smart Create Tankoubon button) actually needs, so callers don't hand-assemble the
 * same `<Tooltip><IconButton/></Tooltip>` + two-line label markup themselves each time. */
export function IconButtonWithTooltip({
  icon,
  title,
  description,
  onClick,
  disabled,
  className = "stdbtn",
  size = "medium",
  style,
  anchor = "cursor",
}: {
  icon: string | ReactNode
  title: ReactNode
  description: ReactNode
  onClick: () => void
  disabled?: boolean
  className?: string
  size?: IconButtonSize
  style?: React.CSSProperties
  anchor?: "cursor" | "element"
}) {
  return (
    <Tooltip
      label={
        <div>
          <div style={{ fontWeight: 600, marginBottom: 4 }}>{title}</div>
          <div style={{ opacity: 0.8 }}>{description}</div>
        </div>
      }
      anchor={anchor}
    >
      <IconButton icon={icon} onClick={onClick} disabled={disabled} className={className} size={size} style={style} />
    </Tooltip>
  )
}
