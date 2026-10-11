import { describe, expect, it } from "vitest"

import {
  appendToken,
  buildDateRangeToken,
  type FilterSuggestion,
  parseSearchTerms,
  phraseSuggestions,
  QUICK_FILTER_TOKENS,
  tagSuggestions,
  titleFilterFragment,
} from "@/lib/searchSuggestions"

/** Applies a suggestion the way `LibraryPage` does — the invariant every range has to satisfy. */
function apply(input: string, suggestion: FilterSuggestion): string {
  const { start, end } = suggestion.replaceRange
  return `${input.slice(0, start)}${suggestion.insertValue}${input.slice(end)}`
}

const stats = [
  { namespace: "female", text: "big breasts", weight: 120 },
  { namespace: "female", text: "huge breasts", weight: 90 },
  { namespace: "female", text: "milf", weight: 60 },
  { namespace: "artist", text: "tari", weight: 3 },
  { namespace: null, text: "tari", weight: 1 },
]

describe("parseSearchTerms", () => {
  it("splits on spaces and commas, tracking offsets and the `-` prefix", () => {
    expect(parseSearchTerms("tari tari").map((t) => t.text)).toEqual(["tari", "tari"])
    expect(parseSearchTerms("a,b").map((t) => t.text)).toEqual(["a", "b"])
    expect(parseSearchTerms("-tari x")).toEqual([
      { start: 0, end: 5, text: "-tari", plain: true, negated: true },
      { start: 6, end: 7, text: "x", plain: true, negated: false },
    ])
  })

  it("marks terms with a quoted segment, or a stray quote, as not plain", () => {
    expect(parseSearchTerms('"tari tari"')[0].plain).toBe(false)
    expect(parseSearchTerms('female:"big breasts"')[0].plain).toBe(false)
    // A quote outside a quoted segment is literal to the engine — and it makes the term match
    // nothing at all, so it is never a phrase source either.
    expect(parseSearchTerms('breasts"')[0].plain).toBe(false)
  })

  it("consumes the redundant `$` after a closing quote into the quoted term", () => {
    expect(parseSearchTerms('"tari tari"$')[0].text).toBe('"tari tari"$')
  })
})

describe("phraseSuggestions", () => {
  it("offers a quoted phrase for a bare multi-word run", () => {
    const [only, ...rest] = phraseSuggestions("tari tari")
    expect(rest).toEqual([])
    expect(only.kind).toBe("phrase")
    expect(only.insertValue).toBe('"tari tari"')
    expect(apply("tari tari", only)).toBe('"tari tari"')
  })

  it("quotes only the bare run when the query also carries an already-quoted tag", () => {
    const input = 'tari tari female:"big breasts"'
    const [only, ...rest] = phraseSuggestions(input)
    expect(rest).toEqual([])
    expect(apply(input, only)).toBe('"tari tari" female:"big breasts"')
  })

  it("lists one candidate per span instead of merging them", () => {
    const input = "pink album female:big breasts"
    const suggestions = phraseSuggestions(input)
    expect(suggestions.map((s) => apply(input, s))).toEqual([
      '"pink album" female:big breasts',
      'pink album female:"big breasts"',
    ])
  })

  it("uses the value-only `ns:\"...\"` spelling for a namespaced multi-word value", () => {
    const input = "female:big breasts"
    const [only] = phraseSuggestions(input)
    expect(only.insertValue).toBe('female:"big breasts"')
    expect(apply(input, only)).toBe('female:"big breasts"')
  })

  it("offers nothing when there is no multi-word span to quote", () => {
    // Each half already carries its own namespace, so quoting either one changes nothing.
    expect(phraseSuggestions("artist:jane female:milf")).toEqual([])
    expect(phraseSuggestions("tari")).toEqual([])
    // A negation cannot be swallowed into a phrase — the sign has to stay outside the quotes.
    expect(phraseSuggestions("-tari tari")).toEqual([])
  })

  it("skips operator-style namespaces but still finds a later bare run", () => {
    const input = "date_added:2026-08-20 foo bar"
    const [only, ...rest] = phraseSuggestions(input)
    expect(rest).toEqual([])
    expect(apply(input, only)).toBe('date_added:2026-08-20 "foo bar"')
  })

  it("strips the trailing `$`, which would otherwise become part of the quoted value", () => {
    const input = "tari tari$"
    const [only] = phraseSuggestions(input)
    expect(apply(input, only)).toBe('"tari tari"')
  })
})

