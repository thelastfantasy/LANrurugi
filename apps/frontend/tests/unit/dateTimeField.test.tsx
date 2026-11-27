import { fireEvent, render, screen } from "@testing-library/react"
import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import { beforeAll, describe, expect, it, vi } from "vitest"

import { DateTimePanel } from "@/components/common-ui/Form/DateTimeField"
import en from "@/i18n/locales/en.json"

beforeAll(async () => {
  await i18n.use(initReactI18next).init({ lng: "en", resources: { en: { translation: en } } })
})

describe("DateTimePanel", () => {
  it("picks the already-selected day instead of clearing the value", () => {
    // Regression: react-day-picker's single mode reports `undefined` for a repeat tap on the
    // selected day unless `required` is set, which left the current date feeling unclickable.
    const onChange = vi.fn()
    render(<DateTimePanel value="2026-11-26" onChange={onChange} mode="date" />)

    fireEvent.click(screen.getByRole("button", { name: /November 26/ }))

    expect(onChange).toHaveBeenCalledWith("2026-11-26")
  })

  it("picks a different day from the same month", () => {
    const onChange = vi.fn()
    render(<DateTimePanel value="2026-11-26" onChange={onChange} mode="date" />)

    fireEvent.click(screen.getByRole("button", { name: /November 20/ }))

    expect(onChange).toHaveBeenCalledWith("2026-11-20")
  })
})
