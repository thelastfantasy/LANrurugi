import { useRef } from "react"

import { Input } from "@/components/common-ui/Form/Input"

/**
 * A `.stdinput`-styled `type="number"` input with its own up/down stepper — not the browser's
 * native `::-webkit-inner-spin-button`, which this project's `.stdinput` rows render at a fixed,
 * often quite short, height (21px, or narrower still in a few callers like the reader's "preload
 * count" field). The native spinner doesn't shrink to fit a short input — it keeps its own
 * browser-default size and visually overflows/misaligns against the input's edge at these heights.
 * The native control is hidden via `.number-input-no-native-spinner` (`index.css` — a real
 * stylesheet rule, since `::-webkit-inner-spin-button` is a pseudo-element an inline `style` can't
 * reach at all), and this component's own two-button stepper takes its place.
 *
 * Deliberately *not* built on `InputGroup` (unlike this project's other icon-in-input compositions):
 * `InputGroup`'s end slot is a fixed `slotSize` box, sized once by whatever pixel value the caller
 * passes — but this component is used at several different `.stdinput` row heights across the app
 * (21px default, 25px in the reader's settings overlay, etc.), and a fixed slot can't track that
 * automatically. The stepper here instead absolutely positions itself against the wrapper's own
 * height (`top`/`bottom: 0`), so each button is always exactly half the *actual* rendered input
 * height, at any height a caller renders this at, with no per-caller size prop to keep in sync.
 * `border-box` sizing throughout keeps that at-most-half-including-border, so the stepper's outer
 * edge lines up with the input's own border no matter which height it ends up at.
 *
 * The stepper is hidden until hover/focus-within (`.number-input-stepper` — `index.css`), matching
 * the native spinner's own hover-to-reveal behaviour rather than permanently overlaying two extra
 * buttons on every number field on screen.
 *
 * Still a real `<input type="number">` underneath (not reimplemented as text + custom parsing) —
 * the buttons call the element's own native `stepUp()`/`stepDown()` (MDN: `HTMLInputElement.
 * stepUp`/`stepDown`), which already handles `min`/`max` clamping and `step` rounding exactly like
 * the native spinner would, then dispatch a real `input` event so React's controlled `value` picks
 * up the change the same way it would from typing.
 */
export function NumberInput({
  value,
  onValueChange,
  min,
  max,
  step = 1,
  className,
  style,
  ...props
}: {
  value: number
  onValueChange: (value: number) => void
  min?: number
  max?: number
  step?: number
  className?: string
  style?: React.CSSProperties
} & Omit<React.InputHTMLAttributes<HTMLInputElement>, "value" | "onChange" | "type" | "min" | "max" | "step" | "className" | "style">) {
  const inputRef = useRef<HTMLInputElement>(null)

  const step_ = (direction: 1 | -1) => {
    const el = inputRef.current
    if (!el) return
    if (direction === 1) el.stepUp()
    else el.stepDown()
    // `stepUp`/`stepDown` set `.value` directly without dispatching any event — fire one so this
    // controlled input's `onValueChange` (and thus the caller's own state) picks up the change,
    // the same as if the user had typed it.
    el.dispatchEvent(new Event("input", { bubbles: true }))
  }

  return (
    <span className="number-input-wrapper" style={{ position: "relative", display: "inline-block", ...style }}>
      <Input
        ref={inputRef}
        type="number"
        min={min}
        max={max}
        step={step}
        className={`number-input-no-native-spinner${className ? ` ${className}` : ""}`}
        style={{ width: "100%", boxSizing: "border-box", paddingRight: 16 }}
        value={String(value)}
        onValueChange={(next) => {
          if (next === "" || next === "-") return
          const parsed = Number(next)
          if (Number.isFinite(parsed)) onValueChange(parsed)
        }}
        {...props}
      />
      <span className="number-input-stepper">
        <button
          type="button"
          className="number-input-stepper__btn number-input-stepper__btn--up"
          aria-label="increment"
          tabIndex={-1}
          onClick={() => step_(1)}
        >
          <i className="fas fa-caret-up" aria-hidden="true" />
        </button>
        <button
          type="button"
          className="number-input-stepper__btn number-input-stepper__btn--down"
          aria-label="decrement"
          tabIndex={-1}
          onClick={() => step_(-1)}
        >
          <i className="fas fa-caret-down" aria-hidden="true" />
        </button>
      </span>
    </span>
  )
}
