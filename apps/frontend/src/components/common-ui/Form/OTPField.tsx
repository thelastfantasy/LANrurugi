import { useEffect, useId, useRef, useState } from "react"

/** Segmented one-time-code input shared by every four-digit entry in the app (the safety password
 * dialog and its Settings equivalent).
 *
 * Built on **one** real `<input>` behind four presentational slots, not one input per digit:
 * moving focus from slot to slot dismisses the soft keyboard on Android browsers after every
 * character, which made the four-digit entry unusable on a phone. A single input keeps the
 * keyboard up and still supports paste of a whole code; the slots only render what it holds.
 * `mask` defaults to on (the slots show dots), because this component only ever carries a
 * credential. Every colour/size lives in the `.otp-field*` classes in the five theme files. */
export function OTPField({
  length = 4,
  value,
  onValueChange,
  onComplete,
  label,
  slotLabel,
  describedById,
  mask = true,
  disabled = false,
  invalid = false,
  autoFocus = false,
  id,
  focusSignal,
  align = "center",
}: {
  length?: number
  value: string
  onValueChange: (value: string) => void
  onComplete?: (value: string) => void
  label?: string
  /** Localized per-slot description, e.g. `(2, 4) => "第 2 位，共 4 位"`. */
  slotLabel: (index: number, total: number) => string
  describedById?: string
  mask?: boolean
  disabled?: boolean
  invalid?: boolean
  autoFocus?: boolean
  id?: string
  /** Bump this after a failed submission to put the caret back in the field. */
  focusSignal?: number
  /** `center` (dialogs) or `start` (a settings row, where the label sits above the field's left edge). */
  align?: "center" | "start"
}) {
  const generatedId = useId()
  const rootId = id ?? generatedId
  const inputRef = useRef<HTMLInputElement | null>(null)
  const [focused, setFocused] = useState(false)

  useEffect(() => {
    if (!autoFocus) return
    // Deferred: the surrounding modal's own focus handling runs after this component mounts.
    const handle = window.setTimeout(() => inputRef.current?.focus(), 50)
    return () => window.clearTimeout(handle)
  }, [autoFocus])

  useEffect(() => {
    if (!focusSignal) return
    const handle = window.setTimeout(() => inputRef.current?.focus(), 0)
    return () => window.clearTimeout(handle)
  }, [focusSignal])

  function handleChange(next: string) {
    const digits = next.replace(/\D/g, "").slice(0, length)
    onValueChange(digits)
    if (digits.length === length && digits !== value) onComplete?.(digits)
  }

  const chars = Array.from({ length }, (_, index) => value[index] ?? "")
  const activeIndex = Math.min(value.length, length - 1)

  return (
    <div className={align === "start" ? "otp-field-wrap is-start" : "otp-field-wrap"}>
      {label && (
        <label className="otp-field-label" htmlFor={rootId}>
          {label}
        </label>
      )}
      <div className="otp-field" data-invalid={invalid ? "true" : undefined}>
        {chars.map((char, index) => (
          <div
            key={index}
            className={`otp-field-slot${focused && index === activeIndex ? " is-active" : ""}`}
            aria-hidden="true"
          >
            {char ? (mask ? "•" : char) : ""}
          </div>
        ))}
        <input
          ref={inputRef}
          id={rootId}
          className="otp-field-input"
          type={mask ? "password" : "text"}
          inputMode="numeric"
          autoComplete="one-time-code"
          pattern="[0-9]*"
          maxLength={length}
          value={value}
          disabled={disabled}
          aria-label={label ?? slotLabel(0, length)}
          aria-describedby={describedById}
          aria-invalid={invalid || undefined}
          onChange={(event) => handleChange(event.target.value)}
          onFocus={() => setFocused(true)}
          onBlur={() => setFocused(false)}
        />
      </div>
    </div>
  )
}
