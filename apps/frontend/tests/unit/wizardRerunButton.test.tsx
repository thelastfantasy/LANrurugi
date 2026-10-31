import { fireEvent, render, screen } from "@testing-library/react"
import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import { MemoryRouter, Route, Routes, useSearchParams } from "react-router-dom"
import { beforeAll, describe, expect, it, vi } from "vitest"

import en from "@/i18n/locales/en.json"
import { WizardRerunButton } from "@/pages/Settings/WizardRerunButton"

const { useSessionMock } = vi.hoisted(() => ({ useSessionMock: vi.fn() }))
vi.mock("@/session/SessionProvider", () => ({ useSession: useSessionMock }))

beforeAll(async () => {
  await i18n.use(initReactI18next).init({
    resources: { en: { translation: en } },
    lng: "en",
    fallbackLng: "en",
    nsSeparator: false,
    keySeparator: false,
    interpolation: { escapeValue: false },
  })
})

/** `MemoryRouter` keeps its history in memory, so `window.location` never moves — the `from`
 *  marker has to be read back off the route it landed on instead. */
function WizardProbe() {
  const [params] = useSearchParams()
  return <div>the wizard:{params.get("from") ?? "none"}</div>
}

function renderButton(available: boolean) {
  useSessionMock.mockReturnValue({ firstRunWizardAvailable: available })
  return render(
    <MemoryRouter initialEntries={["/config"]}>
      <Routes>
        <Route path="/config" element={<WizardRerunButton />} />
        <Route path="/welcome" element={<WizardProbe />} />
      </Routes>
    </MemoryRouter>,
  )
}

describe("WizardRerunButton", () => {
  it("offers the re-run and navigates into the wizard, with the from=settings marker", () => {
    renderButton(true)

    fireEvent.click(screen.getByRole("button", { name: "Run setup wizard again" }))

    // `?from=settings` is what makes the wizard hand the user back to Settings when it ends.
    expect(screen.getByText("the wizard:settings")).toBeInTheDocument()
  })

  it("renders nothing on a deployment that switched the wizard off", () => {
    const { container } = renderButton(false)
    expect(container).toBeEmptyDOMElement()
  })
})
