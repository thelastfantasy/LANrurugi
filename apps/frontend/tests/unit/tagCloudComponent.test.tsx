import { render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it, vi } from "vitest"

import { TagCloud } from "@/components/Display/TagCloud"

// jsdom has no `ResizeObserver` at all (confirmed: `'ResizeObserver' in new JSDOM(...).window` is
// `false`) — `TagCloud.tsx`'s own `useLayoutEffect` constructs one unconditionally, so rendering
// the component in any jsdom-based test throws `ReferenceError: ResizeObserver is not defined`
// without this. A minimal stub (never actually needs to *fire* here — this suite only covers the
// empty-tags render path, not resize-driven rebuilds) is enough.
//
// jsdom also has no `window.matchMedia` at all — `build()` calls it unconditionally (for
// `prefers-reduced-motion`) even on the *non-empty* tags path this suite's second test exercises,
// so a stub is needed here too even though this suite makes no assertions about reduced-motion
// itself.
beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  vi.stubGlobal(
    "matchMedia",
    (query: string) => ({
      matches: false,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    }),
  )
})

// i18next itself isn't initialized in this unit-test environment (no `i18n.ts` import chain run),
// so `useTranslation()`'s `t()` returns the raw translation *key* rather than the fallback string
// `TagCloud.tsx`'s own `emptyMessage = t(...) ?? "No tags to show yet."` only reaches when `t()`
// itself returns `undefined`/falsy (which it never does once i18next is initialized either — this
// suite deliberately asserts against the un-initialized key text, not the English fallback string,
// since that's what actually renders in this environment).
describe("TagCloud (component)", () => {
  it("renders the graceful empty-state message for a zero-tag library, not a broken/blank sphere", () => {
    render(<TagCloud tags={[]} />)
    expect(screen.getByText("stats.noTagsToShow")).toBeInTheDocument()
    // The empty state is a plain message div, never a `.tag-cloud-3d` sphere element — asserting
    // its absence catches a regression where `build()`'s own `rendered.length === 0` early-return
    // stopped actually short-circuiting before constructing the library instance.
    expect(document.querySelector(".tag-cloud-3d")).not.toBeInTheDocument()
  })

  it("does not render the empty-state message once real tags are provided", () => {
    render(<TagCloud tags={[{ namespace: null, text: "example", weight: 5 }]} />)
    expect(screen.queryByText("stats.noTagsToShow")).not.toBeInTheDocument()
  })
})
