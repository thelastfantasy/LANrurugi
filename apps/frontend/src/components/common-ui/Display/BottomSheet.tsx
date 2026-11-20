import { Drawer as BaseDrawer } from "@base-ui/react/drawer"
import { type CSSProperties, type ReactNode, useEffect, useState } from "react"

import { useMenuPalette } from "@/hooks/useMenuPalette"

/** Phone-width action sheet: slides up from the bottom edge, swipe down or tap outside to close,
 * with a fixed cancel bar under the scrolling content. Without `open` it opens on mount and calls
 * `onClose` once its exit slide has finished; with `open` the caller drives it, which avoids the
 * mount-then-open frame when a sheet is opened from another one. */
export function BottomSheet({
  open: controlledOpen,
  onClose,
  onExited,
  cancelLabel,
  children,
  zIndex = 9000,
  aboveKeyboard = false,
}: {
  open?: boolean
  onClose: () => void
  /** After the exit slide has finished, in either mode. */
  onExited?: () => void
  cancelLabel: ReactNode
  children: ReactNode
  /** The backdrop's layer; the sheet sits one above. Raised for a sheet that must cover a modal. */
  zIndex?: number
  /** For a sheet holding a text field: sits on the on-screen keyboard instead of behind it. Mobile
   *  browsers shrink only the visual viewport for the keyboard, which `dvh` does not follow. */
  aboveKeyboard?: boolean
}) {
  const palette = useMenuPalette()
  const keyboardBox = useVisualViewportBox(aboveKeyboard)
  // Uncontrolled: opened one frame after mount so the entry slide plays.
  const [selfOpen, setSelfOpen] = useState(false)
  useEffect(() => {
    if (controlledOpen !== undefined) return
    const frame = requestAnimationFrame(() => setSelfOpen(true))
    return () => cancelAnimationFrame(frame)
  }, [controlledOpen])
  const open = controlledOpen ?? selfOpen
  return (
    <BaseDrawer.Root
      open={open}
      onOpenChange={(next) => {
        if (next) return
        if (controlledOpen === undefined) setSelfOpen(false)
        else onClose()
      }}
      onOpenChangeComplete={(next) => {
        if (next) return
        if (controlledOpen === undefined) onClose()
        onExited?.()
      }}
      swipeDirection="down"
    >
      <BaseDrawer.Portal>
        <BaseDrawer.Backdrop
          className="fixed inset-0 bg-black opacity-[calc(0.5*(1-var(--drawer-swipe-progress)))] transition-opacity duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] data-[ending-style]:opacity-0 data-[starting-style]:opacity-0 data-[swiping]:duration-0"
          style={{ zIndex }}
        />
        {/* Sized to the dynamic viewport: with `inset: 0` the bottom edge follows the layout viewport,
            which mobile browsers move while their toolbar collapses. */}
        <BaseDrawer.Viewport
          className="fixed inset-x-0 top-0 flex items-end justify-center"
          style={
            keyboardBox
              ? { top: keyboardBox.top, height: keyboardBox.height, zIndex: zIndex + 1 }
              : { height: "100dvh", zIndex: zIndex + 1 }
          }
        >
          <BaseDrawer.Popup
            className="flex w-full flex-col overflow-hidden outline-none [transform:translateY(var(--drawer-swipe-movement-y))] transition-transform duration-200 ease-[cubic-bezier(0.32,0.72,0,1)] data-[ending-style]:[transform:translateY(100%)] data-[starting-style]:[transform:translateY(100%)] data-[swiping]:select-none data-[swiping]:duration-0"
            style={{
              maxHeight: "85dvh",
              background: palette.bg,
              color: palette.text,
              borderRadius: "14px 14px 0 0",
              boxShadow: palette.shadow,
            }}
          >
            <BaseDrawer.Content className="min-h-0 flex-1 overflow-y-auto overscroll-contain" style={{ padding: "6px 16px 12px" }}>
              {children}
            </BaseDrawer.Content>
            <div className="shrink-0" style={{ height: 6, background: palette.separator }} />
            <BaseDrawer.Close
              className="flex shrink-0 cursor-pointer items-center justify-center border-0 bg-transparent"
              style={{
                color: "inherit",
                fontSize: 16,
                lineHeight: 1,
                // Same height as the tiles above. No safe-area inset: the page does not opt into
                // `viewport-fit=cover`, yet some mobile browsers still report one and pad it twice.
                height: 48,
                padding: 0,
              }}
            >
              {cancelLabel}
            </BaseDrawer.Close>
          </BaseDrawer.Popup>
        </BaseDrawer.Viewport>
      </BaseDrawer.Portal>
    </BaseDrawer.Root>
  )
}

/** The visual viewport's top and height while `enabled`, following the on-screen keyboard. */
function useVisualViewportBox(enabled: boolean): { top: number; height: number } | null {
  const [box, setBox] = useState<{ top: number; height: number } | null>(null)
  useEffect(() => {
    const viewport = window.visualViewport
    if (!enabled || !viewport) return
    const update = () => setBox({ top: viewport.offsetTop, height: viewport.height })
    update()
    viewport.addEventListener("resize", update)
    viewport.addEventListener("scroll", update)
    return () => {
      viewport.removeEventListener("resize", update)
      viewport.removeEventListener("scroll", update)
    }
  }, [enabled])
  return enabled ? box : null
}

/** Two-column grid of sheet buttons; `separator` draws a rule above it. */
export function SheetGrid({ children, separator }: { children: ReactNode; separator?: string }) {
  return (
    <div
      className="grid grid-cols-2 gap-2.5"
      style={separator ? { paddingTop: 12, borderTop: `1px solid ${separator}` } : undefined}
    >
      {children}
    </div>
  )
}

/** A sheet button: optional left icon, left-aligned label. */
export function SheetTile({
  children,
  icon,
  onClick,
  style,
  danger = false,
}: {
  children: ReactNode
  /** Font Awesome classes without the `fa ` prefix, e.g. `fa-download`. */
  icon?: string
  onClick: () => void
  style?: CSSProperties
  /** Each theme's own destructive-button colors (`.stdbtn-danger`, as in delete confirmations). */
  danger?: boolean
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={`min-w-0 cursor-pointer truncate rounded-md border-0${danger ? " stdbtn-danger" : ""}`}
      // A neutral tint reads on every theme's sheet background, light or dark.
      style={{
        ...(danger ? {} : { background: "rgba(128,128,128,0.14)", color: "inherit" }),
        fontSize: 15,
        textAlign: "left",
        padding: "12px 14px",
        ...style,
      }}
    >
      {icon && (
        <i
          className={`fa ${icon}`}
          style={{ display: "inline-block", width: "1.25em", textAlign: "center", marginRight: 10 }}
        ></i>
      )}
      {children}
    </button>
  )
}
