import { useMemo, useState } from 'react'

/** A purpose-built replacement for real legacy's third-party `tagger` jQuery plugin
 * (`vendor/tagger.js`/`tagger.css`, MIT-licensed, https://github.com/jcubic/tagger — pulled in at
 * legacy's own build time, not a committed source file in the legacy repo) — not a port of the
 * library's JS, since no comparable editable-chip component exists anywhere else in this codebase
 * to build on, but its real markup shape IS replicated (`<div class="tagger"><ul><li><a>tag</a>
 * <a class="close">×</a></li>...</ul></div>`), because legacy's own *theme* CSS (`g.css` etc.)
 * targets exactly that shape (`.tagger>ul>li:not(.tagger-new)>a`) to apply each theme's real
 * background/border/border-radius to a chip — reusing it here means chip theming (including dark/
 * light theme switching) comes free from the same CSS this app already links in, with zero new
 * theme-specific code. The base layout rules `tagger.css` itself would supply (this repo doesn't
 * vendor that file) are reproduced as inline styles instead, copied from the library's real MIT
 * source (`.tagger>ul{display:flex;padding:4px 0}`, `.tagger>ul>li{margin:.4rem 0;padding-left:
 * 10px}`, chip `padding:4px 4px 4px 8px`) and cross-checked via `getComputedStyle` against a real
 * running legacy instance for the values theme CSS doesn't override (padding, weight) — `div.gt`
 * (`TagTable.tsx`'s read-only chips) was tried first and rejected: verified via the same
 * `getComputedStyle` comparison that it's a different, unrelated rule from a real edit-page chip
 * (`g.css` gives `div.gt` its own `font-weight: bold; padding: 1px 4px`, which doesn't match).
 *
 * Controlled on the same flat `"namespace:tag, namespace:tag"` string shape `archive.tags` already
 * uses — a drop-in replacement for a plain `<textarea>` without changing the caller's state shape.
 */

// Display-order-only sort (never rewrites the underlying comma-joined `value`/`onChange` string, so
// removing a tag or committing a new one still operates on the plain, unsorted form `archive.tags`
// already uses elsewhere) — `category:` namespace first (the highest-level classification),
// `source:`/`date_added:`/`timestamp:`/`uploader:` last (metadata about the archive record itself,
// not descriptive content tags), everything else (including bare, namespace-less tags)
// alphabetically in between by namespace. Module-level (not a component-body closure) since it's a
// pure function of its argument with no reactive dependencies of its own.
const NAMESPACE_PRIORITY_FIRST = ['category']
const NAMESPACE_PRIORITY_LAST = ['source', 'date_added', 'timestamp', 'uploader']
function tagSortKey(tag: string): [number, string] {
  const namespace = tag.includes(':') ? tag.slice(0, tag.indexOf(':')) : ''
  if (NAMESPACE_PRIORITY_FIRST.includes(namespace)) return [0, tag]
  if (NAMESPACE_PRIORITY_LAST.includes(namespace)) return [2, tag]
  return [1, tag]
}

