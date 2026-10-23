import { describe, expect, it } from "vitest"

import {
  appendToken,
  buildDateRangeToken,
  type FilterSuggestion,
  filterTerms,
  hasFilterToken,
  isRowPick,
  parseSearchTerms,
  phraseSuggestions,
  QUICK_FILTER_TOKENS,
  QUICK_FILTERS,
  quickFilterActive,
  tagSuggestions,
  toggleQuickFilter,
  withoutFilterToken,
  withoutQuickFilter,
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

describe("filterTerms / hasFilterToken", () => {
  it("splits on whitespace but keeps a quoted run whole", () => {
    expect(filterTerms("artist:jane is:new")).toEqual(["artist:jane", "is:new"])
    expect(filterTerms('  artist:"d kaketsu"  is:new ')).toEqual(['artist:"d kaketsu"', "is:new"])
    expect(filterTerms("")).toEqual([])
  })

  it("does not see a token inside a quoted value", () => {
    // The whole point of token-aware matching: a tag literally named `is:new` is not the chip.
    expect(hasFilterToken("artist:jane", "is:new")).toBe(false)
    expect(hasFilterToken("artist:jane is:new", "is:new")).toBe(true)
    expect(hasFilterToken('artist:"is:new"', "is:new")).toBe(false)
    expect(hasFilterToken("tag:is:new", "is:new")).toBe(false)
  })
})

describe("withoutFilterToken / toggleFilterToken", () => {
  it("removes the token as a whole term and leaves the rest tidy", () => {
    expect(withoutFilterToken("artist:jane is:new has:patch", "is:new")).toBe("artist:jane has:patch")
    expect(withoutFilterToken("is:new artist:jane", "is:new")).toBe("artist:jane")
    expect(withoutFilterToken("is:new", "is:new")).toBe("")
    // Repeats all go, so a chip cannot be un-toggleable.
    expect(withoutFilterToken("is:new is:new", "is:new")).toBe("")
    // A token that is only a substring of a term stays put.
    expect(withoutFilterToken("artist:is:new", "is:new")).toBe("artist:is:new")
  })

  it("toggles on and off symmetrically", () => {
    const chip = (token: string) => ({ token })
    expect(toggleQuickFilter("", chip("is:new")).filter).toBe("is:new")
    expect(toggleQuickFilter("is:new", chip("is:new")).filter).toBe("")
    expect(toggleQuickFilter("artist:jane", chip("rating:>=4")).filter).toBe("artist:jane rating:>=4")
    expect(toggleQuickFilter("artist:jane rating:>=4", chip("rating:>=4")).filter).toBe("artist:jane")
    // Round-tripping any query through a toggle pair returns it unchanged.
    const original = "artist:jane is:reading"
    const once = toggleQuickFilter(original, chip("is:reading")).filter
    expect(toggleQuickFilter(once, chip("is:reading")).filter).toBe(original)
  })

  it("offers the operators the search engine actually accepts", () => {
    // A chip for a value the engine does not recognize would silently match nothing.
    for (const token of ["is:reading", "is:unread", "rating:>=4", "size:>=500M", "pages:>200"]) {
      expect(QUICK_FILTER_TOKENS).toContain(token)
    }
  })
})

describe("isRowPick", () => {
  it("counts only a real row press as a pick", () => {
    // Regression: Base UI reports its own input-text sync through the same `onValueChange`, so a
    // typed query equal to a recent search used to run that history row mid-keystroke.
    // Base UI's real values are kebab-case; its type union spells them in camelCase, and comparing
    // against that spelling (correct-looking, type-checking, never matching) is the bug this guards.
    expect(isRowPick("item-press")).toBe(true)
    expect(isRowPick("itemPress")).toBe(true)
    expect(isRowPick("input-change")).toBe(false)
    expect(isRowPick("list-navigation")).toBe(false)
    expect(isRowPick("none")).toBe(false)
    expect(isRowPick(undefined)).toBe(false)
  })
})

describe("phrase candidates never span a reserved operator", () => {
  it("never quotes a reserved operator's own value", () => {
    // `is:reading project` used to produce `is:"reading project"` — a query a reserved namespace
    // answers with the empty set, built by a row that also swallowed Enter. A single trailing word
    // leaves nothing to quote at all:
    expect(phraseSuggestions("is:reading project")).toEqual([])
    // Bare words *after* such a term are still a legitimate phrase source — they are separate
    // terms the engine globs, nothing to do with `has:`. What must never appear is a row that
    // folds the operator's own value into quotes.
    expect(phraseSuggestions("has:patch extra word").map((r) => r.insertValue)).toEqual(['"extra word"'])
    const reserved = /^(is|has|in|size|rating|pages|read|progress|date_added):"/
    for (const query of [
      "is:reading project",
      "is:reading two words",
      "has:patch extra word",
      "in:tank more words",
      "size:>=500M big one",
      "rating:>=4 good one",
      "pages:>200 long one",
      "progress:<50 half done",
      "date_added:>=2026-01-01 recent one",
    ]) {
      for (const row of phraseSuggestions(query)) {
        expect(reserved.test(row.insertValue), `${query} → ${row.insertValue}`).toBe(false)
      }
    }
  })

  it("still phrases the namespaces whose values really do contain spaces", () => {
    // `bookmark:` takes a bookmark *name*, `category:` a category name, `artist:` a tag value —
    // quoting those is the whole point of the feature.
    expect(phraseSuggestions("bookmark:chapter one").map((r) => r.insertValue)).toEqual([
      'bookmark:"chapter one"',
    ])
    expect(phraseSuggestions("artist:big breasts").map((r) => r.insertValue)).toEqual([
      'artist:"big breasts"',
    ])
    expect(phraseSuggestions("category:test cate").map((r) => r.insertValue)).toEqual([
      'category:"test cate"',
    ])
  })
})

describe("value-taking quick filters (bookmark:)", () => {
  const bookmark = QUICK_FILTERS.find((chip) => chip.namespace === "bookmark")!

  it("is on for any value of its operator, however it was written", () => {
    expect(quickFilterActive("", bookmark)).toBe(false)
    expect(quickFilterActive('bookmark:""', bookmark)).toBe(true)
    expect(quickFilterActive('bookmark:"村上"', bookmark)).toBe(true)
    expect(quickFilterActive("is:new bookmark:chapter one", bookmark)).toBe(true)
    // Not fooled by a different namespace that merely starts the same way.
    expect(quickFilterActive("has:bookmark", bookmark)).toBe(false)
  })

  it("inserts a placeholder and puts the caret between the quotes", () => {
    const { filter, caret } = toggleQuickFilter("is:new", bookmark)
    expect(filter).toBe('is:new bookmark:""')
    // The caret is what makes the chip usable: type straight into the quotes.
    expect(caret).toBe(filter.length - 1)
    expect(filter.slice(0, caret!)).toBe('is:new bookmark:"')
    expect(filter.slice(caret!)).toBe('"')
  })

  it("removes whichever value is there, leaving the rest of the query alone", () => {
    expect(withoutQuickFilter('is:new bookmark:"村上" has:patch', bookmark)).toBe("is:new has:patch")
    expect(toggleQuickFilter('is:new bookmark:"村上"', bookmark).filter).toBe("is:new")
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
    // Guards against a chip being added here for a namespace nothing answers. Five families
    // qualify, each with its own path in `engine.rs`: the `is:`/`has:`/`in:` state operators
    // (`attribute_filter`), the `rating:` comparison form (`parse_rating_filter`), the `size:`
    // comparison form (`size_filter`), the `pages:`/`read:` numeric comparisons
    // (`parse_numeric_filter`), and the quoted `bookmark:` name match (`bookmark_name_filter`) —
    // whose chip is the placeholder `bookmark:""` the reader then types a name into. Anything else
    // — a typo like `is:nonsense`, or a namespace the engine has never heard of — still fails here,
    // which is the point of the guard.
    const families = [
      /^(is|has|in):[a-z-]+$/,
      /^rating:(>=|<=|>|<|=)\d(\.\d)?$/,
      /^size:(>=|<=|>|<)\d+[KMG]?$/,
      /^(pages|read):(>=|<=|>|<)\d+$/,
      /^bookmark:"[^"]*"$/,
    ]
    for (const token of QUICK_FILTER_TOKENS) {
      expect(
        families.some((pattern) => pattern.test(token)),
        `${token} is not a token the engine recognizes`,
      ).toBe(true)
    }
  })
})
