import { useTranslation } from "react-i18next"

// Full IANA timezone list, grouped by continent (phpBB-style), built from
// `Intl.supportedValuesOf('timeZone')` so it stays in sync with the backend's `chrono-tz`.
const TIMEZONE_GROUPS: { label: string; zones: string[] }[] = (() => {
  const supported: string[] = (Intl.supportedValuesOf?.("timeZone") as string[] | undefined) ?? []
  const groups = new Map<string, string[]>()
  for (const tz of supported) {
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
  return sortedAreas.map((area) => ({ label: area, zones: (groups.get(area) ?? []).sort() }))
})()

function isKnownTimezone(tz: string): boolean {
  return tz === "UTC" || TIMEZONE_GROUPS.some((g) => g.zones.includes(tz))
}

/**
 * IANA timezone picker: the supported-zones dropdown, plus a free-text field when the stored value
 * is not one this browser knows (an older/other `chrono-tz` value, or something hand-written).
 *
 * Extracted from the Settings page's Tags & Thumbnails section so the first-run wizard can offer
 * the same control — the zone list is large enough that a second copy would drift, and the value
 * means the same thing in both places (`date_added` display and day-range searches).
 *
 * Deliberately just the control: labelling and the explanatory hint stay with each caller, which
 * is what lets Settings keep its `Row label=...` table layout and the wizard use its own headings.
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
  const known = isKnownTimezone(value)

  return (
    <>
      <select
        className="stdbtn"
        aria-label={ariaLabel}
        value={known ? value : "__custom__"}
        onChange={(e) => {
          if (e.target.value === "__custom__") return
          onChange(e.target.value)
        }}
      >
        <option value="UTC">UTC</option>
        {TIMEZONE_GROUPS.map((group) => (
          <optgroup key={group.label} label={group.label}>
            {group.zones.map((tz) => (
              <option key={tz} value={tz}>
                {tz}
              </option>
            ))}
          </optgroup>
        ))}
        {!known && (
          <option value="__custom__">
            {t("settings.custom")} ({value})
          </option>
        )}
      </select>
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
