import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

const reportError = vi.fn()
vi.mock("@/diagnostics/sdk", () => ({ reportError }))

const { CollapsibleSection } = await import("@/components/Display")

let broken = true
function Fragile() {
  if (broken) throw new Error("bad title shape")
  return <p>recovered</p>
}

// #283: one section's render crash must not take the other sections (or the page) with it.
describe("section error boundary", () => {
  it("keeps sibling sections rendered and reports the section id", () => {
    vi.spyOn(console, "error").mockImplementation(() => {})
    render(
      <ul>
        <CollapsibleSection id="subscriptions" icon="fa-rss" title="Subscriptions" defaultOpen>
          <Fragile />
        </CollapsibleSection>
        <CollapsibleSection id="recycle" icon="fa-recycle" title="Recycle" defaultOpen>
          <p>other section</p>
        </CollapsibleSection>
      </ul>,
    )
    expect(screen.getByRole("alert")).toHaveTextContent("bad title shape")
    expect(screen.getByText("other section")).toBeInTheDocument()
    expect(reportError).toHaveBeenCalledWith(expect.any(Error), { section: "subscriptions" })

    broken = false
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("recovered")).toBeInTheDocument()
  })
})
