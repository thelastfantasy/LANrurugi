import { buildSearchToken } from "@/lib/tagFormat"

/**
 * Suggestion construction for the Library search box's autocomplete.
 *
 * The grammar mirrored here is the backend's own (`crates/lanrurugi-search/src/grammar.rs`): a
 * space or comma separates terms, a leading `-` negates a term, and a `"` — either at the start of
 * a term or right after a `:` — opens a quoted segment that runs to the next unescaped `"`.
 *
 * The one rewrite this module offers is *quoting a span*, which the engine reads as "this exact
 * text, contiguous": on a tag that means the literal tag value, on a title it means the phrase
 * appears somewhere in the title. That only makes sense for a *plain* term — one with no quoted
 * segment and no stray `"` — because the enclosed text has to be exactly the text the engine
 * compares against. Nothing here ever rewrites the input on its own; every candidate is offered as
 * a row the user has to pick.
 */

export type SearchSuggestionKind = "tag" | "title" | "phrase"

/** A row that rewrites part of the filter — tag completion, or a quoted phrase. */
export interface FilterSuggestion {
  kind: "tag" | "phrase"
  /** Text shown in the dropdown. */
  label: string
  /** What replaces `replaceRange` when this row is picked. */
  insertValue: string
  /** Archive count, when it is known without an extra request. */
  count?: number
  /**
   * Range of the current input this row replaces. Always set: a suggestion that appends instead
   * would leave the fragment it was built from in place (`female:big` + `female:big breasts`).
   */
  replaceRange: { start: number; end: number }
}

/** A row that opens an archive the current input already matches, rather than editing the filter. */
export interface TitleSuggestion {
  kind: "title"
  label: string
  arcid: string
}

export type SearchSuggestion = FilterSuggestion | TitleSuggestion

/** One space/comma-delimited term of a filter, with its offsets in the raw input. */
export interface SearchTerm {
  start: number
  end: number
  /** Raw text, e.g. `female:"big breasts"`, `-tari`, or `tari$`. */
  text: string
  /** No quoted segment and no stray `"` — the term is plain text the engine will glob on. */
  plain: boolean
  negated: boolean
}

export interface TagStatLike {
  namespace: string | null
  text: string
  weight: number
}

