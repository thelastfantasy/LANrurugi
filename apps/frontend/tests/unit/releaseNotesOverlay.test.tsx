import { fireEvent, render, screen } from "@testing-library/react"
import i18n from "i18next"
import { initReactI18next } from "react-i18next"
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest"

import type { ChangelogRelease } from "@/api/types"
import {
  formatReleaseDate,
  ReleaseNotesOverlay,
  renderReleaseBody,
} from "@/components/Layout/ReleaseNotesOverlay"
import en from "@/i18n/locales/en.json"

const { useReleaseNotesMock, useSessionMock } = vi.hoisted(() => ({
  useReleaseNotesMock: vi.fn(),
  useSessionMock: vi.fn(),
}))

vi.mock("@/api/hooks", () => ({ useReleaseNotes: useReleaseNotesMock }))
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

const NEWER: ChangelogRelease = {
  tag: "v0.2.0",
  sha: "bbbbbbbcccccccddddddd",
  published_at: "2026-10-14T09:30:00Z",
  url: "https://example.test/releases/v0.2.0",
  body: "## Fixes\n\n- **bold** fix\n- see [#12](https://example.test/12)\n\n<script>alert(1)</script>",
}

const OLDER: ChangelogRelease = {
  tag: "v0.1.0",
  sha: null,
  published_at: null,
  url: null,
  body: "Older notes",
}

function renderOverlay(
  entries: ChangelogRelease[] | null,
  session: { isAuthenticated?: boolean; firstRunWizardPending?: boolean } = {},
) {
  useSessionMock.mockReturnValue({
    isAuthenticated: true,
    firstRunWizardPending: false,
    ...session,
  })
  // Mirrors the real hook: a disabled query never resolves, so it has no data. Returning the
  // entries regardless would make the disabled-query gate untestable — and would have hidden the
  // fact that the overlay now refuses on its own rather than relying on an empty cache.
  useReleaseNotesMock.mockImplementation((enabled: boolean) => ({
    data: enabled ? entries : undefined,
  }))
  return render(<ReleaseNotesOverlay />)
}

describe("ReleaseNotesOverlay", () => {
  beforeEach(() => {
    useReleaseNotesMock.mockReset()
    useSessionMock.mockReset()
  })

  it("renders nothing when there is no upgrade to report", () => {
    const { container } = renderOverlay(null)
    expect(container).toBeEmptyDOMElement()
    expect(screen.queryByText("New Version Release Notes")).toBeNull()
  })

  it("renders nothing for an empty changelog", () => {
    const { container } = renderOverlay([])
    expect(container).toBeEmptyDOMElement()
  })

  it("heads every section with its version, date and short hash, newest first", () => {
    renderOverlay([NEWER, OLDER])

    const sections = [...document.querySelectorAll("#changelog section")]
    expect(sections.map((s) => s.querySelector("h3")?.textContent)).toEqual(["v0.2.0", "v0.1.0"])

    // Asserted on the section's whole text rather than via `getByText`: the date and hash share one
    // line with a separator, so neither is an element's exact text on its own.
    expect(sections[0].textContent).toContain("2026-10-14")
    expect(sections[0].textContent).toContain("bbbbbbb")
    // A release the tags list does not know still renders — version and body, with a dash where the
    // date and hash would be rather than "Invalid Date" or an empty cell.
    expect(sections[1].textContent).toContain("—")
    expect(sections[1].textContent).toContain("Older notes")
  })

  it("renders the release body as markdown and strips anything dangerous from it", () => {
    renderOverlay([NEWER])

    expect(screen.getByRole("heading", { name: "Fixes" })).toBeInTheDocument()
    expect(screen.getByText("bold").tagName).toBe("STRONG")
    expect(screen.queryByText("alert(1)")).toBeNull()
  })

  it("opens release-note links in a new tab so the modal is not navigated away from", () => {
    renderOverlay([NEWER])

    const link = screen.getByRole("link", { name: "#12" })
    expect(link).toHaveAttribute("target", "_blank")
    expect(link).toHaveAttribute("rel", "noreferrer")
  })

  it("never asks for a changelog on behalf of a guest, and renders nothing", () => {
    // `Layout` renders for an anonymous visitor on a guest-mode library too, so the admin-only
    // condition has to be explicit here. The query is disabled rather than merely hidden, so a
    // guest's browser does not even request release notes.
    const { container } = renderOverlay([NEWER], { isAuthenticated: false })

    expect(useReleaseNotesMock).toHaveBeenCalledWith(false)
    expect(container).toBeEmptyDOMElement()
  })

  it("waits for the first-run wizard instead of stacking on top of it", () => {
    const { container } = renderOverlay([NEWER], { firstRunWizardPending: true })

    expect(useReleaseNotesMock).toHaveBeenCalledWith(false)
    expect(container).toBeEmptyDOMElement()
  })

  it("asks for the notes once an authenticated admin is past the wizard", () => {
    renderOverlay([NEWER])

    expect(useReleaseNotesMock).toHaveBeenCalledWith(true)
    expect(screen.getByText("New Version Release Notes")).toBeInTheDocument()
  })

  it("closes on the close button, on Escape, and on a shade click", () => {
    renderOverlay([NEWER])
    fireEvent.click(screen.getByRole("button", { name: "Close" }))
    expect(screen.queryByText("New Version Release Notes")).toBeNull()

    renderOverlay([NEWER])
    fireEvent.keyDown(document, { key: "Escape" })
    expect(screen.queryByText("New Version Release Notes")).toBeNull()

    const { container } = renderOverlay([NEWER])
    fireEvent.click(container.querySelector("#overlay-shade")!)
    expect(screen.queryByText("New Version Release Notes")).toBeNull()
  })
})

describe("formatReleaseDate", () => {
  it("renders the UTC publish date, not the reader's local one", () => {
    // A release published at 23:30Z would read as the previous or next day in most timezones; the
    // date GitHub published it is the one that belongs in the header.
    expect(formatReleaseDate("2026-10-14T23:30:00Z")).toBe("2026-10-14")
  })

  it("falls back to a dash rather than showing an invalid date", () => {
    expect(formatReleaseDate(null)).toBe("—")
    expect(formatReleaseDate("")).toBe("—")
    expect(formatReleaseDate("last tuesday")).toBe("—")
  })
})

describe("renderReleaseBody", () => {
  it("strips a javascript: href as well as a script tag", () => {
    const html = renderReleaseBody("[click](javascript:alert(1))\n\n<script>alert(2)</script>")
    expect(html).not.toContain("javascript:")
    expect(html).not.toContain("<script")
  })

  it("keeps the link text of a stripped link rather than dropping the sentence", () => {
    expect(renderReleaseBody("[click](javascript:alert(1))")).toContain("click")
  })
})
