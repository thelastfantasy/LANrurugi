import { Combobox } from "@base-ui/react/combobox"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useSettings } from "@/api/hooks"
import { Modal } from "@/components/Display"
import { presetTimeRange, type TimeRangePreset } from "@/lib/timeRange"

import { ActivityComboboxItem, ActivitySingleSelectShell } from "./ActivityCombobox"

const PRESETS: TimeRangePreset[] = ["last_hour", "today", "this_week", "this_month"]

export interface TimeRangeValue {
  start_ts?: number
  end_ts?: number
}

/** Preset time ranges (computed in the server's configured timezone — same source
 * `useSettings().data?.timezone` the rest of the app reads) plus a "custom" mode with two native
 * `datetime-local` inputs. Custom mode deliberately uses the *browser's own local* timezone (not
 * the server-configured one) — the user is picking an absolute point in time on their own clock
 * at that point, so there's no relative-boundary ambiguity the way "today"/"this week" has. */
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
  // Which selection produced the current `value` — `start_ts`/`end_ts` alone can't distinguish a
  // preset ("today"/"this_week"/...) from a custom range, since every preset also just resolves to
  // a concrete `start_ts`/`end_ts` pair. Without this, the trigger label always fell back to
  // "自定义范围" for every non-empty selection, including presets, confirmed live. `null` means
  // "all time" (no filter).
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

      {/* Screen-centered (the shared `Modal` component every other overlay in this app already
          uses), not anchored to the trigger — a trigger-anchored popup here would need its own
          left/right-flip logic to stay on-screen near a viewport edge (`ActivityComboboxShell`'s
          own popups already do this via `Combobox.Positioner`, which this hand-rolled panel isn't
          using), and on a narrow phone viewport specifically there's rarely room to the trigger's
          side for a 260px-wide panel with two date inputs anyway — centering sidesteps needing any
          of that. */}
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
            {/* `.stdbtn`'s own theme `min-width: 150px` is sized for a normal-width toolbar button
                — two of them side by side need at least ~306px (150 + 150 + this row's own 6px
                gap), wider than this dialog's own compact 280px, so the left button ("取消")
                overflowed the dialog's left edge with `justify-content: flex-end` packing them
                against the right side instead of shrinking them, confirmed live. `flex: 1` on both
                splits the available row width evenly between them regardless of the class's own
                min-width floor. */}
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
