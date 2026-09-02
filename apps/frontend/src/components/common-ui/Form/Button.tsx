import { Button as BaseButton } from "@base-ui/react/button"
import { cloneElement, type ComponentProps, Fragment, isValidElement, type ReactNode } from "react"

import { Tooltip } from "@/components/common-ui/Display/Tooltip"

export type ButtonVariant = "stdbtn" | "favtag-btn" | "ghost-btn"

/** Site-wide `.stdbtn`-styled button, built on Base UI's `Button` (a real `<button>`) rather than
 * legacy's `<input type="button">`. `className`, if given, appends onto `variant` rather than replacing it.
 * `"ghost-btn"` is transparent/borderless at rest — no legacy equivalent, for a square icon-only
 * button inside an already-chromed container (e.g. a popover bubble) where `.stdbtn`'s own border/
 * background would double up against the container's. */
export function Button({
  variant = "stdbtn",
  className,
  ...props
}: { variant?: ButtonVariant; className?: string } & Omit<ComponentProps<typeof BaseButton>, "className">) {
  return <BaseButton className={className ? `${variant} ${className}` : variant} {...props} />
}

/** `"medium"` (32×21, default), `"small"` (24×18, dense rows), `"large"` (40×26). A one-off size
 * passes a plain number (px) or any CSS length string (e.g. `"1.5em"`, `"2rem"`) instead. Same
 * vocabulary as the removed `Display/IconButton.tsx`, plus CSS-unit strings. */
const ICON_BUTTON_SIZES = {
  small: { width: 24, height: 18 },
  medium: { width: 32, height: 21 },
  large: { width: 40, height: 26 },
} as const

type IconButtonSizePreset = keyof typeof ICON_BUTTON_SIZES

export type IconButtonSize = IconButtonSizePreset | number | (string & {})

function resolveIconButtonSize(size: IconButtonSize): { width: number | string; height: number | string } {
  if (typeof size === "string" && size in ICON_BUTTON_SIZES) return ICON_BUTTON_SIZES[size as IconButtonSizePreset]
  if (typeof size === "string") return { width: size, height: size }
  return { width: size, height: size }
}

/** Renders a plain FA class string as `<i>`, or merges `aria-hidden` onto a caller-built element.
 * A `Fragment` (e.g. an icon plus adjacent text) only accepts `key`/`children`, so it's passed
 * through unchanged — the caller is responsible for `aria-hidden` on its own inner elements. */
function renderIcon(icon: string | ReactNode): ReactNode {
  if (typeof icon === "string") return <i className={icon} aria-hidden="true"></i>
  if (isValidElement(icon) && icon.type !== Fragment) return cloneElement(icon, { "aria-hidden": "true" } as Record<string, unknown>)
  return icon
}

/** `variant` alone picks one of `Button`'s known classes; `className` alone (no `variant`) is used
 * verbatim, matching a caller that writes its own full class list (e.g. `"stdbtn stdbtn-danger"` or
 * a class with no `ButtonVariant` equivalent like `"modal-close-btn"`). Both together append, same
 * as `Button` itself. Neither given falls back to `"stdbtn"`. */
function resolveIconButtonClassName(variant: ButtonVariant | undefined, className: string | undefined): string {
  if (variant) return className ? `${variant} ${className}` : variant
  return className ?? "stdbtn"
}

/** Chakra-UI-style `IconButton` — defaults to a true 26px square, not the `"medium"` preset
 * (32×21, non-square — for callers matching a neighboring `.stdbtn` text button's own height). */
export function IconButton({
  variant,
  className,
  icon,
  size = 26,
  style,
  title,
  "aria-label": ariaLabel,
  ...props
}: {
  variant?: ButtonVariant
  className?: string
  icon: string | ReactNode
  size?: IconButtonSize
  style?: React.CSSProperties
  title?: string
  "aria-label"?: string
} & Omit<ComponentProps<typeof BaseButton>, "className" | "children">) {
  const { width, height } = resolveIconButtonSize(size)
  return (
    <BaseButton
      className={resolveIconButtonClassName(variant, className)}
      title={title}
      aria-label={ariaLabel ?? title}
      style={{ width, height, minWidth: width, margin: 0, padding: 0, display: "inline-flex", alignItems: "center", justifyContent: "center", ...style }}
      {...props}
    >
      {renderIcon(icon)}
    </BaseButton>
  )
}

/** `IconButton` pre-wrapped in a `Tooltip` showing a bold title + description. */
export function IconButtonWithTooltip({
  variant,
  className,
  icon,
  title,
  description,
  size = "medium",
  style,
  anchor = "cursor",
  wrapperStyle,
  ...props
}: {
  variant?: ButtonVariant
  className?: string
  icon: string | ReactNode
  title: ReactNode
  description: ReactNode
  size?: IconButtonSize
  style?: React.CSSProperties
  anchor?: "cursor" | "element"
  /** Forwarded to `Tooltip`'s `wrapperStyle` — its hover-trigger span defaults to
   * `alignItems: stretch`, overriding a caller's outer `alignItems: center`. */
  wrapperStyle?: React.CSSProperties
} & Omit<ComponentProps<typeof BaseButton>, "className" | "children" | "title">) {
  return (
    <Tooltip
      label={
        <div>
          <div style={{ fontWeight: 600, marginBottom: 4 }}>{title}</div>
          <div style={{ opacity: 0.8 }}>{description}</div>
        </div>
      }
      anchor={anchor}
      wrapperStyle={wrapperStyle}
    >
      <IconButton variant={variant} className={className} icon={icon} size={size} style={style} aria-label={typeof title === "string" ? title : undefined} {...props} />
    </Tooltip>
  )
}
