// The two number-field variants behind every numeric input in the app: both must render the same
// affordance (a real `type="number"` with the native spinner suppressed and this project's own
// stepper), and they must differ in exactly one way — whether "empty" is a value the field can hold.
import "@/i18n"

import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

import { NullableNumberInput, NumberInput } from "@/components/common-ui/Form/NumberInput"

/** The stepper both variants render, with the icons that make it unmistakably ours rather than the
 *  browser's `::-webkit-inner-spin-button`. */
function expectStepper(input: HTMLElement) {
  const wrapper = input.closest(".number-input-wrapper")
  expect(wrapper).toBeTruthy()
  const stepper = wrapper!.querySelector(".number-input-stepper")
  expect(stepper).toBeTruthy()
  const icons = Array.from(stepper!.querySelectorAll("i")).map((i) => i.className)
  expect(icons).toEqual(["fas fa-caret-up", "fas fa-caret-down"])
}

describe("NumberInput", () => {
  it("renders a number field with the native spinner suppressed and this project's stepper", () => {
    render(<NumberInput value={10} onValueChange={vi.fn()} min={1} />)
    const input = screen.getByRole("spinbutton") as HTMLInputElement

    expect(input.type).toBe("number")
    expect(input.value).toBe("10")
    expect(input.min).toBe("1")
    // Without this class the browser draws its own spinner over the hand-rolled one — the exact
    // 21px-row overflow `index.css` documents.
    expect(input.className).toContain("number-input-no-native-spinner")
    expectStepper(input)
  })

  it("reports typing as a number and ignores an emptied box", () => {
    const onValueChange = vi.fn()
    render(<NumberInput value={3} onValueChange={onValueChange} />)
    const input = screen.getByRole("spinbutton")

    fireEvent.change(input, { target: { value: "7" } })
    expect(onValueChange).toHaveBeenCalledWith(7)

    // A controlled `number` field has no empty state: the value it was given stays on screen.
    onValueChange.mockClear()
    fireEvent.change(input, { target: { value: "" } })
    expect(onValueChange).not.toHaveBeenCalled()
  })
})

describe("NullableNumberInput", () => {
  it("renders an empty box for the empty value rather than a stand-in zero", () => {
    render(<NullableNumberInput value="" onValueChange={vi.fn()} min={0} />)
    const input = screen.getByRole("spinbutton") as HTMLInputElement

    expect(input.value).toBe("")
    // The whole reason this variant exists: a per-domain limit or a usage cap that has never been
    // set must not be written back as 0 (the host rejects a 0 concurrency limit outright).
    expectStepper(input)
  })

  it("reports both a typed number and a freshly emptied box", () => {
    const onValueChange = vi.fn()
    render(<NullableNumberInput value={10} onValueChange={onValueChange} />)
    const input = screen.getByRole("spinbutton")

    fireEvent.change(input, { target: { value: "4" } })
    expect(onValueChange).toHaveBeenCalledWith(4)

    onValueChange.mockClear()
    fireEvent.change(input, { target: { value: "" } })
    expect(onValueChange).toHaveBeenCalledWith("")
  })

  it("keeps a fractional value fractional (a byte-rate field takes decimals)", () => {
    const onValueChange = vi.fn()
    render(<NullableNumberInput value={1.5} onValueChange={onValueChange} step="any" />)
    const input = screen.getByRole("spinbutton") as HTMLInputElement

    expect(input.step).toBe("any")
    fireEvent.change(input, { target: { value: "2.25" } })
    expect(onValueChange).toHaveBeenCalledWith(2.25)
  })
})