export function TagInput({
  value,
  onChange,
  suggestions = [],
  disabled,
}: {
  value: string
  onChange: (next: string) => void
  suggestions?: string[]
  disabled?: boolean
}) {
  const [draft, setDraft] = useState('')

  const tags = useMemo(
    () =>
      value
        .split(/,\s?/)
        .map((t) => t.trim())
        .filter(Boolean)
        .sort((a, b) => {
          const [tierA, keyA] = tagSortKey(a)
          const [tierB, keyB] = tagSortKey(b)
          return tierA !== tierB ? tierA - tierB : keyA.localeCompare(keyB)
        }),
    [value],
  )

  const filteredSuggestions = useMemo(() => {
    if (!draft.trim()) return []
    const lower = draft.trim().toLowerCase()
    return suggestions
      .filter((s) => s.toLowerCase().includes(lower) && !tags.includes(s))
      .slice(0, 8)
  }, [draft, suggestions, tags])

  // `allow_duplicates: false` (real legacy `tagger` init option, `edit.js:64-73`).
  function commit(raw: string) {
    const next = raw.trim()
    if (!next || tags.includes(next)) return
    onChange([...tags, next].join(', '))
  }

  function removeTag(tag: string) {
    onChange(tags.filter((t) => t !== tag).join(', '))
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (e.key === 'Enter' || e.key === ',') {
      e.preventDefault()
      commit(draft)
      setDraft('')
    } else if (e.key === 'Backspace' && draft === '' && tags.length > 0) {
      removeTag(tags[tags.length - 1])
    }
  }

  // Matches `Edit.handlePaste`'s real comma-split behavior. Accumulates locally rather than
  // calling `commit` once per piece — each `commit` call would otherwise build its new value from
  // the same stale `tags` closure (this render's snapshot, not yet updated by the previous
  // iteration's `onChange`), so only the last piece would ever survive.
  function handlePaste(e: React.ClipboardEvent<HTMLInputElement>) {
    const text = e.clipboardData.getData('text')
    if (!text.includes(',')) return
    e.preventDefault()
    const next = [...tags]
    for (const piece of text.split(/,\s?/)) {
      const trimmed = piece.trim()
      if (trimmed && !next.includes(trimmed)) next.push(trimmed)
    }
    onChange(next.join(', '))
    setDraft('')
  }

  return (
    <div style={{ position: 'relative' }}>
      {/* `display: 'block'` override — legacy theme CSS's own `.tagger` rule (`g.css` etc.)
          declares `display: table-cell` plus `max-width: 450px`/`width: 60%`, but a lone
          `table-cell` div with no real `<table>`/`<tr>` ancestor doesn't actually respect either
          under the anonymous-table layout algorithm browsers fall back to — confirmed live via
          `getBoundingClientRect()`: it rendered 572px wide against a 574px grid column regardless
          of the declared 450px cap, visibly not lining up with the other fields sharing that same
          column (issue #45; those fields are also widened past legacy's original 450px cap — see
          `Edit.tsx`'s own `maxWidth: 'none'` override on each `.stdinput`). No explicit `width`
          needed here — an ordinary block element with none set is `width: auto`, which already
          fills its parent's content box on its own; forcing `display: block` (clearing the
          `table-cell` value) plus `maxWidth: 'none'` (clearing the 450px cap) is enough for that
          default to take over, without needing to touch the shared legacy stylesheet
          (`background`/`border`/`color`/`font-size` etc. still come from the same `.tagger`
          rule). Clearing `width` (the theme rule's own `width: 60%`) too, for the same reason as
          `maxWidth` — otherwise 60% of the 574px column is 344px, still short of the other
          fields' full width. */}
      <div className="tagger" style={{ minHeight: 125, width: 'auto', maxWidth: 'none', display: 'block', boxSizing: 'border-box' }}>
        <ul
          style={{
            display: 'flex',
            flexWrap: 'wrap',
            width: '100%',
            alignItems: 'center',
            padding: '4px 0',
            boxSizing: 'border-box',
            margin: 0,
            listStyle: 'none',
          }}
        >
          {tags.map((tag) => (
            <li key={tag} style={{ margin: '0.4rem 0', paddingLeft: 10, display: 'flex' }}>
              <a
                style={{
                  padding: disabled ? '4px 8px' : '4px 4px 4px 8px',
                  display: 'inline-flex',
                  alignItems: 'center',
                  gap: 4,
                  wordBreak: 'break-all',
                  textDecoration: 'none',
                }}
              >
                {tag}
                {!disabled && (
                  <button
                    type="button"
                    onClick={() => removeTag(tag)}
                    aria-label="Remove tag"
                    style={{
                      background: 'none',
                      border: 'none',
                      padding: 0,
                      margin: 0,
                      cursor: 'pointer',
                      color: 'inherit',
                      lineHeight: 1,
                      fontSize: '1em',
                    }}
                  >
                    ×
                  </button>
                )}
              </a>
            </li>
          ))}
          {!disabled && (
            // `tagger-new` — legacy theme CSS's real background/border/radius for the suggestion
            // dropdown below targets exactly `.tagger .tagger-new ul` (verified against `g.css`'s
            // own rule, `background: #F2EFDF; border: 1px solid #806769; border-radius: 5px`); this
            // `<li>` lacking that class meant the selector never matched at all, so the dropdown's
            // own `background: 'inherit'` inline style resolved through fully transparent ancestors
            // instead, rendering suggestion text with no backing surface to read it against
            // (issue #45).
            <li className="tagger-new" style={{ flexGrow: 1, position: 'relative', margin: '0.4rem 0', paddingLeft: 10, minWidth: 80 }}>
              <input
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={handleKeyDown}
                onPaste={handlePaste}
                onBlur={() => {
                  if (draft.trim()) {
                    commit(draft)
                    setDraft('')
                  }
                }}
                style={{
                  border: 'none',
                  outline: 'none',
                  boxShadow: 'none',
                  width: '100%',
                  background: 'transparent',
                  color: 'inherit',
                  font: 'inherit',
                  padding: 0,
                }}
              />
              {filteredSuggestions.length > 0 && (
                // No `background`/`border`/`borderRadius` here — deliberately left to the
                // `.tagger .tagger-new ul` theme CSS rule the parent `<li>`'s `tagger-new` class
                // now matches (see that class's own docs above), which is where legacy's real
                // per-theme background/border/radius for this exact dropdown actually lives; an
                // inline value here would just override it with a fixed color that doesn't follow
                // theme/dark-mode switching.
                <ul
                  style={{
                    position: 'absolute',
                    zIndex: 100,
                    top: '100%',
                    left: 0,
                    right: 0,
                    margin: 0,
                    padding: 5,
                    listStyle: 'none',
                  }}
                >
                  {filteredSuggestions.map((s) => (
                    <li key={s}>
                      <button
                        type="button"
                        onMouseDown={(e) => {
                          e.preventDefault()
                          commit(s)
                          setDraft('')
                        }}
                        style={{
                          textAlign: 'left',
                          width: '100%',
                          background: 'none',
                          border: 'none',
                          padding: '4px 6px',
                          cursor: 'pointer',
                          color: 'inherit',
                          font: 'inherit',
                        }}
                      >
                        {s}
                      </button>
                    </li>
                  ))}
                </ul>
              )}
            </li>
          )}
        </ul>
      </div>
    </div>
  )
}
