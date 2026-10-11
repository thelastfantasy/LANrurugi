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
 * Namespaces whose values are operator-style filters rather than free text — the same ones
 * `engine.rs::token_matches` special-cases ahead of its generic tag/title lookup. Quoting a span
 * that starts with one of these changes nothing, so no candidate is offered for it (a row that
 * appears to do nothing when clicked is worse than no row).
 */
const OPERATOR_NAMESPACES = new Set(["date_added", "pages", "read", "rating"])

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
 * The fragment a title lookup should search for: the last run of plain, namespace-less terms
 * (so `tari tari female:"big breasts"` looks up `tari tari`, not the already-quoted tag), or
 * `null` when the input has no such run.
 */
export function titleFilterFragment(input: string): string | null {
  const terms = parseSearchTerms(input)
  let end = -1
  for (let i = terms.length - 1; i >= 0; i--) {
    if (isPhraseWord(terms[i])) {
      end = i
      break
    }
  }
  if (end < 0) return null
  let start = end
  while (start > 0 && isPhraseWord(terms[start - 1])) start--
  const text = terms
    .slice(start, end + 1)
    .map((t) => t.text)
    .join(" ")
    .replace(/\$$/, "")
  return text.length >= 2 ? text : null
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
export const QUICK_FILTER_TOKENS = [
  "is:new",
  "is:completed",
  "is:incomplete",
  "is:read",
  "has:patch",
  "has:bookmark",
  "has:category",
  "has:split-suggestion",
  "in:tank",
] as const

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
