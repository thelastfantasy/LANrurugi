import { Popover } from "@base-ui/react/popover"
import type { ReactElement } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, FONT_SIZE_MD, Z_OVERLAY_TOOLTIP } from "@/theme"

/** Click-to-open/close popover — the "?" help-bubble pattern, built on Base UI's `Popover`.
 * `Tooltip.tsx` stays separate (hover/focus lifecycle) rather than sharing this primitive. */
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
