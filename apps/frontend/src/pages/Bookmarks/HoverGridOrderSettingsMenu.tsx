import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { IconButton, PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { Z_OVERLAY_CONTENT } from "@/theme"

import { type HoverGridPageOrder, useHoverGridPageOrder } from "./useHoverGridPageOrder"

const OPTIONS: HoverGridPageOrder[] = ["bookmarkedAtDesc", "bookmarkedAtAsc", "pageAsc", "pageDesc"]

/** Gear-icon popover controlling how `BookmarkHoverGrid` orders the pages inside its own popup —
 * same trigger/menu pattern as `Activity/RetentionSettingsInline.tsx`'s own gear (own `open`
 * state, an outside-click listener that also checks the portaled menu's own ref, a left/right
 * open direction picked from available viewport space) and `Library/SettingsMenu.tsx`'s (radio
 * options inside a `PopupMenu`, `localStorage`-backed rather than a server round-trip). Lives next
 * to `BookmarksPage.tsx`'s own archive-sort `<select>` — a separate control because it governs a
 * different, unrelated ordering (pages *within* one archive's hover preview, not the list of
 * archives itself). */
export function HoverGridOrderSettingsMenu() {
  const { t } = useTranslation()
  const [order, setOrder] = useHoverGridPageOrder()

  const [open, setOpen] = useState(false)
  const [openTowardLeft, setOpenTowardLeft] = useState(false)
  const ref = useRef<HTMLSpanElement>(null)
  const menuRef = useRef<HTMLUListElement>(null)

  useEffect(() => {
    if (!open) return
    function onClick(e: globalThis.MouseEvent) {
      const target = e.target as Node
      if (ref.current?.contains(target)) return
      if (menuRef.current?.contains(target)) return
      setOpen(false)
    }
    document.addEventListener("click", onClick)
    return () => document.removeEventListener("click", onClick)
  }, [open])

  return (
    <span ref={ref} style={{ position: "relative", display: "inline-flex" }}>
      <IconButton
        icon={<i className="fa fa-cog" style={{ fontSize: 18 }}></i>}
        size={25}
        title={t("bookmarks.hoverGridOrderLabel") ?? undefined}
        style={{ border: "none", background: "transparent", position:"relative", top: 2 }}
        onClick={() => {
          if (!open && ref.current) {
            const rect = ref.current.getBoundingClientRect()
            const spaceRight = window.innerWidth - rect.right
            setOpenTowardLeft(spaceRight < 220)
          }
          setOpen((v) => !v)
        }}
      />
      {open && (
        <PopupMenu
          ref={menuRef}
          portal={false}
          style={{
            position: "absolute",
            top: "100%",
            ...(openTowardLeft ? { right: 0 } : { left: 0 }),
            zIndex: Z_OVERLAY_CONTENT,
          }}
          mainLabel={{ icon: "fa-cog", text: t("bookmarks.hoverGridOrderLabel") ?? "" }}
        >
          {OPTIONS.map((option) => (
            <PopupMenuItem key={option} onClick={() => setOrder(option)}>
              <span style={{ display: "flex",  gap: 4, alignItems: "flex-end" }}>
                <input type="radio" readOnly checked={order === option} />
                <span style={{ verticalAlign: "middle" }}>{t(`bookmarks.hoverGridOrderOption.${option}`)}</span>
              </span>
            </PopupMenuItem>
          ))}
        </PopupMenu>
      )}
    </span>
  )
}
