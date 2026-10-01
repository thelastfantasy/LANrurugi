import { describe, expect, it } from "vitest"

import { buildSearchToken, getTagSearchURL } from "@/lib/tagFormat"

describe("buildSearchToken", () => {
  it("leaves a single-word namespaced value unquoted, with the exact-match suffix when requested", () => {
    expect(buildSearchToken("female", "milf", true)).toBe("female:milf$")
    expect(buildSearchToken("female", "milf", false)).toBe("female:milf")
  })

  it("quotes only the value half (namespace stays outside), matching e-hentai own syntax (issue #59)", () => {
    expect(buildSearchToken("female", "huge breasts", true)).toBe('female:"huge breasts"')
    // Quoting already implies an exact match server-side — the `$` suffix would be redundant, so
    // `exact` has no separate effect once the value contains a space.
    expect(buildSearchToken("female", "huge breasts", false)).toBe('female:"huge breasts"')
  })

  // `LANRURUGI_TEST_TITLE_MULTIWORD_JA` — a real, non-synthetic multi-word Japanese archive title,
  // kept out of source per this repo's own convention (real copyrighted work titles never
  // hardcoded — see .env.example's own docs on TEST_REAL_DOWNLOAD_URL et al. for the same
  // reasoning applied elsewhere). Unset by default; this test is skipped (not failed) when it's
  // missing, since a synthetic ASCII placeholder wouldn't actually exercise the CJK-text code path
  // this test cares about.
  const multiwordJaTitle = process.env.LANRURUGI_TEST_TITLE_MULTIWORD_JA
  it.skipIf(!multiwordJaTitle)("quotes a bare (non-namespaced) multi-word value the same way", () => {
    const title = multiwordJaTitle as string
    const singleWord = title.replace(/\s+/g, "")
    expect(buildSearchToken("", singleWord, false)).toBe(singleWord)
    expect(buildSearchToken("", title, false)).toBe(`"${title}"`)
  })
})

describe("getTagSearchURL", () => {
  it("builds a plain exact-match URL for a single-word tag", () => {
    expect(getTagSearchURL("female", "milf")).toBe("/?q=female%3Amilf%24")
  })

  it("quotes a multi-word tag value instead of leaving a bare space in the query", () => {
    expect(getTagSearchURL("female", "huge breasts")).toBe("/?q=female%3A%22huge%20breasts%22")
  })

  it("leaves source tags as external links, untouched by quoting", () => {
    expect(getTagSearchURL("source", "e-hentai.org/g/123/abc")).toBe("https://e-hentai.org/g/123/abc")
  })
})
