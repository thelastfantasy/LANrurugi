/** A labelled date-and-time field for any form: a button showing `YYYY-MM-DD HH:MM`, opening a
 * `react-day-picker` calendar in a popover. It exists because the native `datetime-local` picker
 * left its own overlay behind on Android after Cancel, and because the calendar has to follow the
 * app's language and theme. */

import "react-day-picker/style.css"
import "./DateTimeField.css"

import { Popover as BasePopover } from "@base-ui/react/popover"
import { useRef, useState } from "react"
import { DayPicker } from "react-day-picker"
import { enUS, zhCN } from "react-day-picker/locale"
import { useTranslation } from "react-i18next"

import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FLOATING_POPUP_SHADOW, Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"

const pad = (value: number) => String(value).padStart(2, "0")

/** `YYYY-MM-DD` in local time — the value format of the date-only mode. */
export function formatDate(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

/** A `YYYY-MM-DD` string back to a Date at local midnight. */
export function parseDate(value: string): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value.trim())
  if (!match) return null
  const [, year, month, day] = match.map(Number)
  const date = new Date(year, month - 1, day, 0, 0, 0, 0)
  return Number.isNaN(date.getTime()) ? null : date
}

export function formatLocal(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`
}

/** `YYYY-MM-DD HH:MM` (local) back to a Date. */
export function parseLocal(value: string): Date | null {
  const match = /^(\d{4})-(\d{2})-(\d{2}) (\d{2}):(\d{2})$/.exec(value.trim())
  if (!match) return null
  const [, year, month, day, hour, minute] = match.map(Number)
  const date = new Date(year, month - 1, day, hour, minute, 0, 0)
  return Number.isNaN(date.getTime()) ? null : date
}

export function DateTimeField({
  label,
  value,
  onChange,
  mode = "datetime",
  style,
}: {
  label?: string
  /** `YYYY-MM-DD HH:MM` for `datetime`, `YYYY-MM-DD` for `date`. */
  value: string
  onChange: (next: string) => void
  /** Date only: the calendar writes `YYYY-MM-DD` and closes on pick, with no time row. */
  mode?: "date" | "datetime"
  /** Applied to the trigger itself, so a caller can pin a width without wrapping the control. */
  style?: React.CSSProperties
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const [open, setOpen] = useState(false)
  const anchor = useRef<HTMLButtonElement | null>(null)

  const control = (
    <BasePopover.Root open={open} onOpenChange={setOpen}>
        <BasePopover.Trigger
          ref={anchor}
          className="stdinput lrrg-datetime-trigger"
          style={{ width: "100%", ...style }}
          aria-label={label ?? value ?? undefined}
        >
          <span className="lrrg-datetime-value">{value || t("datetime.pickDate")}</span>
          <i className="far fa-calendar" aria-hidden="true" />
        </BasePopover.Trigger>
        <BasePopover.Portal>
          {/* An invisible, click-catching layer while the calendar is open: a stray tap on the page
              behind it should close the calendar, never press whatever sits under the finger. */}
          <BasePopover.Backdrop className="lrrg-datetime-backdrop" />
          <BasePopover.Positioner
            sideOffset={4}
            align="start"
            collisionPadding={8}
            className="outline-none"
            style={{ zIndex: Z_OVERLAY_ABOVE_LEGACY_MODAL }}
          >
            <BasePopover.Popup
              className="lrrg-datetime rounded-[.3em]"
              style={{
                background: palette.bg,
                border: `1px solid ${palette.border}`,
                boxShadow: FLOATING_POPUP_SHADOW,
                color: palette.text,
              }}
            >
              <DateTimePanel
                value={value}
                mode={mode}
                onChange={(next) => {
                  onChange(next)
                  if (mode === "date") setOpen(false)
                }}
              />
            </BasePopover.Popup>
          </BasePopover.Positioner>
        </BasePopover.Portal>
      </BasePopover.Root>
  )

  if (!label) return control

  return (
    <label className="lrrg-datetime-field">
      <span>{label}</span>
      {control}
    </label>
  )
}

/** The calendar plus a time, editing one field. */
export function DateTimePanel({
  value,
  onChange,
  mode = "datetime",
}: {
  value: string
  onChange: (next: string) => void
  mode?: "date" | "datetime"
}) {
  const { t, i18n } = useTranslation()
  const locale = i18n.language.startsWith("zh") ? zhCN : enUS
  const parsed = (mode === "date" ? parseDate(value) : parseLocal(value)) ?? new Date()
  const [month, setMonth] = useState(
    () => new Date(parsed.getFullYear(), parsed.getMonth(), 1),
  )

  /// Month arithmetic outside the calendar, so our own arrows and the swipe gesture share one path.
  const [slide, setSlide] = useState<"next" | "prev" | null>(null)
  const touch = useRef<{ x: number; y: number } | null>(null)

  const stepMonth = (direction: -1 | 1) => {
    setSlide(direction === 1 ? "next" : "prev")
    setMonth((current) => {
      const next = new Date(current.getFullYear(), current.getMonth() + direction, 1)
      // Stay inside the dropdowns' own range, the same clamp the library applies.
      const first = new Date(parsed.getFullYear() - 5, 0)
      const last = new Date(parsed.getFullYear() + 1, 11)
      return next < first ? first : next > last ? last : next
    })
  }

  const onTouchStart = (event: React.TouchEvent) => {
    const point = event.touches[0]
    touch.current = point ? { x: point.clientX, y: point.clientY } : null
  }

  const onTouchEnd = (event: React.TouchEvent) => {
    const start = touch.current
    touch.current = null
    const point = event.changedTouches[0]
    if (!start || !point) return
    const dx = point.clientX - start.x
    const dy = point.clientY - start.y
    // Horizontal intent only: a vertical drag is the dialog scrolling, not a month change.
    if (Math.abs(dx) < 40 || Math.abs(dx) < Math.abs(dy)) return
    stepMonth(dx < 0 ? 1 : -1)
  }

  const write = (date: Date) =>
    onChange(mode === "date" ? formatDate(date) : formatLocal(date))

  const pickDay = (day: Date | undefined) => {
    if (!day) return
    write(new Date(day.getFullYear(), day.getMonth(), day.getDate(), parsed.getHours(), parsed.getMinutes()))
  }

  const pickTime = (hours: number, minutes: number) => {
    write(new Date(parsed.getFullYear(), parsed.getMonth(), parsed.getDate(), hours, minutes))
  }

  return (
    <div
      className="lrrg-datetime-body"
      onTouchStart={onTouchStart}
      onTouchEnd={onTouchEnd}
    >
      <div className="lrrg-datetime-nav">
        <button
          type="button"
          className="lrrg-nav-btn"
          aria-label={t("datetime.previousMonth")}
          onClick={() => stepMonth(-1)}
        >
          <i className="fas fa-chevron-left" aria-hidden="true" />
        </button>
        <button
          type="button"
          className="lrrg-nav-btn"
          aria-label={t("datetime.nextMonth")}
          onClick={() => stepMonth(1)}
        >
          <i className="fas fa-chevron-right" aria-hidden="true" />
        </button>
      </div>
      <div
        key={`${month.getFullYear()}-${month.getMonth()}`}
        className={`lrrg-datetime-month${slide ? ` lrrg-slide-${slide}` : ""}`}
      >
      <DayPicker
        mode="single"
        month={month}
        onMonthChange={(next) => {
          setSlide(next > month ? "next" : "prev")
          setMonth(next)
        }}
        selected={parsed}
        onSelect={pickDay}
        locale={locale}
        showOutsideDays
        weekStartsOn={1}
        // Month + year dropdowns, so jumping to another year is one click rather than twelve.
        captionLayout="dropdown"
        startMonth={new Date(parsed.getFullYear() - 5, 0)}
        endMonth={new Date(parsed.getFullYear() + 1, 11)}
        className="lrrg-daypicker"
        hideNavigation
      />
      </div>
      {mode === "date" && (
        <div className="lrrg-datetime-time">
          <button
            type="button"
            className="stdbtn lrrg-datetime-now"
            title={t("datetime.today")}
            aria-label={t("datetime.today")}
            onClick={() => onChange(formatDate(new Date()))}
          >
            <i className="fas fa-calendar-day" aria-hidden="true" />
          </button>
        </div>
      )}
      {mode === "datetime" && <div className="lrrg-datetime-time">
        <span className="lrrg-datetime-time-label">{t("datetime.time")}</span>
        <input
          className="stdinput lrrg-datetime-part"
          type="number"
          min={0}
          max={23}
          value={parsed.getHours()}
          onChange={(event) =>
            pickTime(Math.min(23, Math.max(0, Number(event.target.value))), parsed.getMinutes())
          }
        />
        <span>:</span>
        <input
          className="stdinput lrrg-datetime-part"
          type="number"
          min={0}
          max={59}
          value={parsed.getMinutes()}
          onChange={(event) =>
            pickTime(parsed.getHours(), Math.min(59, Math.max(0, Number(event.target.value))))
          }
        />
        <button
          type="button"
          className="stdbtn lrrg-datetime-now"
          title={t("datetime.now")}
          aria-label={t("datetime.now")}
          onClick={() => onChange(formatLocal(new Date()))}
        >
          <i className="fas fa-clock" aria-hidden="true" />
        </button>
      </div>}
    </div>
  )
}
