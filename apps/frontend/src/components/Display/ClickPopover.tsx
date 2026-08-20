import { Popover } from "@base-ui/react/popover"
import type { ReactElement } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, FONT_SIZE_MD, Z_OVERLAY_TOOLTIP } from "@/theme"

/** Click-to-open/close popover — the "?" help-bubble pattern (long, multi-line explanatory
 * content the user deliberately opens and reads, rather than something that flashes past on
 * hover). Built on Base UI's own `Popover` (same primitive family as `ActivityCombobox.tsx`'s
 * `Combobox`-based shells) rather than a hand-rolled `getBoundingClientRect` positioning loop —
 * `Popover.Positioner`'s anchor-positioning (viewport-aware side flipping, `sideOffset`) and
 * `Popover.Trigger`'s built-in click-to-toggle/outside-click/Escape handling cover exactly this
 * component's needs for free. `Tooltip.tsx` stays a separate, hand-rolled component (hover/focus
 * lifecycle, cursor-follow mode) rather than being ported onto the same primitive — a hover
 * tooltip that also responds to clicks would leave callers to reason about which of two different
 * open/close lifecycles applies to their case.
 *
 * `trigger` renders as the popover's own anchor via Base UI's `render` prop — pass any single
 * element (a button, an icon) and Base UI clones its own trigger behavior (onClick/aria-*) onto
 * it, so a caller can nest this directly inside a positioned wrapper (e.g. an `<input>`'s own
 * `position: relative` container, MUI/Chakra `InputAdornment`-style) instead of it always
 * rendering as a freestanding sibling element. */
export function ClickPopover({
  label,
  trigger,
  maxWidth = 320,
  zIndex = Z_OVERLAY_TOOLTIP,
}: {
  label: React.ReactNode
  trigger: ReactElement
  /** Overrides the bubble's default 320px cap — for content that reads worse wrapped that narrow
   * (e.g. a multi-line search-syntax reference). */
  maxWidth?: number
  zIndex?: number
}) {
  const palette = useMenuPalette()
  return (
    <Popover.Root>
      <Popover.Trigger render={trigger} />
      <Popover.Portal>
        <Popover.Positioner sideOffset={8} collisionPadding={8} className="outline-none" style={{ zIndex }}>
          <Popover.Popup
            className={`thin-scrollbar rounded-[.3em] ${FLOATING_POPUP_TRANSITION_CLASSES}`}
            style={{
              background: palette.bg,
              border: `1px solid ${palette.border}`,
              boxShadow: FLOATING_POPUP_SHADOW,
              color: palette.text,
              padding: "10px 14px",
              fontSize: FONT_SIZE_MD,
              lineHeight: 1.6,
              textAlign: "left",
              maxWidth,
              maxHeight: "min(60vh, 400px)",
              overflowY: "auto",
              transformOrigin: "var(--transform-origin)",
            }}
          >
            {label}
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  )
}
