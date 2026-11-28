import { Combobox } from "@base-ui/react/combobox"
import { useLayoutEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { FaCaretDown, FaCheck } from "react-icons/fa6"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, FLOATING_POPUP_TRANSITION_CLASSES, Z_OVERLAY_ABOVE_LEGACY_MODAL, Z_OVERLAY_CONTENT } from "@/theme"

/** `+0900`-style UTC offset of `tz` right now (so a DST zone shows its current one). */
function utcOffset(tz: string): string {
  try {
    const name = new Intl.DateTimeFormat("en-US", { timeZone: tz, timeZoneName: "longOffset" })
      .formatToParts(new Date())
      .find((part) => part.type === "timeZoneName")?.value
    const match = name?.match(/^GMT([+-])(\d{2}):(\d{2})$/)
    return match ? `${match[1]}${match[2]}${match[3]}` : "+0000"
  } catch {
    return ""
  }
}

// Full IANA timezone list, grouped by continent (phpBB-style), built from
// `Intl.supportedValuesOf('timeZone')` so it stays in sync with the backend's `chrono-tz`.
const TIMEZONE_GROUPS: { value: string; items: string[] }[] = (() => {
  const supported: string[] = (Intl.supportedValuesOf?.("timeZone") as string[] | undefined) ?? []
  const groups = new Map<string, string[]>()
  for (const tz of supported) {
    if (tz === "UTC") continue
    const area = tz.includes("/") ? tz.slice(0, tz.indexOf("/")) : "Other"
    const bucket = groups.get(area)
    if (bucket) {
      bucket.push(tz)
    } else {
      groups.set(area, [tz])
    }
  }
  const order = ["Africa", "America", "Antarctica", "Asia", "Atlantic", "Australia", "Europe", "Indian", "Pacific"]
  const sortedAreas = [...groups.keys()].sort((a, b) => {
    const ai = order.indexOf(a)
    const bi = order.indexOf(b)
    return (ai === -1 ? order.length : ai) - (bi === -1 ? order.length : bi) || a.localeCompare(b)
  })
  return [{ value: "", items: ["UTC"] }, ...sortedAreas.map((area) => ({ value: area, items: (groups.get(area) ?? []).sort() }))]
})()

const OFFSETS = new Map(TIMEZONE_GROUPS.flatMap((g) => g.items.map((tz) => [tz, utcOffset(tz)] as const)))

/** What a query is matched against: the id with `_` read as a space ("new york"), and the offset
 *  with and without its colon ("+09", "+09:00", "-0330"). */
const SEARCH_TEXT = new Map(
  [...OFFSETS].map(([tz, offset]) => {
    const colon = offset ? `${offset.slice(0, 3)}:${offset.slice(3)}` : ""
    return [tz, `${tz} ${tz.replace(/_/g, " ")} ${offset} ${colon}`.toLowerCase()] as const
  }),
)

/** Every space-separated word of the query must appear somewhere, in any order. */
function matchesTimezone(tz: string, query: string): boolean {
  const text = SEARCH_TEXT.get(tz) ?? tz.toLowerCase()
  return query
    .toLowerCase()
    .split(/\s+/)
    .every((word) => text.includes(word))
}

/**
 * IANA timezone picker: the supported zones, filterable and each shown with its current UTC
 * offset, plus a free-text field when the stored value is not one this browser knows (an
 * older/other `chrono-tz` value, or something hand-written).
 *
 * Shared by Settings and the first-run wizard. Deliberately just the control: labelling and the
 * explanatory hint stay with each caller.
 */
