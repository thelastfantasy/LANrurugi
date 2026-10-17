import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import { MemoryRouter, Route, Routes } from "react-router-dom"
import { beforeAll, describe, expect, it, vi } from "vitest"

import en from "@/i18n/locales/en.json"
import { FirstRunWizard } from "@/pages/FirstRunWizard"

const { changePasswordMock, markDoneMock, useSessionMock, wizardState } = vi.hoisted(() => ({
  changePasswordMock: vi.fn(async () => ({})),
  markDoneMock: vi.fn(async () => ({})),
  useSessionMock: vi.fn(),
  /** What the instance already has handled, which is what decides the ticks on the step tabs. */
  wizardState: { llmKeySet: false, importCount: 0 },
}))

vi.mock("@/session/SessionProvider", () => ({ useSession: useSessionMock }))
// Partial rather than a full replacement: `@/i18n` and several transitive imports pull other
// exports off this module, and only the ones the wizard actually reads need stubbing.
vi.mock("@/api/hooks", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/api/hooks")>()),
  useSettings: () => ({
    data: { llm_api_key_set: wizardState.llmKeySet, language: "auto", timezone: "UTC" },
  }),
  useImportLegacyCount: () => ({ data: { import_count: wizardState.importCount } }),
  useUpdateSettings: () => ({ mutateAsync: markDoneMock }),
  useChangePassword: () => ({ mutateAsync: changePasswordMock }),
}))
// Calls `useSettings()` for the site title, which is unrelated to what this file asserts.
vi.mock("@/hooks/useDocumentTitle", () => ({ useDocumentTitle: () => {} }))

/** i18next's real config in `@/i18n` is only initialized as a side effect of a particular import
 *  graph, which this test does not share — and its `keySeparator: false` is load-bearing (every
 *  key here contains dots). Initialized explicitly so the queries below can match the real English
 *  strings, which is the point: these are the same accessible names the Playwright spec uses. */
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

function renderWizard({
  available = true,
  guest = false,
  llmKeySet = false,
  importCount = 0,
}: {
  /** What the *server* reports for `first_run_wizard_available` (the deploy-time switch). */
  available?: boolean
  guest?: boolean
  llmKeySet?: boolean
  importCount?: number
} = {}) {
  wizardState.llmKeySet = llmKeySet
  wizardState.importCount = importCount
  const isAuthenticated = !guest
  useSessionMock.mockReturnValue({
    status: undefined,
    isSuccess: true,
    isError: false,
    isPending: false,
    isFetching: false,
    isAuthenticated,
    isGuestModeEnabled: guest,
    isGuest: guest,
    usingDefaultPassword: true,
    // Mirrors `SessionProvider`'s own derivation, which ANDs the server's field with
    // `isAuthenticated` so a stale or hand-crafted payload can never route a non-admin in.
    firstRunWizardAvailable: isAuthenticated && available,
    firstRunWizardPending: isAuthenticated && available,
    refresh: vi.fn(),
  })
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } })
  return render(
    <QueryClientProvider client={queryClient}>
      {/* A real route for the library too, so the unavailable branch's `<Navigate>` has somewhere
          to land that a test can observe. */}
      <MemoryRouter initialEntries={["/welcome"]}>
        <Routes>
          <Route path="/welcome" element={<FirstRunWizard />} />
          <Route path="/" element={<div>the library</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>,
  )
}

/** `fireEvent`, not a raw `.click()`: the latter dispatches outside React's act() scope, so the
 *  render triggered by the click has not been flushed by the next assertion. */
function click(name: string | RegExp) {
  fireEvent.click(screen.getByRole("button", { name }))
}