/** `ns:value` (value optionally already opened with a quote) at the start of a term. */
const NAMESPACE_PREFIX = /^-?([^:\s"*?]+):"?(.*)$/

/**
 * Namespaces whose value is an *enumerated symbol or comparison* rather than free text, and which
 * `engine.rs` therefore answers ahead of its generic tag lookup: the state operators owned by
 * `attribute_filter` (`is:`, `has:`, `in:`, `size:`), the numeric/date forms
 * (`pages:`/`read:`/`rating:`/`date_added:`), and `progress:` (an alias of `read:`).
 *
 * No phrase candidate is offered for these. Joining `is:reading project` into
 * `is:"reading project"` looks helpful and is not: a reserved namespace answers an unrecognized
 * value with the *empty set* (that is what keeps `is:nonsense` from becoming a tag search), so the
 * row would build a query that can never match — and because picking a row consumes Enter, the
 * symptom a reader sees is "Enter does nothing, and my quotes moved".
 *
 * Deliberately *not* here: `bookmark:`, `category:` and `phrase:`, whose values are names or
 * phrases that legitimately contain spaces, and every other namespace (tags, `title:`,
 * `filename:`), which is what this module's phrase feature exists for.
 */
const OPERATOR_NAMESPACES = new Set([
  "date_added",
  "pages",
  "read",
  "progress",
  "rating",
  "size",
  "is",
  "has",
  "in",
])

/** Splits a filter into its terms, tracking each one's offsets. */
export function parseSearchTerms(input: string): SearchTerm[] {
  const terms: SearchTerm[] = []
  let i = 0
  while (i < input.length) {
    while (i < input.length && (input[i] === "," || input[i] === " ")) i++
    if (i >= input.length) break
    const start = i
    let negated = false
    let plain = true
    let quoted = false
    if (input[i] === "-") {
      negated = true
      i++
    }
    if (input[i] === '"') {
      plain = false
      quoted = true
      i++
    }
    while (i < input.length) {
      const c = input[i]
      if (quoted) {
        if (c === "\\") {
          i += 2
          continue
        }
        if (c === '"') {
          quoted = false
          plain = false
          i++
          // A `$` right after the closing quote is the redundant exact-match suffix, part of no
          // value — consume it so it can't look like trailing literal text.
          if (input[i] === "$") i++
          continue
        }
        i++
        continue
      }
      if (c === "," || c === " ") break
      if (c === ":") {
        i++
        if (input[i] === '"') {
          quoted = true
          plain = false
          i++
        }
        continue
      }
      // A `"` outside a quoted segment is literal to the engine, but it makes the term unusable as
      // a phrase source (`breasts"` matches nothing at all), so the term stops being plain.
      if (c === '"') plain = false
      i++
    }
    terms.push({ start, end: i, text: input.slice(start, i), plain, negated })
  }
  return terms
}

/** Its own namespace, or `null` for a bare (non-namespaced) term. */
function namespaceOf(term: SearchTerm): string | null {
  return NAMESPACE_PREFIX.exec(term.text)?.[1] ?? null
}

/** A term that is plain text within a single namespace-less word run — the only kind that can be
 * joined into a quoted phrase without swallowing a `ns:` prefix or a negation. */
function isPhraseWord(term: SearchTerm): boolean {
  return term.plain && !term.negated && namespaceOf(term) === null
}

/**
 * Quoted-phrase candidates, one per quotable span, each replacing exactly that span:
 *
 * - a run of two or more bare words becomes `"the whole run"`;
 * - a `ns:first` term followed by bare words becomes `ns:"first and the rest"` — the same token as
 *   the whole-run spelling (grammar.rs pins the two forms as identical), and the spelling that
 *   makes a multi-word tag value searchable at all.
 *
 * Every span gets its own row rather than one merged rewrite: `a b female:"c d" e f` is genuinely
 * ambiguous about which part is meant to be the phrase, and only the user knows.
 */
export function phraseSuggestions(input: string): FilterSuggestion[] {
  const terms = parseSearchTerms(input)
  const out: FilterSuggestion[] = []
  let i = 0
  while (i < terms.length) {
    const head = terms[i]
    if (!head.plain || head.negated) {
      i++
      continue
    }
    const namespace = namespaceOf(head)
    if (namespace !== null && OPERATOR_NAMESPACES.has(namespace.toLowerCase())) {
      i++
      continue
    }
    if (namespace !== null) {
      const headValue = NAMESPACE_PREFIX.exec(head.text)?.[2] ?? ""
      let j = i + 1
      while (j < terms.length && isPhraseWord(terms[j])) j++
      if (j > i + 1) {
        const value = [headValue, ...terms.slice(i + 1, j).map((t) => t.text)]
          .join(" ")
          .replace(/\$$/, "")
          .trim()
        if (value !== "") {
          out.push({
            kind: "phrase",
            label: buildSearchToken(namespace, value),
            insertValue: buildSearchToken(namespace, value),
            replaceRange: { start: head.start, end: terms[j - 1].end },
          })
        }
        i = j
        continue
      }
      i++
      continue
    }
    let j = i + 1
    while (j < terms.length && isPhraseWord(terms[j])) j++
    if (j - i >= 2) {
      const phrase = terms
        .slice(i, j)
        .map((t) => t.text)
        .join(" ")
        .replace(/\$$/, "")
      out.push({
        kind: "phrase",
        label: buildSearchToken("", phrase),
        insertValue: buildSearchToken("", phrase),
        replaceRange: { start: head.start, end: terms[j - 1].end },
      })
      i = j
      continue
    }
    i++
  }
  return out
}

/**
 * Tag-value completions for the term the input currently ends on. Namespace-aware when that term
 * is `ns:...` (including the already-opened `ns:"...` form, where matching on the whole fragment
 * used to reduce to matching on the text after the last space), and a plain substring match over
 * `namespace:text` otherwise — the behavior this replaced.
 */
export function tagSuggestions(
  input: string,
  stats: readonly TagStatLike[],
  limit: number,
): FilterSuggestion[] {
  // Nothing to complete after a delimiter, matching the fragment-based behavior this replaced.
  if (input === "" || /[\s,]$/.test(input)) return []
  const terms = parseSearchTerms(input)
  const trailing = terms[terms.length - 1]
  if (!trailing) return []

  const namespace = NAMESPACE_PREFIX.exec(trailing.text)?.[1] ?? null
  const needle = (
    namespace !== null
      ? (NAMESPACE_PREFIX.exec(trailing.text)?.[2] ?? "")
      : trailing.text.replace(/^-/, "")
  )
    .replace(/\$$/, "")
    .replace(/"$/, "")
    .toLowerCase()

  const matched = stats
    .map((s) => ({
      namespace: s.namespace,
      text: s.text,
      weight: s.weight,
      label: s.namespace ? `${s.namespace}:${s.text}` : s.text,
    }))
    .filter((s) =>
      namespace !== null
        ? (s.namespace ?? "").toLowerCase() === namespace.toLowerCase() &&
          s.text.toLowerCase().includes(needle)
        : s.label.toLowerCase().includes(needle),
    )
    .sort((a, b) => b.weight - a.weight)
    .slice(0, limit)

  return matched.map((s) => ({
    kind: "tag",
    label: s.label,
    insertValue: `${trailing.negated ? "-" : ""}${buildSearchToken(s.namespace ?? "", s.text)}`,
    count: s.weight,
    replaceRange: { start: trailing.start, end: trailing.end },
  }))
}

/**
 * Quick-filter tokens the panel offers as one-click chips. All of them are operators the engine
 * answers from archive state (`is:`/`has:`/`in:`), so a chip is a genuine filter, not a shortcut
 * that happens to look like one.
 */
/** A quick-filter chip in the search panel.
 *
 *  Most chips stand for one literal token. The ones whose operator takes a value the *user* supplies
 *  (`bookmark:"<name>"`) instead insert a placeholder and put the caret between the quotes, ready to
 *  type — and stand for any value of that operator, so the chip is on however the name is spelled
 *  and switching it off removes whichever name was there. */
export interface QuickFilter {
  /** The canonical token, and what the chip shows unless {@link QuickFilter.label} says otherwise. */
  token: string
  /** Display text, when the literal token is not what a reader should see (`bookmark:""` reads as a
   *  mistake; `bookmark:"…"` reads as "a name goes here"). */
  label?: string
  /** Set when the chip means "any value of this operator" rather than exactly {@link token}. */
  namespace?: string
  /** Text to insert, when it differs from the token (the placeholder form). */
  insert?: string
  /** Caret offset inside the inserted text; defaults to the end of it. */
  caret?: number
}

export const QUICK_FILTERS: readonly QuickFilter[] = [
  // `is:` — reading state, in the order a reader moves through it.
  { token: "is:new" },
  { token: "is:reading" },
  { token: "is:read" },
  { token: "is:unread" },
  { token: "is:completed" },
  { token: "is:incomplete" },
  { token: "is:untagged" },
  // `has:` — associated state.
  { token: "has:patch" },
  { token: "has:bookmark" },
  { token: "has:category" },
  { token: "has:split-suggestion" },
  // Membership, size and length: the three non-state things worth a single click. `is:tank` is
  // deliberately absent — the Library's own selector already covers Tankoubon rows, and having
  // both it and `in:tank` (membership) in one row reads as two spellings of the same filter.
  { token: "in:tank" },
  { token: "size:>=500M" },
  { token: "pages:>200" },
  // Values the reader supplies: the caret lands inside the quotes on insert.
  { token: 'bookmark:""', label: 'bookmark:"…"', namespace: "bookmark", caret: 10 },
  { token: "rating:<3" },
  { token: "rating:>=4" },
]

/** The chips' tokens as plain strings — what the search bar's Enter-action map needs. */
export const QUICK_FILTER_TOKENS: readonly string[] = QUICK_FILTERS.map((chip) => chip.token)

/**
 * Builds the filter text for an "added between" range — `date_added:>=2026-01-01
 * date_added:<=2026-03-31` — from the two `YYYY-MM-DD` values a native `<input type="date">`
 * produces. Returns `null` when neither bound is usable, and skips an unusable bound rather than
 * emitting a token that would match nothing; an inverted range is swapped rather than silently
 * returning zero results, since that is what a user who typed the two dates backwards meant.
 */
export function buildDateRangeToken(from: string, to: string): string | null {
  const valid = (value: string) => /^\d{4}-\d{2}-\d{2}$/.test(value)
  const hasFrom = valid(from)
  const hasTo = valid(to)
  if (!hasFrom && !hasTo) return null
  if (hasFrom && hasTo) {
    const [start, end] = from <= to ? [from, to] : [to, from]
    return `date_added:>=${start} date_added:<=${end}`
  }
  return hasFrom ? `date_added:>=${from}` : `date_added:<=${to}`
}

/**
 * Stable React key for a suggestion row (a title row is identified by its archive, everything else
 * by the text it would insert).
 */
export function suggestionKey(suggestion: SearchSuggestion): string {
  return suggestion.kind === "title"
    ? `title:${suggestion.arcid}`
    : `${suggestion.kind}:${suggestion.insertValue}`
}

/**
 * The text Base UI's Autocomplete fills the input with when a row is picked — and therefore also
 * the key the search bar uses to tell "the user typed this" from "a row was picked". Titles use
 * their label because that is what a title row *is*; the pick itself navigates rather than editing
 * the query.
 */
export function suggestionSelectionText(suggestion: SearchSuggestion): string {
  return suggestion.label
}

/** Appends a token to a filter string with exactly one separating space, so a chip clicked after a
 *  half-typed word can't silently fuse into it. */
export function appendToken(filter: string, token: string): string {
  const trimmed = filter.trimEnd()
  return trimmed === "" ? token : `${trimmed} ${token}`
}

/** Splits a filter into its whitespace-separated terms, dropping empties. Quoted values are left
 *  alone: a quoted run is one term however many spaces it contains, which is what makes
 *  {@link hasFilterToken} safe to use for toggling a chip off (unlike a substring test, which would
 *  also match inside `artist:"is:new"`). */
export function filterTerms(filter: string): string[] {
  const terms: string[] = []
  let current = ""
  let quoted = false
  for (const char of filter) {
    if (char === '"') quoted = !quoted
    if (!quoted && /\s/.test(char)) {
      if (current !== "") terms.push(current)
      current = ""
      continue
    }
    current += char
  }
  if (current !== "") terms.push(current)
  return terms
}

/** Whether a chip's token is already part of the query — the chip's own on/off state. Exposed so
 *  the panel's chip cannot disagree with what the search will actually run, and separate from a
 *  loose `includes` so `is:new` is not "present" inside `artist:"is:new"`. */
export function hasFilterToken(filter: string, token: string): boolean {
  return filterTerms(filter).some((term) => term.toLocaleLowerCase() === token.toLocaleLowerCase())
}

/** Removes every occurrence of a token as a whole term. Used when a chip is toggled off; removing
 *  only the first match would leave a query the user cannot turn off, and the result is retrimmed
 *  so no double space is left where the token was. */
export function withoutFilterToken(filter: string, token: string): string {
  return filterTerms(filter)
    .filter((term) => term.toLocaleLowerCase() !== token.toLocaleLowerCase())
    .join(" ")
}

/**
 * Whether a value change reported by Base UI's Autocomplete is a *pick* — a row the user actually
 * chose — rather than its own sync of the input text to a matching item.
 *
 * The distinction is load-bearing: Base UI reports both through `onValueChange`, so without it,
 * typing a query that happens to equal one of your recent searches runs that history row from
 * under the cursor (applying the search and closing the panel mid-word).
 *
 * Compared with the hyphen stripped because Base UI's reason *values* are kebab-case
 * (`node_modules/@base-ui/react/internals/reason-parts.mjs`: `itemPress = 'item-press'`) while its
 * own type union is written with camelCase identifiers — reading the type and comparing against
 * `"itemPress"` compiles, type-checks, and never matches, which silently turned every pick into "the
 * reader typed this" (a picked archive row wrote its own title into the search box).
 */
export function isRowPick(reason: string | undefined): boolean {
  return reason?.replace(/-/g, "").toLowerCase() === "itempress"
}

/** Whether a chip is currently on: any value of its operator counts, so a `bookmark:"村上"` the
 *  user typed by hand lights the same chip as the placeholder was inserted with. */
export function quickFilterActive(filter: string, chip: QuickFilter): boolean {
  if (!chip.namespace) return hasFilterToken(filter, chip.token)
  const prefix = `${chip.namespace.toLocaleLowerCase()}:`
  return filterTerms(filter).some((term) => term.toLocaleLowerCase().startsWith(prefix))
}

/** Removes a chip: the exact token, or — for a value-taking operator — whichever value is there. */
export function withoutQuickFilter(filter: string, chip: QuickFilter): string {
  if (!chip.namespace) return withoutFilterToken(filter, chip.token)
  const prefix = `${chip.namespace.toLocaleLowerCase()}:`
  return filterTerms(filter)
    .filter((term) => !term.toLocaleLowerCase().startsWith(prefix))
    .join(" ")
}

/** A chip's own action: insert when it is absent, remove when it is already there.
 *
 *  `caret` is an offset into the *returned* filter, present only when the inserted text is a
 *  placeholder the reader is expected to fill in (see {@link QuickFilter.caret}) — the caller puts
 *  the input's selection there, which is what makes a `bookmark:` chip typeable straight away. */
export function toggleQuickFilter(
  filter: string,
  chip: QuickFilter,
): { filter: string; caret?: number } {
  if (quickFilterActive(filter, chip)) return { filter: withoutQuickFilter(filter, chip) }
  const inserted = chip.insert ?? chip.token
  const next = appendToken(filter, inserted)
  if (chip.caret === undefined) return { filter: next }
  const fromEnd = inserted.length - chip.caret
  return { filter: next, caret: Math.max(0, next.length - fromEnd) }
}
