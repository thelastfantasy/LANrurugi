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
export default function TagInput({
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
        .filter(Boolean),
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
      <div className="tagger" style={{ minHeight: 125 }}>
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
            <li style={{ flexGrow: 1, position: 'relative', margin: '0.4rem 0', paddingLeft: 10, minWidth: 80 }}>
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
                    background: 'inherit',
                    border: 'inherit',
                    borderRadius: 3,
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
