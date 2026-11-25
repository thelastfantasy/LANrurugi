import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import { Tooltip } from "@/components/common-ui/Display/Tooltip"

/** A queue row's finished item is a link wrapped in `TooltipIfPresent`. On a phone the tap used to
 * synthesize `mouseover`, open the metadata bubble over the row, and the link never opened. */
function Subject({ onFollow }: { onFollow: () => void }) {
  return (
    <Tooltip label={<span>preview tags</span>} wrapperStyle={{ flex: "1 1 auto" }}>
      <a
        href="#/reader/x"
        onClick={(event) => {
          event.preventDefault()
          onFollow()
        }}
      >
        finished item
      </a>
    </Tooltip>
  )
}

function touchPointer(element: Element, type: "pointerDown" | "pointerUp") {
  fireEvent[type](element, { pointerType: "touch", clientX: 10, clientY: 10 })
}

afterEach(() => {
  vi.useRealTimers()
})

describe("Tooltip on a touch screen", () => {
  it("does not open on a tap and lets the tap follow the link", () => {
    const onFollow = vi.fn()
    render(<Subject onFollow={onFollow} />)
    const link = screen.getByText("finished item")

    touchPointer(link, "pointerDown")
    touchPointer(link, "pointerUp")
    // What the browser fires next, which used to be enough to open the bubble.
    fireEvent.mouseOver(link)
    expect(screen.queryByText("preview tags")).not.toBeInTheDocument()

    fireEvent.click(link)
    expect(onFollow).toHaveBeenCalledTimes(1)
  })

  it("opens after a long press and swallows the click that ends it", () => {
    vi.useFakeTimers()
    const onFollow = vi.fn()
    render(<Subject onFollow={onFollow} />)
    const link = screen.getByText("finished item")

    touchPointer(link, "pointerDown")
    act(() => {
      vi.advanceTimersByTime(600)
    })
    expect(screen.getByText("preview tags")).toBeInTheDocument()

    touchPointer(link, "pointerUp")
    fireEvent.click(link)
    expect(onFollow).not.toHaveBeenCalled()
  })

  it("still opens on hover for a mouse", () => {
    render(<Subject onFollow={vi.fn()} />)
    const link = screen.getByText("finished item")

    fireEvent.pointerDown(link, { pointerType: "mouse" })
    fireEvent.mouseOver(link)
    expect(screen.getByText("preview tags")).toBeInTheDocument()
  })
})
