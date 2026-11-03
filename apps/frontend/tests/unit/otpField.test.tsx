import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { useState } from "react"
import { describe, expect, it, vi } from "vitest"

import { OTPField } from "@/components/common-ui/Form/OTPField"

function Harness({
  disabled,
  invalid,
  onComplete,
  focusSignal,
  mask,
}: {
  disabled?: boolean
  invalid?: boolean
  onComplete?: (value: string) => void
  focusSignal?: number
  mask?: boolean
}) {
  const [value, setValue] = useState("")
  return (
    <>
      <OTPField
        value={value}
        onValueChange={setValue}
        onComplete={onComplete}
        label="Safety password"
        slotLabel={(index, total) => `Digit ${index + 1} of ${total}`}
        disabled={disabled}
        invalid={invalid}
        focusSignal={focusSignal}
        mask={mask}
      />
      <span data-testid="value">{value}</span>
    </>
  )
}

function input(container: HTMLElement): HTMLInputElement {
  const el = container.querySelector<HTMLInputElement>("input.otp-field-input")
  if (!el) throw new Error("no OTP input")
  return el
}

describe("OTPField", () => {
  it("renders four slots behind a single labelled input", () => {
    const { container } = render(<Harness />)
    expect(container.querySelectorAll(".otp-field-slot")).toHaveLength(4)
    // One real input keeps the mobile keyboard up; it is the labelled control.
    expect(container.querySelectorAll("input")).toHaveLength(1)
    expect(screen.getByLabelText("Safety password")).toBe(input(container))
  })

  it("accepts only digits and reports the whole value", () => {
    const { container } = render(<Harness />)
    fireEvent.change(input(container), { target: { value: "1a2b" } })
    expect(screen.getByTestId("value").textContent).toBe("12")
  })

  it("fires onComplete once all slots are filled", () => {
    const onComplete = vi.fn()
    const { container } = render(<Harness onComplete={onComplete} />)
    fireEvent.change(input(container), { target: { value: "1234" } })
    expect(onComplete).toHaveBeenCalledWith("1234")
  })

  it("masks the digits by default", () => {
    const { container } = render(<Harness />)
    expect(input(container)).toHaveAttribute("type", "password")
    fireEvent.change(input(container), { target: { value: "12" } })
    const slots = Array.from(container.querySelectorAll(".otp-field-slot")).map((s) => s.textContent)
    expect(slots).toEqual(["•", "•", "", ""])
  })

  it("returns focus to the input when focusSignal changes", async () => {
    const view = (signal: number) => (
      <>
        <Harness focusSignal={signal} />
        <button type="button" data-testid="confirm">
          confirm
        </button>
      </>
    )
    const { container, rerender } = render(view(0))
    screen.getByTestId("confirm").focus()
    expect(document.activeElement).toBe(screen.getByTestId("confirm"))
    rerender(view(1))
    await waitFor(() => expect(document.activeElement).toBe(input(container)))
  })

  it("propagates disabled and invalid state", () => {
    const { container } = render(<Harness disabled invalid />)
    expect(container.querySelector(".otp-field")).toHaveAttribute("data-invalid", "true")
    expect(input(container)).toBeDisabled()
  })
})
