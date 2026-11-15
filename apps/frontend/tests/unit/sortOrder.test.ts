import { describe, expect, it } from "vitest"

import {
  defaultSortDir,
  effectiveSortDir,
  nextSortDir,
  RELEVANCE_SORT,
} from "@/lib/sortOrder"

describe("sort order", () => {
  it("always ranks relevance most-relevant-first, whichever direction is current", () => {
    expect(effectiveSortDir(RELEVANCE_SORT, "asc")).toBe("asc")
    // The regression this pins: the Library page's default direction (desc) used to reach the
    // engine as `order_desc == true`, which ranks the *lowest* relevance score first.
    expect(effectiveSortDir(RELEVANCE_SORT, "desc")).toBe("asc")
  })

  it("leaves every other field's direction alone", () => {
    expect(effectiveSortDir("title", "desc")).toBe("desc")
    expect(effectiveSortDir("date_added", "asc")).toBe("asc")
  })

  it("switches to the picked field's natural direction, not the old field's", () => {
    // The regression this pins: relevance forces `asc`, so carrying the current direction into a
    // switch to "Date" asked the engine for oldest-first.
    expect(nextSortDir("asc", "date_added")).toBe("desc")
    expect(nextSortDir("desc", "title")).toBe("asc")
    expect(nextSortDir("desc", RELEVANCE_SORT)).toBe("asc")
    expect(nextSortDir("asc", "date_added", "asc")).toBe("asc")
    expect(nextSortDir("asc")).toBe("asc")
  })

  it("gives each field its own default when switching to it", () => {
    expect(defaultSortDir("date_added")).toBe("desc")
    expect(defaultSortDir("lastread")).toBe("desc")
    expect(defaultSortDir("title")).toBe("asc")
    expect(defaultSortDir("artist")).toBe("asc")
  })
})
