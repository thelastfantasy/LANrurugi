import { Combobox } from "@base-ui/react/combobox"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useSettings } from "@/api/hooks"
import { Modal } from "@/components/common-ui/Display"
import { presetTimeRange, type TimeRangePreset } from "@/lib/timeRange"

import { ActivityComboboxItem, ActivitySingleSelectShell } from "./ActivityCombobox"

const PRESETS: TimeRangePreset[] = ["last_hour", "today", "this_week", "this_month"]

export interface TimeRangeValue {
  start_ts?: number
  end_ts?: number
}

/** Preset time ranges (in the server's configured timezone) plus a "custom" mode with two native
 * `datetime-local` inputs, which deliberately use the browser's own local timezone instead. */
export function TimeRangeCombobox({
  onValueChange,
}: {
  onValueChange: (value: TimeRangeValue) => void
}) {
  const { t } = useTranslation()
  const settings = useSettings()
  const timezone = settings.data?.timezone ?? ""
  const [customOpen, setCustomOpen] = useState(false)
  const [customStart, setCustomStart] = useState("")
  const [customEnd, setCustomEnd] = useState("")
  const [activeSelection, setActiveSelection] = useState<TimeRangePreset | "custom" | null>(null)

  const label =
    activeSelection === "custom"
      ? (t("activity.customRangeActive") ?? "")
      : activeSelection
        ? (t(`activity.timeRange.${activeSelection}`) ?? "")
        : (t("activity.allTime") ?? "")

  function applyPreset(preset: TimeRangePreset) {
    const range = presetTimeRange(preset, timezone)
    setActiveSelection(preset)
    onValueChange({ start_ts: range.start, end_ts: range.end })
  }

  function applyCustom() {
    const start = customStart ? Math.floor(new Date(customStart).getTime() / 1000) : undefined
    const end = customEnd ? Math.floor(new Date(customEnd).getTime() / 1000) : undefined
    setActiveSelection("custom")
    onValueChange({ start_ts: start, end_ts: end })
    setCustomOpen(false)
  }

  return (
    <>
      <Combobox.Root
        items={[...PRESETS, "custom", ""]}
        value={null}
        onValueChange={(v) => {
          if (v === "custom") {
            setCustomOpen(true)
            return
          }
          if (v === "" || v == null) {
            setActiveSelection(null)
            onValueChange({})
            return
          }
          applyPreset(v as TimeRangePreset)
        }}
      >
        <ActivitySingleSelectShell placeholder={t("activity.selectTimeRange") ?? ""} triggerLabel={label}>
          <ActivityComboboxItem value="" label={t("activity.allTime")} />
          {PRESETS.map((preset) => (
            <ActivityComboboxItem key={preset} value={preset} label={t(`activity.timeRange.${preset}`)} />
          ))}
          <ActivityComboboxItem value="custom" label={t("activity.customRange")} />
        </ActivitySingleSelectShell>
      </Combobox.Root>

      {/* Screen-centered via the shared Modal, not trigger-anchored — sidesteps needing its own
          viewport-edge flip logic for a 260px panel. */}
      {customOpen && (
        <Modal onClose={() => setCustomOpen(false)} width={280} textAlign="left">
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              {t("activity.rangeStart")}
              <input
                type="datetime-local"
                className="stdinput"
                style={{ width: "100%", boxSizing: "border-box" }}
                value={customStart}
                onChange={(e) => setCustomStart(e.target.value)}
              />
            </label>
            <label style={{ display: "flex", flexDirection: "column", gap: 4 }}>
              {t("activity.rangeEnd")}
              <input
                type="datetime-local"
                className="stdinput"
                style={{ width: "100%", boxSizing: "border-box" }}
                value={customEnd}
                onChange={(e) => setCustomEnd(e.target.value)}
              />
            </label>
            <div style={{ display: "flex", gap: 6 }}>
              <button type="button" className="stdbtn" style={{ flex: 1, minWidth: 0 }} onClick={() => setCustomOpen(false)}>
                {t("common.cancel")}
              </button>
              <button type="button" className="stdbtn" style={{ flex: 1, minWidth: 0 }} onClick={applyCustom}>
                {t("common.apply")}
              </button>
            </div>
          </div>
        </Modal>
      )}
    </>
  )
}
