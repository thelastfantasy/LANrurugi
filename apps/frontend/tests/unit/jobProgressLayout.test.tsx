// The progress bar's *track* must keep the same length for the whole download. It used to sit in a
// flex row next to a label sized to its own text, so every reading that changed shape
// (`999.9 MB` → `1.0 GB`, `(9.9%)` → `(13.1%)`, a speed that gains a digit) moved the track's right
// edge — reported live as the bar growing and shrinking. Both layouts are asserted here rather than
// eyeballed, including the phone-width one, where a fixed-width label would instead have squeezed
// the track to a sliver and started clipping the numbers.
import "@/i18n"

import { render } from "@testing-library/react"
import { afterEach, describe, expect, it, vi } from "vitest"

import type { JobRecord } from "@/api/types"
import { JobProgressBar } from "@/components/Display"

function job(over: Partial<JobRecord> = {}): JobRecord {
  return {
    id: "j1",
    name: "download_url",
    state: "active",
    progress: 0.5,
    result: null,
    result_truncated: false,
    error: null,
    downloaded_bytes: 100 * 1024 * 1024,
    total_bytes: 1024 * 1024 * 1024,
    ...over,
  } as JobRecord
}

function stubViewport(narrow: boolean) {
  vi.stubGlobal(
    "matchMedia",
    (query: string) => ({
      matches: narrow,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }),
  )
}

/** The component's root is the flex row; selected structurally rather than by text, because the
 *  label's own children include a nested speed span that a text query would happily return. */
function trackAndLabel(narrow: boolean) {
  stubViewport(narrow)
  const { container } = render(<JobProgressBar job={job()} />)
  const row = container.firstElementChild as HTMLElement
  return { row, track: row.children[0] as HTMLElement, label: row.children[1] as HTMLElement }
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("JobProgressBar layout", () => {
  it("keeps the label a fixed width beside the track on a wide row", () => {
    const { row, track, label } = trackAndLabel(false)

    expect(getComputedStyle(row).flexDirection).toBe("row")
    // The exact number matters less than the fact that it is a constant: `formatBytes` output
    // varies in length from reading to reading, and only a fixed box keeps the track still.
    expect(label.style.width).toBe("224px")
    expect(label.style.flex).toBe("0 0 auto")
    expect(label.style.whiteSpace).toBe("nowrap")
    // The track grows along the row here, so it is the flex item that takes the leftover width
    // (jsdom normalizes the shorthand to its three longhands).
    expect(track.style.flexGrow).toBe("1")
  })

  it("stacks the label under a full-width track on a narrow row", () => {
    const { row, track, label } = trackAndLabel(true)

    expect(getComputedStyle(row).flexDirection).toBe("column")
    // No fixed width in this layout: the label takes the row's own width, and the track above it
    // does too. It also wraps instead of truncating, so a reading wider than a phone's row loses
    // nothing — it just takes a second line.
    expect(label.style.width).toBe("")
    expect(label.style.whiteSpace).not.toBe("nowrap")
    // Crucially *not* `flex: 1`: in a column that would apply to the vertical axis and its
    // `flex-basis: 0` would collapse the track to zero height, which is exactly how the bar
    // vanished on a phone. The fixed height must therefore survive, with the width coming from the
    // row's own `align-items: stretch`.
    expect(track.style.flexGrow).toBe("")
    expect(track.style.height).not.toBe("")
    expect(track.style.width).toBe("100%")
  })
})
