import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useActivityRetention, useUpdateActivityRetention } from "@/api/hooks"
import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { Z_OVERLAY_CONTENT } from "@/theme"
import { toast } from "@/toast"

/** Same five-option shape as `ApiTokensSection.tsx`'s own `EXPIRY_OPTIONS` (1周/1个月/3个月/1年/
 * 永久) — a familiar, already-established picker convention, reused here for "how long activity
 * records are kept" instead of "how long a token stays valid". `"forever"` maps to `null`
 * (`retention_secs: None` — matches the backend's own `Option<i64>` semantics, "keep forever"). */
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
 * `Library/SettingsMenu.tsx`'s own view-options gear (own `open` state, an outside-click listener
 * that also checks the portaled menu's own ref, a left/right open direction picked from available
 * viewport space). Pulled out of the filter bar's own inline row into this popover per direct
 * feedback: an always-visible `<select>` sitting in the middle of the filter controls read as just
 * another filter despite governing something structurally different (log retention, not the
 * current query) — a gear icon in the page's own top-right corner (matching the Library page's
 * view-settings gear) reads as "page-level setting" instead.
 *
 * Lives on the Activity page itself (not the Settings page) — the plan's own deliberate choice,
 * since this setting only ever matters in the context of the activity log it governs. The select's
 * value is derived directly from the query result (`react-query`'s own cache is the single source
 * of truth) rather than mirrored into local state via an effect. */
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
