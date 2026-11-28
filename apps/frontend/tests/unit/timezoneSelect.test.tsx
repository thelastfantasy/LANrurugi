import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { fireEvent, render, screen } from "@testing-library/react"
import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import { beforeAll, describe, expect, it, vi } from "vitest"

import { TimezoneSelect } from "@/components/Form"
import en from "@/i18n/locales/en.json"

beforeAll(async () => {
  await i18n.use(initReactI18next).init({ lng: "en", resources: { en: { translation: en } } })
})

async function search(query: string) {
  // The popup's colors come from the theme palette, which reads the server settings.
  render(
    <QueryClientProvider client={new QueryClient()}>
      <TimezoneSelect value="UTC" onChange={vi.fn()} ariaLabel="Timezone" />
    </QueryClientProvider>,
  )
  fireEvent.click(screen.getByRole("combobox", { name: "Timezone" }))
  fireEvent.change(await screen.findByPlaceholderText(/Search by name or offset/), { target: { value: query } })
}

describe("TimezoneSelect", () => {
  it("shows each zone with its UTC offset", async () => {
    // Tehran: a half-hour offset with no DST, under the same id in every engine (Kolkata is listed as
    // Asia/Calcutta by some).
    await search("tehran")
    expect(await screen.findByRole("option", { name: "Asia/Tehran +0330" })).toBeInTheDocument()
  })

  it("finds zones by offset, with or without the colon", async () => {
    await search("+03:30")
    expect(await screen.findByRole("option", { name: /^Asia\/Tehran/ })).toBeInTheDocument()
    expect(screen.queryByRole("option", { name: /^Asia\/Tokyo/ })).not.toBeInTheDocument()
  })

  it("reads an underscore as a space", async () => {
    await search("new york")
    expect(await screen.findByRole("option", { name: /^America\/New_York/ })).toBeInTheDocument()
  })

  it("says so when nothing matches", async () => {
    await search("no such zone")
    expect(await screen.findByText("No matching timezone")).toBeInTheDocument()
  })
})
