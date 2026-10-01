import { Popover as BasePopover } from "@base-ui/react/popover"
import type { ComponentProps, ReactElement, ReactNode } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, Z_OVERLAY_TOOLTIP } from "@/theme"

/** Site-wide click-triggered speech-bubble popup with a pointing arrow, built on Base UI's
 * `Popover`. Distinct from `Tooltip` (hover-only, no arrow) and `ClickPopover` (click-toggle,
 * "?" help-bubble content, no arrow) — use this when the popup needs to visibly point at its
 * trigger, e.g. a compact action bar anchored to a small icon. */
export function Popover({
  trigger,
  triggerNativeButton = true,
  children,
  sideOffset = 10,
  ...rootProps
}: {
  trigger: ReactElement
  /** Set `false` when `trigger` isn't a real `<button>` (e.g. an invisible positioning anchor) —
   * otherwise Base UI logs a dev warning about missing native button semantics. */
  triggerNativeButton?: boolean
  children: ReactNode
  sideOffset?: number
} & Omit<ComponentProps<typeof BasePopover.Root>, "children">) {
  const palette = useMenuPalette()
  return (
    <BasePopover.Root {...rootProps}>
      <BasePopover.Trigger nativeButton={triggerNativeButton} render={trigger} />
      <BasePopover.Portal>
        <BasePopover.Positioner sideOffset={sideOffset} collisionPadding={8} className="outline-none" style={{ zIndex: Z_OVERLAY_TOOLTIP }}>
          <BasePopover.Popup
            // Autofocusing the first button would also trigger its Tooltip via focus, reading as
            // "opened on its own" — this bubble is click-toggled, not modal, so skip autofocus.
            initialFocus={false}
            className={`relative rounded-[.3em] ${FLOATING_POPUP_TRANSITION_CLASSES}`}
            style={{
              background: palette.bg,
              border: `1px solid ${palette.border}`,
              boxShadow: FLOATING_POPUP_SHADOW,
              color: palette.text,
              transformOrigin: "var(--transform-origin)",
            }}
          >
            <BasePopover.Arrow
              className="relative block h-[6px] w-3 overflow-hidden data-[side=bottom]:top-[-6px] data-[side=left]:right-[-9px] data-[side=left]:rotate-90 data-[side=right]:left-[-9px] data-[side=right]:-rotate-90 data-[side=top]:bottom-[-6px] data-[side=top]:rotate-180 before:absolute before:bottom-0 before:left-1/2 before:box-border before:h-[calc(6px*sqrt(2))] before:w-[calc(6px*sqrt(2))] before:border before:border-[var(--arrow-border)] before:bg-[var(--arrow-bg)] before:[transform:translate(-50%,50%)_rotate(45deg)] before:content-['']"
              style={{ ["--arrow-bg" as string]: palette.bg, ["--arrow-border" as string]: palette.border }}
            />
            {children}
          </BasePopover.Popup>
        </BasePopover.Positioner>
      </BasePopover.Portal>
    </BasePopover.Root>
  )
}
