import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { normalizeTag } from "@/lib/tagFormat"

/** Replicates legacy's `tagger` jQuery plugin markup (`<div class="tagger"><ul><li><a>`) so its
 * theme CSS applies unmodified. Controlled on the flat `"namespace:tag, namespace:tag"` string. */

// Display-order-only sort (never rewrites the underlying comma-joined value) — `category:` first,
// `source:`/`date_added:`/`timestamp:`/`uploader:` last, everything else alphabetical between.
const NAMESPACE_PRIORITY_FIRST = ["category"]
const NAMESPACE_PRIORITY_LAST = ["source", "date_added", "timestamp", "uploader"]
function tagSortKey(tag: string): [number, string] {
  const namespace = tag.includes(":") ? tag.slice(0, tag.indexOf(":")) : ""
  if (NAMESPACE_PRIORITY_FIRST.includes(namespace)) return [0, tag]
  if (NAMESPACE_PRIORITY_LAST.includes(namespace)) return [2, tag]
  return [1, tag]
}

function splitTags(value: string): string[] {
  return value
    .split(/,\s?/)
    .map((t) => t.trim())
    .filter(Boolean)
}

/** How long a touch must stay on a tag before it opens for editing. */
const LONG_PRESS_MS = 500

export function TagInput({
  value,
  onChange,
  suggestions = [],
  disabled,
  formatTag,
}: {
  value: string
  onChange: (next: string) => void
  suggestions?: string[]
  disabled?: boolean
  /** Rewrites a tag the user just added or edited (e.g. a `source:` URL into its stored form).
   * The tag goes in as typed and is swapped once this resolves; failures keep it as typed. */
  formatTag?: (tag: string) => Promise<string>
}) {
  const { t } = useTranslation()
  const [draft, setDraft] = useState("")
  const inputRef = useRef<HTMLInputElement>(null)
  const [editing, setEditing] = useState<{ original: string; draft: string } | null>(null)
  const pressTimerRef = useRef<number | undefined>(undefined)
  const touchPressRef = useRef(false)
  // Set once an edit is committed or cancelled, so the blur that follows its unmount is ignored.
  const closingRef = useRef(false)

  const historyRef = useRef<{ past: string[]; future: string[] }>({ past: [], future: [] })
  const skipHistoryRef = useRef(false)
  const lastValueRef = useRef(value)

  useEffect(() => {
    if (skipHistoryRef.current) {
      skipHistoryRef.current = false
    } else if (value !== lastValueRef.current) {
      historyRef.current.past.push(lastValueRef.current)
      historyRef.current.future = []
    }
    lastValueRef.current = value
  }, [value])

  function undo() {
    const { past } = historyRef.current
    if (past.length === 0) return
    const previous = past.pop() as string
    historyRef.current.future.push(value)
    skipHistoryRef.current = true
    onChange(previous)
  }

  function redo() {
    const { future } = historyRef.current
    if (future.length === 0) return
    const next = future.pop() as string
    historyRef.current.past.push(value)
    skipHistoryRef.current = true
    onChange(next)
  }

  const tags = useMemo(
    () =>
      splitTags(value).sort((a, b) => {
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

  /** Swaps `tag` for its `formatTag` form in whatever the value is by the time that answers. Kept
   * out of undo history, so one undo still removes the tag as a whole. */
  function reformat(tag: string) {
    if (!formatTag) return
    formatTag(tag)
      .then((formatted) => {
        const next = normalizeTag(formatted)
        if (!next || next === tag) return
        const current = splitTags(lastValueRef.current)
        if (!current.includes(tag)) return
        const replaced = current.includes(next) ? current.filter((t) => t !== tag) : current.map((t) => (t === tag ? next : t))
        // Recorded as already seen, so the history effect does not push it, and a second
        // reformat answering before the re-render builds on this one instead of overwriting it.
        lastValueRef.current = replaced.join(", ")
        onChange(lastValueRef.current)
      })
      .catch(() => {})
  }

  function commit(raw: string) {
    const next = normalizeTag(raw)
    if (!next || tags.includes(next)) return
    onChange([...tags, next].join(", "))
    reformat(next)
  }

  function startEdit(tag: string) {
    if (disabled) return
    closingRef.current = false
    setEditing({ original: tag, draft: tag })
  }

  /** An emptied tag is removed; one edited into an existing tag merges into it. */
  function finishEdit() {
    if (!editing || closingRef.current) return
    closingRef.current = true
    const { original } = editing
    const next = normalizeTag(editing.draft)
    setEditing(null)
    if (next === original) return
    const others = tags.filter((t) => t !== original)
    onChange((!next || others.includes(next) ? others : [...others, next]).join(", "))
    if (next) reformat(next)
  }

  function cancelLongPress() {
    window.clearTimeout(pressTimerRef.current)
  }

  function removeTag(tag: string) {
    onChange(tags.filter((t) => t !== tag).join(", "))
    // Restore focus to the draft input so the user can keep typing/undo without an extra click.
    inputRef.current?.focus()
  }

  /** Only handled while the draft input is empty, so mid-typing Ctrl+Z still uses the browser's
   * own native text-input undo instead of reverting the last committed/removed tag. */
  function handleUndoRedoKeyDown(e: React.KeyboardEvent): boolean {
    if (draft !== "" || !(e.ctrlKey || e.metaKey)) return false
    const key = e.key.toLowerCase()
    if (key === "z" && !e.shiftKey) {
      e.preventDefault()
      e.stopPropagation()
      undo()
      return true
    }
    if (key === "y" || (key === "z" && e.shiftKey)) {
      e.preventDefault()
      e.stopPropagation()
      redo()
      return true
    }
    return false
  }

  function handleKeyDown(e: React.KeyboardEvent<HTMLInputElement>) {
    if (handleUndoRedoKeyDown(e)) return
    if (e.key === "Enter" || e.key === ",") {
      e.preventDefault()
      commit(draft)
      setDraft("")
    } else if (e.key === "Backspace" && draft === "" && tags.length > 0) {
      removeTag(tags[tags.length - 1])
    }
  }

  /** Accumulates locally instead of calling `commit` per piece — each call would otherwise build
   * from the same stale `tags` closure and only the last piece would survive. */
  function handlePaste(e: React.ClipboardEvent<HTMLInputElement>) {
    const text = e.clipboardData.getData("text")
    if (!text.includes(",")) return
    e.preventDefault()
    const next = [...tags]
    const added: string[] = []
    for (const piece of text.split(/,\s?/)) {
      const tag = normalizeTag(piece)
      if (tag && !next.includes(tag)) {
        next.push(tag)
        added.push(tag)
      }
    }
    onChange(next.join(", "))
    setDraft("")
    added.forEach(reformat)
  }

  return (
    <div style={{ position: "relative" }}>
      {/* Overrides legacy theme CSS's `.tagger` (display: table-cell; max-width: 450px; width: 60%),
          which doesn't lay out correctly without a real table ancestor (issue #45). */}
      <div
        className="tagger"
        style={{ minHeight: 125, width: "auto", maxWidth: "none", display: "block", boxSizing: "border-box", cursor: disabled ? undefined : "text" }}
        onClick={(e) => {
          if (e.target === e.currentTarget || (e.target as HTMLElement).tagName === "UL") {
            inputRef.current?.focus()
          }
        }}
        onKeyDown={handleUndoRedoKeyDown}
      >
        <ul
          style={{
            display: "flex",
            flexWrap: "wrap",
            width: "100%",
            alignItems: "center",
            padding: "4px 0",
            boxSizing: "border-box",
            margin: 0,
            listStyle: "none",
          }}
        >
          {tags.map((tag) => (
            <li key={tag} style={{ margin: "0.4rem 0", paddingLeft: 10, display: "flex" }}>
              {editing?.original === tag ? (
                <a style={{ padding: "4px 8px", display: "inline-flex", alignItems: "center", lineHeight: 1.25, minHeight: "1.5em" }}>
                  <input
                    autoFocus
                    value={editing.draft}
                    size={Math.max(editing.draft.length, 4)}
                    aria-label={t("common.editTag") ?? undefined}
                    onChange={(e) => setEditing({ original: tag, draft: e.target.value })}
                    onKeyDown={(e) => {
                      // Keeps Ctrl+Z on this field's own text rather than the tag list's history.
                      e.stopPropagation()
                      if (e.key === "Enter") {
                        e.preventDefault()
                        finishEdit()
                      } else if (e.key === "Escape") {
                        e.preventDefault()
                        closingRef.current = true
                        setEditing(null)
                      }
                    }}
                    onBlur={finishEdit}
                    style={{
                      border: "none",
                      outline: "none",
                      boxShadow: "none",
                      background: "transparent",
                      color: "inherit",
                      font: "inherit",
                      padding: 0,
                      maxWidth: "100%",
                    }}
                  />
                </a>
              ) : (
                <a
                  title={disabled ? undefined : (t("common.editTagHint") ?? undefined)}
                  onDoubleClick={(e) => {
                    if ((e.target as HTMLElement).closest("button")) return
                    startEdit(tag)
                  }}
                  onPointerDown={(e) => {
                    touchPressRef.current = e.pointerType !== "mouse"
                    if (e.pointerType === "mouse" || disabled) return
                    cancelLongPress()
                    pressTimerRef.current = window.setTimeout(() => startEdit(tag), LONG_PRESS_MS)
                  }}
                  onPointerUp={cancelLongPress}
                  onPointerLeave={cancelLongPress}
                  onPointerCancel={cancelLongPress}
                  onContextMenu={(e) => {
                    // A long press would otherwise also raise the browser's own touch menu.
                    if (!disabled && touchPressRef.current) e.preventDefault()
                  }}
                  style={{
                    WebkitTouchCallout: "none",
                    padding: disabled ? "4px 8px" : "4px 4px 4px 8px",
                    display: "inline-flex",
                    alignItems: "center",
                    // Taller than either font's line box, so a tag whose CJK value falls back to a
                    // font with different metrics is no taller than a Latin one; long tags still wrap.
                    lineHeight: 1.25,
                    minHeight: "1.5em",
                    gap: 4,
                    wordBreak: "break-all",
                    textDecoration: "none",
                  }}
                >
                  {tag}
                  {!disabled && (
                    <button
                      type="button"
                      onClick={() => removeTag(tag)}
                      aria-label={t("common.removeTag") ?? undefined}
                      style={{
                        background: "none",
                        border: "none",
                        padding: 0,
                        margin: 0,
                        cursor: "pointer",
                        color: "inherit",
                        lineHeight: 1,
                        fontSize: "1em",
                      }}
                    >
                      ×
                    </button>
                  )}
                </a>
              )}
            </li>
          ))}
          {!disabled && (
            <li className="tagger-new" style={{ flexGrow: 1, position: "relative", margin: "0.4rem 0", paddingLeft: 10, minWidth: 80 }}>
              <input
                ref={inputRef}
                value={draft}
                onChange={(e) => setDraft(e.target.value)}
                onKeyDown={handleKeyDown}
                onPaste={handlePaste}
                onBlur={() => {
                  if (draft.trim()) {
                    commit(draft)
                    setDraft("")
                  }
                }}
                style={{
                  border: "none",
                  outline: "none",
                  boxShadow: "none",
                  width: "100%",
                  background: "transparent",
                  color: "inherit",
                  font: "inherit",
                  padding: 0,
                }}
              />
              {filteredSuggestions.length > 0 && (
                <ul
                  style={{
                    position: "absolute",
                    zIndex: 100,
                    top: "100%",
                    left: 0,
                    right: 0,
                    margin: 0,
                    padding: 5,
                    listStyle: "none",
                  }}
                >
                  {filteredSuggestions.map((s) => (
                    <li key={s}>
                      <button
                        type="button"
                        onMouseDown={(e) => {
                          e.preventDefault()
                          commit(s)
                          setDraft("")
                        }}
                        style={{
                          textAlign: "left",
                          width: "100%",
                          background: "none",
                          border: "none",
                          padding: "4px 6px",
                          cursor: "pointer",
                          color: "inherit",
                          font: "inherit",
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
