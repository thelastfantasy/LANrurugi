import { Drawer as BaseDrawer } from "@base-ui/react/drawer"
import { createContext, type ReactNode, useContext, useState } from "react"
import { createPortal } from "react-dom"

import { IconButton } from "@/components/common-ui/Form"

const FooterSlot = createContext<HTMLElement | null>(null)

/** Renders its children in the open `Drawer`'s fixed bottom bar, wherever it sits in the content —
 * so an action row can stay with the state it acts on and still stay put while the content scrolls. */
export function DrawerFooter({ children }: { children: ReactNode }) {
  const slot = useContext(FooterSlot)
  return slot ? createPortal(children, slot) : null
}

/** Full-height side panel that slides in from the right, built on Base UI's `Drawer`, on the same
 * `.id1` box as `Modal`. Unlike `Modal`, the caller keeps it mounted and drives `open`: the
 * content is only rendered while open (and through its exit slide), so effects inside `children`
 * run each time it opens. Swipe right or press Escape/outside to close. Content scrolls between a
 * fixed title bar and an optional fixed bottom bar (`DrawerFooter`). */
export function Drawer({
  open,
  onClose,
  title,
  children,
  width = 560,
  swipeToClose = true,
}: {
  open: boolean
  onClose: () => void
  title?: ReactNode
  children: ReactNode
  width?: number
  /** `false` when the content uses horizontal swipes itself (e.g. paging), which would otherwise
   * fight the drawer's swipe-right-to-close. Closing then stays on the button, backdrop and Escape. */
  swipeToClose?: boolean
}) {
  const [footer, setFooter] = useState<HTMLDivElement | null>(null)
  return (
    <BaseDrawer.Root open={open} onOpenChange={(next) => !next && onClose()} swipeDirection="right">
      <BaseDrawer.Portal>
        <BaseDrawer.Backdrop
          className="fixed inset-0 bg-black opacity-[calc(0.6*(1-var(--drawer-swipe-progress)))] transition-opacity duration-300 ease-[cubic-bezier(0.32,0.72,0,1)] data-[ending-style]:opacity-0 data-[starting-style]:opacity-0 data-[swiping]:duration-0"
          style={{ zIndex: 9000 }}
        />
        <BaseDrawer.Viewport
          className="fixed inset-0 flex items-stretch justify-end"
          style={{ zIndex: 9001 }}
          data-base-ui-swipe-ignore={swipeToClose ? undefined : ""}
        >
          <BaseDrawer.Popup
            className="id1 overflow-hidden text-left outline-none [transform:translateX(var(--drawer-swipe-movement-x))] transition-transform duration-300 ease-[cubic-bezier(0.32,0.72,0,1)] data-[ending-style]:[transform:translateX(100%)] data-[starting-style]:[transform:translateX(100%)] data-[swiping]:select-none data-[swiping]:duration-0"
            // Layout inline: legacy `div.id1` (display: inline-block, a fixed width, margins and
            // rounded corners for archive cards) outranks Tailwind's single-class utilities.
            style={{
              display: "flex",
              flexDirection: "column",
              height: "100%",
              width,
              maxWidth: "100vw",
              margin: 0,
              borderRadius: 0,
              borderTop: "none",
              borderRight: "none",
              borderBottom: "none",
              padding: 0,
            }}
          >
            <div className="flex shrink-0 items-center gap-3" style={{ padding: "14px 16px 10px 20px" }}>
              <BaseDrawer.Title className="m-0 min-w-0 flex-1 truncate" style={{ fontSize: 15, fontWeight: 700 }}>
                {title}
              </BaseDrawer.Title>
              <BaseDrawer.Close
                render={
                  <IconButton
                    icon="fa fa-times"
                    size={30}
                    className="modal-close-btn"
                    style={{ borderRadius: "50%", flexShrink: 0 }}
                  />
                }
              />
            </div>
            <BaseDrawer.Content className="min-h-0 flex-1 overflow-y-auto overscroll-contain" style={{ padding: "4px 20px 20px" }}>
              <FooterSlot.Provider value={footer}>{children}</FooterSlot.Provider>
            </BaseDrawer.Content>
            {/* Hidden until a `DrawerFooter` renders into it. */}
            <div
              ref={setFooter}
              className="shrink-0 empty:hidden"
              style={{ padding: "12px 20px", borderTop: "1px solid rgba(128,128,128,0.25)" }}
            />
          </BaseDrawer.Popup>
        </BaseDrawer.Viewport>
      </BaseDrawer.Portal>
    </BaseDrawer.Root>
  )
}