describe("titleFilterFragment", () => {
  it("takes the whole bare run, not just the last word", () => {
    expect(titleFilterFragment("pink album")).toBe("pink album")
  })

  it("takes the bare run even when a quoted tag follows it", () => {
    expect(titleFilterFragment('tari tari female:"big breasts"')).toBe("tari tari")
  })

  it("has nothing to look up for a namespaced or quoted trailing term", () => {
    expect(titleFilterFragment("female:big")).toBeNull()
    expect(titleFilterFragment('"tari')).toBeNull()
    expect(titleFilterFragment("x")).toBeNull()
  })
})

describe("tagSuggestions", () => {
  it("completes within the namespace the term already names, replacing the whole term", () => {
    const input = "female:big"
    const suggestions = tagSuggestions(input, stats, 8)
    expect(suggestions.map((s) => s.label)).toEqual(["female:big breasts"])
    expect(suggestions[0].count).toBe(120)
    // The inserted token quotes the multi-word value — the same `buildSearchToken` spelling every
    // other insertion point in the app uses.
    expect(apply(input, suggestions[0])).toBe('female:"big breasts"')
  })

  it("completes inside an already-opened quote, where the old fragment match saw only the tail", () => {
    const input = 'female:"big b'
    const [only] = tagSuggestions(input, stats, 8)
    expect(only.label).toBe("female:big breasts")
    expect(apply(input, only)).toBe('female:"big breasts"')
  })

  it("keeps a negation outside the inserted token", () => {
    const input = "-female:big"
    const [only] = tagSuggestions(input, stats, 8)
    expect(apply(input, only)).toBe('-female:"big breasts"')
  })

  it("falls back to a substring match over `namespace:text` for a bare fragment", () => {
    const suggestions = tagSuggestions("tari", stats, 8)
    expect(suggestions.map((s) => s.label)).toEqual(["artist:tari", "tari"])
    expect(apply("tari", suggestions[0])).toBe("artist:tari")
  })

  it("offers nothing after a delimiter or an empty input", () => {
    expect(tagSuggestions("", stats, 8)).toEqual([])
    expect(tagSuggestions("artist:jane ", stats, 8)).toEqual([])
  })
})

describe("buildDateRangeToken", () => {
  it("emits two half-open bounds for a range", () => {
    expect(buildDateRangeToken("2026-01-01", "2026-03-31")).toBe(
      "date_added:>=2026-01-01 date_added:<=2026-03-31",
    )
  })

  it("swaps an inverted range instead of returning something that matches nothing", () => {
    expect(buildDateRangeToken("2026-03-31", "2026-01-01")).toBe(
      "date_added:>=2026-01-01 date_added:<=2026-03-31",
    )
  })

  it("accepts a single bound and skips an unusable one", () => {
    expect(buildDateRangeToken("2026-01-01", "")).toBe("date_added:>=2026-01-01")
    expect(buildDateRangeToken("", "2026-01-01")).toBe("date_added:<=2026-01-01")
    expect(buildDateRangeToken("2026-01-01", "not-a-date")).toBe("date_added:>=2026-01-01")
    expect(buildDateRangeToken("", "")).toBeNull()
  })
})

describe("appendToken", () => {
  it("separates with exactly one space and never fuses onto a half-typed word", () => {
    expect(appendToken("artist:jane", "is:new")).toBe("artist:jane is:new")
    expect(appendToken("artist:jane ", "is:new")).toBe("artist:jane is:new")
    expect(appendToken("", "is:new")).toBe("is:new")
  })
})

describe("QUICK_FILTER_TOKENS", () => {
  it("only offers operators the engine actually implements", () => {
    // Guards against a chip being added here for a namespace nothing answers: every token is an
    // `is:`/`has:`/`in:` operator, which `engine.rs::attribute_filter` owns.
    for (const token of QUICK_FILTER_TOKENS) {
      expect(token).toMatch(/^(is|has|in):[a-z-]+$/)
    }
  })
})
