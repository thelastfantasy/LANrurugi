import { type ComponentProps, useLayoutEffect, useRef, useState } from "react"

import { Menu, type MenuSide } from "@/components/common-ui/Display/Menu"
import { IconButton, type IconButtonSize } from "@/components/common-ui/Form/Button"

/** Base rotation (degrees) that points the arrow at each side, before the open-state flip below. */
const ARROW_ROTATION_FOR_SIDE: Record<MenuSide, number> = {
  bottom: 0,
  top: 180,
  left: 90,
  right: -90,
  "inline-end": -90,
  "inline-start": 90,
}

/**
 * An icon button paired with a small arrow that opens a {@link Menu} — for a control whose main
 * action is a single click/tap, but that also has a secondary set of related choices (e.g.
 * translation on/off plus which language to translate into, or a plain action button plus a menu
 * of variants). Distinct from a traditional "split button" (where both halves trigger variants of
 * the same action): here the right half's only job is opening the menu, and the left half is
 * whatever the caller wants — a toggle (pass `active` to dim it when off) is one common shape, a
 * plain momentary action button is another (omit `active` entirely).
 *
 * The arrow points the direction the popup actually opened — not just the requested `menuSide`,
 * which Base UI's collision detection can override (e.g. a bottom toolbar has no room to open
 * downward, so a `menuSide="bottom"` request still opens upward in practice; see `Menu`'s own
 * `onSideChange` for how the real side is learned). While open, the arrow additionally flips 180°
 * from that resting angle — the conventional "caret now points back at its own trigger" cue that
 * distinguishes "closed, will open this way" from "open now, click to close" — animated via the
 * same transform rather than an instant jump.
 */
export function ToggleButtonMenu({
  icon,
  title,
  active,
  onClick,
  menuTrigger,
  menuTitle,
  menuChildren,
  menuSide = "bottom",
  menuAlign = "start",
  size = 32,
  style,
}: {
  /** FA icon class for the left half, e.g. `"fas fa-language fa-2x"`. */
  icon: string
  title?: string
  /** Omit for a plain action button; pass the current on/off state to dim the icon when inactive. */
  active?: boolean
  onClick: () => void
  /** aria-label for the arrow trigger — the menu's own purpose, e.g. "choose translation language". */
  menuTrigger: string
  /** Optional label rendered as a disabled first row inside the popup (matches other menus'
   * section-heading convention rather than relying on `menuTrigger`'s aria-only text). */
  menuTitle?: string
  menuChildren: ComponentProps<typeof Menu>["children"]
  /** Requested side — a hint, not a guarantee; see this component's own docs for why the arrow
   * doesn't just render this value directly. */
  menuSide?: ComponentProps<typeof Menu>["side"]
  menuAlign?: ComponentProps<typeof Menu>["align"]
  size?: IconButtonSize
  style?: React.CSSProperties
}) {
  // Seeded from the requested side, then corrected below (before paint) once this component's own
  // position in the viewport is knowable — a trigger already sitting near the bottom of the screen
  // should point up from its very first render, not just after the user opens it once and Base
  // UI's own collision detection corrects it via `onSideChange`.
  const [actualSide, setActualSide] = useState<MenuSide>((menuSide as MenuSide) ?? "bottom")
  const [open, setOpen] = useState(false)
  const arrowRef = useRef<HTMLButtonElement>(null)

  useLayoutEffect(() => {
    const estimate = () => {
      const el = arrowRef.current
      if (!el) return
      // A lightweight guess, not a re-implementation of Base UI's real collision math (which also
      // accounts for the popup's own measured height) — close enough to pick the right resting
      // direction before the first open, and `onSideChange` overwrites this with the real answer
      // the moment Base UI actually computes one.
      const rect = el.getBoundingClientRect()
      const viewportH = window.innerHeight
      setActualSide((current) => {
        // Only override the horizontal-adjacent guesses when the caller actually requested a
        // vertical side — a `menuSide="left"`/`"right"` request means the caller already knows
        // better than a top/bottom viewport heuristic can.
        if (current !== "top" && current !== "bottom") return current
        return rect.top > viewportH / 2 ? "top" : "bottom"
      })
    }
    estimate()
    window.addEventListener("resize", estimate)
    return () => window.removeEventListener("resize", estimate)
  }, [])

  const restingRotation = ARROW_ROTATION_FOR_SIDE[actualSide] ?? 0
  const rotation = open ? restingRotation + 180 : restingRotation

  return (
    <div style={{ display: "inline-flex", alignItems: "center", ...style }}>
      <IconButton
        variant="ghost-btn"
        icon={icon}
        title={title}
        size={size}
        style={{ borderRadius: "50%", opacity: active === false ? 0.55 : 1 }}
        onClick={onClick}
      />
      <Menu
        side={menuSide}
        align={menuAlign}
        onOpenChange={setOpen}
        onSideChange={setActualSide}
        trigger={
          <IconButton
            ref={arrowRef}
            variant="ghost-btn"
            icon={
              <i
                className="fas fa-chevron-down"
                style={{ fontSize: "0.6em", transform: `rotate(${rotation}deg)`, transition: "transform 0.15s ease" }}
              />
            }
            title={menuTrigger}
            aria-label={menuTrigger}
            size={16}
            style={{ borderRadius: "50%", marginLeft: 2 }}
          />
        }
      >
        {menuTitle && (
          <div style={{ padding: "0.3em 1em", opacity: 0.65, fontSize: "0.85em", whiteSpace: "nowrap" }}>{menuTitle}</div>
        )}
        {menuChildren}
      </Menu>
    </div>
  )
}