export function TimezoneSelect({
  value,
  onChange,
  ariaLabel,
}: {
  value: string
  onChange: (timezone: string) => void
  /** The caller owns the visible label, so it has to hand the accessible name over too — a `<Row
   *  label=...>` or an `<h3>` is not one. */
  ariaLabel?: string
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const known = OFFSETS.has(value)
  const triggerRef = useRef<HTMLButtonElement | null>(null)
  const [overlayZIndex, setOverlayZIndex] = useState<number | undefined>(undefined)

  // Same as `common-ui/Form/Select`: inside a dialog the portalled popup must clear its z-index.
  useLayoutEffect(() => {
    const overlay = triggerRef.current?.closest<HTMLElement>("[data-overlay]")
    setOverlayZIndex(overlay ? Z_OVERLAY_ABOVE_LEGACY_MODAL : undefined)
  }, [])

  return (
    <>
      <Combobox.Root<string>
        items={TIMEZONE_GROUPS}
        value={known ? value : null}
        onValueChange={(next) => {
          if (next) onChange(next)
        }}
        filter={matchesTimezone}
        autoHighlight
      >
        {/* Looks like the settings rows' other selects (`.stdinput`, full width), not a button. */}
        <Combobox.Trigger
          ref={triggerRef}
          className="stdinput"
          aria-label={ariaLabel}
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            gap: 6,
            width: "100%",
            cursor: "pointer",
            textAlign: "left",
          }}
        >
          <span className="truncate">
            {known ? `${value} (${OFFSETS.get(value)})` : `${t("settings.custom")} (${value})`}
          </span>
          <FaCaretDown aria-hidden="true" />
        </Combobox.Trigger>
        <Combobox.Portal>
          <Combobox.Positioner
            align="start"
            sideOffset={4}
            className="outline-none"
            style={{ zIndex: overlayZIndex ?? Z_OVERLAY_CONTENT }}
          >
            <Combobox.Popup
              aria-label={ariaLabel}
              // text-left resets legacy's body { text-align: center }, which inherits into the portal.
              className={`flex flex-col rounded-[.2em] text-left ${FLOATING_POPUP_TRANSITION_CLASSES}`}
              style={{
                background: palette.bg,
                border: `1px solid ${palette.border}`,
                boxShadow: FLOATING_POPUP_SHADOW,
                color: palette.text,
                transformOrigin: "var(--transform-origin)",
                width: "max(var(--anchor-width), min(22rem, calc(100vw - 2rem)))",
                maxWidth: "calc(100vw - 2rem)",
                maxHeight: "min(26rem, var(--available-height))",
              }}
            >
              <div style={{ padding: 6, borderBottom: `1px solid ${palette.border}` }}>
                <Combobox.Input
                  className="stdinput"
                  style={{ width: "100%", boxSizing: "border-box" }}
                  placeholder={t("settings.timezoneFilterPlaceholder") ?? ""}
                />
              </div>
              <Combobox.Empty>
                <div style={{ padding: "8px 12px", opacity: 0.7 }}>{t("settings.timezoneNoMatch")}</div>
              </Combobox.Empty>
              <Combobox.List className="thin-scrollbar min-h-0 flex-1 overflow-y-auto overscroll-contain py-[.25em] empty:p-0">
                {(group: { value: string; items: string[] }) => (
                  <Combobox.Group key={group.value || "UTC"} items={group.items}>
                    {group.value && (
                      <Combobox.GroupLabel className="px-3 pt-2 pb-1 text-[0.85em] font-bold opacity-70 select-none">
                        {group.value}
                      </Combobox.GroupLabel>
                    )}
                    <Combobox.Collection>
                      {(tz: string) => (
                        <Combobox.Item
                          key={tz}
                          value={tz}
                          className="select-item-highlighted grid cursor-pointer grid-cols-[1em_1fr_auto] items-center gap-2 py-[.3em] ps-2 pe-3 outline-none select-none"
                        >
                          <Combobox.ItemIndicator className="col-start-1 flex text-xs">
                            <FaCheck aria-hidden="true" />
                          </Combobox.ItemIndicator>
                          <span className="col-start-2 truncate">{tz}</span>{" "}
                          <span className="col-start-3 tabular-nums opacity-70">{OFFSETS.get(tz)}</span>
                        </Combobox.Item>
                      )}
                    </Combobox.Collection>
                  </Combobox.Group>
                )}
              </Combobox.List>
            </Combobox.Popup>
          </Combobox.Positioner>
        </Combobox.Portal>
      </Combobox.Root>
      {!known && (
        <input
          className="stdinput"
          style={{ width: "100%", marginTop: 4 }}
          value={value}
          onChange={(e) => onChange(e.target.value)}
          type="text"
          placeholder="e.g. Asia/Tokyo"
        />
      )}
    </>
  )
}
