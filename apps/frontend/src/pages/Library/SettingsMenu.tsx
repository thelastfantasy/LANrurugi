import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { PopupMenu, PopupMenuItem, PopupMenuSeparator } from "@/components/Overlay/PopupMenu"
import { Z_OVERLAY_CONTENT } from "@/theme"

/** Settings gear menu (legacy's `#settings-menu` contextMenu, `index.js:117-199`) — bundles
 * Display Mode (thumbnail grid vs compact table), Crop Thumbnails, Hide Completed, and Group
 * Tankoubons into one dropdown, each persisted to the same `localStorage` keys legacy itself
 * uses. Positioned next to "Go to Page", matching legacy's own placement. */
export function SettingsMenu({
  viewMode,
  setViewMode,
  cropThumbs,
  setCropThumbs,
  hideCompleted,
  setHideCompleted,
  groupbyTanks,
  setGroupbyTanks,
}: {
  viewMode: "thumbnail" | "compact"
  setViewMode: (v: "thumbnail" | "compact") => void
  cropThumbs: boolean
  setCropThumbs: (v: boolean) => void
  hideCompleted: boolean
  setHideCompleted: (v: boolean) => void
  groupbyTanks: boolean
  setGroupbyTanks: (v: boolean) => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  // Which side the menu opens toward — decided fresh each time it opens, not a fixed direction,
  // from the gear icon's own position: opens
  // toward the side with more room, so it never gets clipped by the viewport edge regardless of
  // where "Go to Page"/the gear ends up sitting (this toolbar is at the far right of the page, so
  // a hardcoded direction is wrong in one direction or the other depending on viewport width).
  const [openTowardLeft, setOpenTowardLeft] = useState(false)
  const ref = useRef<HTMLSpanElement>(null)
  // The menu itself is portaled to `document.body` (see PopupMenu's doc comment), so it's no
  // longer a DOM descendant of `ref` — checking only `ref.current.contains(...)` below would
  // treat every click *inside* the open menu as an "outside" click and close it before the
  // item's own onClick fires. Also checking this second ref against the portaled `<ul>` fixes it.
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
    <span ref={ref} style={{ position: "relative", marginLeft: 6, top: 2 }}>
      <a
        href="#"
        className="fa fa-cog fa-2x table-option"
        style={{ position: "relative", }}
        title={t("Index Settings") ?? undefined}
        onClick={(e) => {
          e.preventDefault()
          if (!open && ref.current) {
            const rect = ref.current.getBoundingClientRect()
            const spaceRight = window.innerWidth - rect.right
            // Menu itself is ~220px (`PopupMenu`'s own min-width) — opens left if the right side
            // genuinely doesn't have room for it, not just "less than the left side".
            setOpenTowardLeft(spaceRight < 220)
          }
          setOpen((v) => !v)
        }}
      ></a>
      {open && (
        <PopupMenu
          ref={menuRef}
          // `position: absolute` here is measured against this menu's own trigger `<span
          // ref={ref} style={{ position: 'relative' }}>` (`top: '100%'`/`left`/`right: 0`) — not
          // portaled, so that ancestor is still the one it's positioned against instead of
          // `document.body`. The toolbar this lives in doesn't clip overflow, so there's no
          // clipping problem `portal` would be solving here anyway.
          portal={false}
          style={{
            position: "absolute",
            top: "100%",
            ...(openTowardLeft ? { right: 0 } : { left: 0 }),
            zIndex: Z_OVERLAY_CONTENT,
          }}
          // The gear icon's own real title (`Index Settings`) — this menu's actual name, as
          // distinct from `Display Mode` right below, which is a sub-heading for just the
          // Thumbnail/Compact radio pair, not the whole menu.
          mainLabel={{ icon: "fa-cog", text: t("Index Settings") ?? "Index Settings" }}
        >
          <PopupMenuItem disabled>
            <i className="fas fa-table" style={{ width: 18 }}></i> {t("Display Mode")}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setViewMode("thumbnail")}>
            <input type="radio" readOnly checked={viewMode === "thumbnail"} /> {t("Thumbnail")}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setViewMode("compact")}>
            <input type="radio" readOnly checked={viewMode === "compact"} /> {t("Compact")}
          </PopupMenuItem>
          <PopupMenuSeparator />
          {/* `marginLeft: 0` overrides the browser's own native checkbox UA-stylesheet margin
              (Chrome: 4px) — left as default, these sit ~4px right of the icon column above
              (`索引设置`/`显示模式`'s `<i style={{ width: 18 }}>`, which has no such margin),
              even though these three are top-level toggles (siblings of Display Mode), not
              sub-items nested under it the way the radio pair above them is. */}
          <PopupMenuItem onClick={() => setCropThumbs(!cropThumbs)}>
            <input
              type="checkbox"
              readOnly
              checked={cropThumbs}
              style={{ marginLeft: 0 }}
            />{" "}
            {t("Crop thumbnails")}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setHideCompleted(!hideCompleted)}>
            <input
              type="checkbox"
              readOnly
              checked={hideCompleted}
              style={{ marginLeft: 0 }}
            />{" "}
            {t("Hide completed Archives")}
          </PopupMenuItem>
          <PopupMenuItem onClick={() => setGroupbyTanks(!groupbyTanks)}>
            <input
              type="checkbox"
              readOnly
              checked={groupbyTanks}
              style={{ marginLeft: 0 }}
            />{" "}
            {t("Group Tankoubons")}
          </PopupMenuItem>
        </PopupMenu>
      )}
    </span>
  )
}