describe("FirstRunWizard", () => {
  it("walks all five steps with the wizard-wide skip present until the last one", () => {
    renderWizard()

    expect(screen.getByRole("heading", { name: "First-Time Setup" })).toBeInTheDocument()
    // The escape hatch on every step bar the last, so the wizard can never trap a user.
    expect(
      screen.getByRole("button", { name: "Skip setup and start using LANrurugi" }),
    ).toBeInTheDocument()

    click("Next")
    expect(screen.getByRole("heading", { name: "Admin password" })).toBeInTheDocument()

    click("Skip this step")
    expect(screen.getByRole("heading", { name: "DeepSeek API key" })).toBeInTheDocument()

    click("Skip this step")
    expect(screen.getByText("Import from a LANraragi backup")).toBeInTheDocument()

    click("Next")
    expect(screen.getByRole("heading", { name: "You're all set" })).toBeInTheDocument()
    // Finishing is the only way out of the last step.
    expect(
      screen.queryByRole("button", { name: "Skip setup and start using LANrurugi" }),
    ).toBeNull()
  })

  it("marks the instance handled when the wizard is finished, then leaves", async () => {
    markDoneMock.mockClear()
    renderWizard()

    click("Next")
    click("Skip this step")
    click("Skip this step")
    click("Next")
    click("Submit and go to the library")

    await waitFor(() => expect(markDoneMock).toHaveBeenCalledWith({ first_run_wizard_done: true }))
  })

  it("lets the user jump to any step at any time, in either direction", () => {
    renderWizard()

    // Straight to the last step from the first, without walking through the middle.
    click("Done")
    expect(screen.getByRole("heading", { name: "You're all set" })).toBeInTheDocument()

    click("Basics")
    expect(screen.getByRole("heading", { name: "Admin password" })).toBeInTheDocument()
  })

  it("ticks the steps whose items are already handled when the wizard opens", () => {
    // The instance under test already has a DeepSeek key and has imported before, so those two
    // tabs carry a tick before the user has done anything here.
    renderWizard({ llmKeySet: true, importCount: 1 })

    const ticked = (name: string) =>
      screen.getByRole("button", { name }).querySelector(".wizard-step-done") !== null
    expect(ticked("DeepSeek Key")).toBe(true)
    expect(ticked("Import")).toBe(true)
    // Still on the default password, and the wizard has not run, so neither is ticked.
    expect(ticked("Basics")).toBe(false)
    expect(ticked("Done")).toBe(false)
  })

  it("ticks a step once its item is genuinely done, not merely visited", () => {
    renderWizard()

    click("Next")
    click("Skip this step")
    click("Skip this step")
    // Passed through both items without doing either of them.
    expect(screen.queryByRole("button", { name: "Basics" })?.querySelector(".wizard-step-done")).toBeNull()
    expect(screen.queryByRole("button", { name: "DeepSeek Key" })?.querySelector(".wizard-step-done")).toBeNull()
  })

  it("keeps a half-typed step's draft alive across jumps to other steps", () => {
    renderWizard()

    click("Basics")
    fireEvent.change(screen.getByLabelText("New Password"), { target: { value: "hunter2" } })
    fireEvent.change(screen.getByLabelText("New Password Confirmation"), {
      target: { value: "hunter2" },
    })

    // Wander off to the last step and back — the draft has to survive the step unmounting.
    click("Done")
    click("Basics")

    expect(screen.getByLabelText("New Password")).toHaveValue("hunter2")
    expect(screen.getByLabelText("New Password Confirmation")).toHaveValue("hunter2")
  })

  it("treats an untouched step as nothing to submit, not as an empty password", async () => {
    changePasswordMock.mockClear()
    renderWizard()

    click("Next")
    click("Next") // Next on the password step, with both fields blank.

    expect(await screen.findByRole("heading", { name: "DeepSeek API key" })).toBeInTheDocument()
    // Submitting an empty string would *clear* the stored password server-side, so an untouched
    // step must not reach the endpoint at all.
    expect(changePasswordMock).not.toHaveBeenCalled()
  })

  it("refuses to advance on a half-filled password rather than dropping what was typed", async () => {
    changePasswordMock.mockClear()
    renderWizard()

    click("Basics")
    fireEvent.change(screen.getByLabelText("New Password"), { target: { value: "hunter2" } })
    click("Next")

    expect(await screen.findByRole("alert")).toHaveTextContent("Passwords don't match!")
    expect(changePasswordMock).not.toHaveBeenCalled()
    expect(screen.getByRole("heading", { name: "Admin password" })).toBeInTheDocument()
  })

  it("submits the step's own draft from its Next, and only once", async () => {
    changePasswordMock.mockClear()
    renderWizard()

    click("Basics")
    fireEvent.change(screen.getByLabelText("New Password"), { target: { value: "hunter2" } })
    fireEvent.change(screen.getByLabelText("New Password Confirmation"), {
      target: { value: "hunter2" },
    })
    click("Next")

    await waitFor(() => expect(changePasswordMock).toHaveBeenCalledWith("hunter2"))
    expect(await screen.findByRole("heading", { name: "DeepSeek API key" })).toBeInTheDocument()
    expect(changePasswordMock).toHaveBeenCalledTimes(1)
  })

  it("has the last page submit only what no step already sent", async () => {
    changePasswordMock.mockClear()
    markDoneMock.mockClear()
    renderWizard()

    // A key typed on its own step but never sent from there: the catch-all is what has to save it.
    click("DeepSeek Key")
    fireEvent.change(screen.getByLabelText("DeepSeek API Key"), {
      target: { value: "sk-typed-not-sent" },
    })

    click("Done")
    click("Submit and go to the library")

    await waitFor(() =>
      expect(markDoneMock).toHaveBeenCalledWith({ llm_api_key: "sk-typed-not-sent" }),
    )
    // Nothing was typed on the password step, so it must not be written (and must not be cleared).
    expect(changePasswordMock).not.toHaveBeenCalled()
  })

  it("names the offending step when the catch-all submit cannot send a draft", async () => {
    markDoneMock.mockClear()
    renderWizard()

    // Only the first of the two password fields filled in, then straight to the last page: the
    // catch-all has to refuse, and say which tab the half-typed draft came from.
    click("Basics")
    fireEvent.change(screen.getByLabelText("New Password"), { target: { value: "hunter2" } })
    click("Done")
    click("Submit and go to the library")

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Basics could not be submitted: Passwords don't match!",
    )
    // Refusing to finish is the point: finishing here would drop the draft silently.
    expect(markDoneMock).not.toHaveBeenCalledWith({ first_run_wizard_done: true })
  })

  it("carries the language and timezone settings on the same page as the password", () => {
    renderWizard()

    click("Basics")
    expect(screen.getByRole("heading", { name: "Admin password" })).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "Language" })).toBeInTheDocument()
    expect(screen.getByRole("heading", { name: "Timezone" })).toBeInTheDocument()
    // The Settings page's full editor, not the sidebar's one-click selector: the automatic switch
    // plus (once it is off) the ordered fallback list.
    expect(screen.getByRole("checkbox", { name: "Automatic (browser default)" })).toBeChecked()
    // The timezone picker defaults to the server's own value rather than to a blank field.
    expect(screen.getByRole("combobox", { name: "Timezone" })).toHaveValue("UTC")
  })

  it("treats the language order as a draft too, committed by Next rather than on each click", async () => {
    markDoneMock.mockClear()
    renderWizard()

    click("Basics")
    fireEvent.click(screen.getByRole("checkbox", { name: "Automatic (browser default)" }))

    // Turning the automatic switch off seeds the list, and nothing has been written yet.
    expect(await screen.findByRole("button", { name: "Move up" })).toBeInTheDocument()
    expect(markDoneMock).not.toHaveBeenCalledWith({ language: "en" })

    click("Next")
    await waitFor(() => expect(markDoneMock).toHaveBeenCalledWith({ language: "en" }))
  })

  it("submits the timezone draft from Next, and keeps it across jumps until then", async () => {
    markDoneMock.mockClear()
    renderWizard()

    click("Basics")
    fireEvent.change(screen.getByRole("combobox", { name: "Timezone" }), {
      target: { value: "Asia/Tokyo" },
    })

    // Wander away without submitting: the pick has to survive, and must not have been written yet.
    click("Done")
    expect(markDoneMock).not.toHaveBeenCalledWith({ timezone: "Asia/Tokyo" })
    click("Basics")

    click("Next")
    await waitFor(() => expect(markDoneMock).toHaveBeenCalledWith({ timezone: "Asia/Tokyo" }))
  })

  it("reports the default password as still in place, from the session's own signal", () => {
    renderWizard()

    click("Next")
    expect(
      screen.getByText("Still using the default password — anyone can log in."),
    ).toBeInTheDocument()
  })

  it("never renders for a guest, even on a guest-mode deployment", () => {
    // The guest-visible half of a guest-mode instance is `/` and the reader; `/welcome` edits the
    // admin password and the LLM key, so it is admin-only twice over — `RequireAuth` at the route
    // (covered by routeGuards.test.tsx) and this component's own refusal below. A guest reaching
    // the URL by hand gets sent to the library they are actually allowed to see, not the wizard.
    renderWizard({ guest: true })

    expect(screen.getByText("the library")).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "First-Time Setup" })).toBeNull()
    expect(
      screen.queryByRole("button", { name: "Skip setup and start using LANrurugi" }),
    ).toBeNull()
  })

  it("bounces straight to the library on a deployment that switched the wizard off", () => {
    renderWizard({ available: false })

    // No wizard chrome at all — a stale bookmark or hand-typed `/welcome` gets nothing, rather
    // than a flow whose "finish" would write a marker nothing reads.
    expect(screen.getByText("the library")).toBeInTheDocument()
    expect(screen.queryByRole("heading", { name: "First-Time Setup" })).toBeNull()
    expect(
      screen.queryByRole("button", { name: "Skip setup and start using LANrurugi" }),
    ).toBeNull()
  })
})
