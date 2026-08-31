import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useActivityRetention, useUpdateActivityRetention } from "@/api/hooks"
import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { Z_OVERLAY_CONTENT } from "@/theme"
import { toast } from "@/toast"

/** Same five-option shape as `ApiTokensSection.tsx`'s `EXPIRY_OPTIONS`. `"forever"` maps to
 * `null` (`retention_secs: None`, "keep forever"). */
const RETENTION_OPTIONS: { value: string; secs: number | null }[] = [
  { value: "1w", secs: 7 * 86400 },
  { value: "1m", secs: 30 * 86400 },
  { value: "3m", secs: 90 * 86400 },
  { value: "1y", secs: 365 * 86400 },
  { value: "forever", secs: null },
]

function closestOption(secs: number | null): string {
  if (secs == null) return "forever"
  const match = RETENTION_OPTIONS.find((o) => o.secs === secs)
  return match?.value ?? "forever"
}

/** Gear-icon popover for the activity retention period — same trigger/menu pattern as
 * `Library/SettingsMenu.tsx`'s view-options gear. */
export function RetentionSettingsMenu() {
  const { t } = useTranslation()
  const retention = useActivityRetention()
  const updateRetention = useUpdateActivityRetention()
  const selected = closestOption(retention.data?.retention_secs ?? null)

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

  async function handleChange(value: string) {
    const option = RETENTION_OPTIONS.find((o) => o.value === value)
    try {
      await updateRetention.mutateAsync(option?.secs ?? null)
      toast({ text: t("activity.retentionUpdated") ?? undefined, icon: "success" })
    } catch {
      toast({ heading: t("activity.errorUpdatingRetention") ?? undefined, icon: "error" })
    }
  }

  return (
    <span ref={ref} style={{ position: "relative" }}>
      <a
        href="#"
        className="fa fa-cog fa-2x table-option"
        style={{ position: "relative" }}
        title={t("activity.retentionLabel") ?? undefined}
        onClick={(e) => {
          e.preventDefault()
          if (!open && ref.current) {
            const rect = ref.current.getBoundingClientRect()
            const spaceRight = window.innerWidth - rect.right
            setOpenTowardLeft(spaceRight < 220)
          }
          setOpen((v) => !v)
        }}
      ></a>
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
          mainLabel={{ icon: "fa-cog", text: t("activity.retentionLabel") ?? "" }}
        >
          <PopupMenuItem disabled style={{ display: "block" }}>
            <select
              className="stdinput"
              style={{ width: "100%", boxSizing: "border-box" }}
              value={selected}
              disabled={updateRetention.isPending}
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => void handleChange(e.target.value)}
            >
              {RETENTION_OPTIONS.map((option) => (
                <option key={option.value} value={option.value}>
                  {t(`activity.retentionOption.${option.value}`)}
                </option>
              ))}
            </select>
          </PopupMenuItem>
        </PopupMenu>
      )}
    </span>
  )
}
