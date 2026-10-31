// Localisation for the API reference's *body text* — pure functions over an overlay object.
//
// The OpenAPI document itself stays English on purpose: it is the machine-readable contract other
// tools consume, and a contract that changes language per reader is not a contract. Everything here
// therefore happens at render time, in the docs site only.
//
// Three kinds of source text, in the order they are tried, because they need different treatment:
//
//   1. **Generated boilerplate.** 133 operation descriptions and every derived response description
//      are one template repeated with a different `file:line` in it. Matching them with a regex is
//      what keeps the overlay from needing ~250 near-identical entries.
//   2. **Repeated prose.** Parameter and response descriptions recur verbatim across operations
//      ("ID of the Archive to process." appears 14 times), so those maps are keyed by the English
//      source text and one entry covers every occurrence.
//   3. **Per-operation text.** Summaries and real descriptions are unique, so they are keyed by
//      `operationId`.
//
// Anything with no entry falls through to the English source rather than to a placeholder — a
// half-translated page is honest about what has not been translated yet, and the generator reports
// the count.

/** Normalise Starlight's locale value (`zh-CN`, or the literal `root`) to an overlay key. */
export function localeKey(locale) {
  const raw = String(locale ?? '').toLowerCase()
  // This site's default locale is the Chinese one, so `root` means `zh`.
  return raw === 'root' || raw === '' ? 'zh' : raw.split('-')[0]
}

function applyPatterns(text, patterns) {
  for (const pattern of patterns ?? []) {
    const re = new RegExp(pattern.match)
    if (re.test(text)) return text.replace(re, pattern.replace)
  }
  return text
}

/**
 * The generator's own "nothing hand-written for this endpoint yet" preamble, rebuilt in the target
 * language.
 *
 * Structurally rather than by regex substitution, because the part that needs replacing is the
 * *quoted handler comment* — a different string per operation. A template that only rewrites the
 * preamble leaves an English block in the middle of a Chinese page (which is exactly what shipped
 * first), and a regex that tried to capture the whole quote would have to match arbitrary prose.
 *
 * The quote itself is replaced from `overlay.quotedComments[operationId]` when a translation exists,
 * and the preamble's wording changes with it: an untranslated quote is labelled as quoted verbatim,
 * a translated one as a translation. Saying "quoted verbatim" over a translation would be a lie, and
 * the `file:line` is kept either way so the reader can check the original.
 */
function localizeDerivedDescription(overlay, operation) {
  const text = operation.description ?? ''
  const derived = overlay.derived
  if (!derived || !text.startsWith('Not yet described in this reference.')) return null

  const [head, ...tail] = text.split('\n\n---\n\n')
  const quote = tail.join('\n\n---\n\n')
  const named = head.match(/The handler is `(?<handler>[^`]+)` in (?<location>.+?)\.$/)
  const commented = head.match(/comment \((?<location>[^)]+)\), quoted verbatim/)

  const lines = [derived.headline]
  if (named) {
    lines.push(derived.noComment.replace('{handler}', named.groups.handler).replace('{location}', named.groups.location))
  }
  if (quote) {
    const translated = overlay.quotedComments?.[operation.operationId]
    lines.push(
      (translated ? derived.quoteTranslated : derived.quoteVerbatim).replace(
        '{location}',
        commented?.groups.location ?? '',
      ),
    )
    lines.push(translated ?? quote)
  }
  return lines.join('\n\n')
}

/** An operation's summary and description in the overlay's language, or the source text. */
export function localizeOperation(overlay, operation) {
  // Every path out of here strips the appended access notes: the overlay's own Chinese text never
  // carried them, and the English source does — so without this only the English pages would print
  // the paragraph twice, once as description and once as the callout right below it.
  if (!overlay) {
    return {
      summary: operation.summary,
      description: stripAccessNotes(operation.description ?? ''),
    }
  }
  const entry = overlay.operations?.[operation.operationId]
  return {
    summary: entry?.summary ?? operation.summary,
    description: stripAccessNotes(
      entry?.description ??
        localizeDerivedDescription(overlay, operation) ??
        (operation.description ?? ''),
    ),
  }
}

/**
 * Whether an explicit overlay decision exists for a string.
 *
 * Asked separately from "did the output change" because those are not the same question: a proper
 * noun like `Tankoubon` is *deliberately* its own translation, and comparing strings would report it
 * as untranslated forever.
 */
export function hasParameterEntry(overlay, text) {
  return Boolean(overlay && text && overlay.parameters?.[text.trim()] !== undefined)
}

export function hasResponseEntry(overlay, text) {
  if (!overlay || !text) return false
  const trimmed = text.trim()
  if (overlay.responses?.[trimmed] !== undefined) return true
  return (overlay.patterns?.responseDescription ?? []).some((p) => new RegExp(p.match).test(trimmed))
}

/**
 * Whether an operation's *description* has a decision behind it: its own entry, or a boilerplate
 * template that matched. Separate from the summary on purpose — every operation now has a translated
 * summary, so asking the combined question would report descriptions as 100% covered while they were
 * still English.
 */
export function hasOperationDescription(overlay, operation) {
  if (!overlay) return false
  if (overlay.operations?.[operation.operationId]?.description) return true
  return (
    Boolean(overlay.derived) &&
    (operation.description ?? '').startsWith('Not yet described in this reference.')
  )
}

/**
 * Drop the prose copy of the access notes from a description.
 *
 * The docgen appends the notes to `description` so Swagger UI shows them under the endpoint title
 * instead of at the bottom of the operation (it offers no other placement), and also emits them as
 * `x-lanrurugi-access-notes`. This site renders them as its own localised callout, so without this
 * an English page would print the same paragraph twice, immediately above itself.
 */
export function stripAccessNotes(description) {
  if (typeof description !== 'string') return description
  const marker = '<!-- lanrurugi:access-notes -->'
  const at = description.indexOf(marker)
  return at === -1 ? description : description.slice(0, at).trimEnd()
}

/**
 * An operation's per-role behaviour notes, translated.
 *
 * Keyed by `operationId` rather than by the note text: the same English note is shared by nine
 * archive-scoped reads (the document file uses a YAML anchor for exactly that reason), so keying by
 * text would mean nine identical overlay entries.
 */
export function localizeAccessNotes(overlay, operation) {
  const source = operation['x-lanrurugi-access-notes']
  if (!Array.isArray(source)) return []
  const translated = overlay?.accessNotes?.[operation.operationId]
  return Array.isArray(translated) && translated.length === source.length ? translated : source
}

/** A parameter's description, looked up by its English source text. */
export function localizeParameter(overlay, text) {
  if (!overlay || !text) return text
  return overlay.parameters?.[text.trim()] ?? text
}

/** A response's description: explicit entry first, then the generated-boilerplate template. */
export function localizeResponse(overlay, text) {
  if (!overlay || !text) return text
  const trimmed = text.trim()
  return (
    overlay.responses?.[trimmed] ??
    applyPatterns(trimmed, overlay.patterns?.responseDescription)
  )
}
