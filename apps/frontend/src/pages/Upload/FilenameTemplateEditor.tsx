import { Fragment, useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import type { PendingFilenameConflict } from '../../api/types'
import { PopupMenu, PopupMenuItem, useMenuPalette } from '../../components/PopupMenu'
import { FONT_SIZE_8PT, FONT_SIZE_10PT, Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from '../../theme'
import { splitFilenameStemAndExt } from './shared'

const TEMPLATE_VARS = [
  'filename',
  'crc',
  'title',
  'ext',
  'date-yyyymmdd',
  'date-yyyy-mm-dd',
  'namespace',
] as const

/** `YYYYMMDD`/`YYYY-MM-DD` (local time, computed fresh each render — the moment the rename is
 * happening, not the original download's timestamp). Keyed by the `date-*` template-variable
 * suffix so `substituteFilenameTemplate`'s lookup is a plain object-key hit. */
function dateTemplateValues(): Record<string, string> {
  const now = new Date()
  const yyyy = String(now.getFullYear())
  const mm = String(now.getMonth() + 1).padStart(2, '0')
  const dd = String(now.getDate()).padStart(2, '0')
  return {
    'date-yyyymmdd': `${yyyy}${mm}${dd}`,
    'date-yyyy-mm-dd': `${yyyy}-${mm}-${dd}`,
  }
}

/** Replaces every `{var}` occurrence in `template` with its real value from `vars` — an
 * unrecognized placeholder is left as-is rather than silently dropped, so a typo is visible
 * instead of vanishing without a trace. */
function substituteFilenameTemplate(template: string, vars: Record<string, string>): string {
  return template.replace(/\{([\w-]+)\}/g, (match, key: string) => vars[key] ?? match)
}

/** What a Shift-click on a template-variable button inserts — wrapped in parentheses (`({var})`)
 * for every variable except `ext`, which gets a leading `.` instead (`.{ext}`), since a file
 * extension is never meaningfully wrapped in parentheses and a plain click already omits the `.`
 * separator. */
function shiftClickInsertion(key: string): string {
  return key === 'ext' ? '.{ext}' : `({${key}})`
}

/** The dropdown offering both resolutions for a `PendingFilenameConflict` — "Overwrite" runs
 * immediately, "Rename and Catalog" signals the caller to open {@link RenamePopover}, anchored off
 * the trigger button's own rect rather than the clicked menu item (which disappears the instant
 * this menu closes). */
export function ConflictMenu({
  anchor,
  onOverwrite,
  onRename,
}: {
  anchor: DOMRect
  onOverwrite: () => void
  onRename: () => void
}) {
  const { t } = useTranslation()
  const pos = anchoredPosition(anchor, 160)
  return (
    <PopupMenu style={{ position: 'fixed', top: pos.top, left: pos.left, zIndex: Z_OVERLAY_CONTENT }}>
      <PopupMenuItem onClick={onOverwrite}>
        <i className="fa fa-clone" aria-hidden="true" style={{ marginRight: 6 }}></i>
        {t('Overwrite')}
      </PopupMenuItem>
      <PopupMenuItem onClick={onRename}>
        <i className="fa fa-i-cursor" aria-hidden="true" style={{ marginRight: 6 }}></i>
        {t('Rename and Catalog')}
      </PopupMenuItem>
    </PopupMenu>
  )
}

/** Picks a `{top, left}` for a `width`-wide floating panel anchored below `rect` (its trigger's
 * `getBoundingClientRect()`).
 *
 * `preferCenter: false` (default, used by `ConflictMenu`): always left-aligns with the trigger,
 * flipping to right-aligned only when left-aligned doesn't fit — a small menu should visually hug
 * its trigger.
 *
 * `preferCenter: true` (used by `RenamePopover`, a wide standalone form panel): picks whichever
 * side pulls the panel closer to the viewport center when both directions have room, since these
 * triggers live inside a right-aligned-icon row and a right-aligned trigger opening rightward
 * would still hug the window edge even when it technically fits.
 *
 * Both modes flip above the trigger when there isn't enough room below. */
function anchoredPosition(
  rect: DOMRect,
  width: number,
  preferCenter = false,
): { top: number; left: number } {
  const margin = 8
  const estimatedHeight = 220
  const fitsLeftAligned = rect.left + width + margin <= window.innerWidth
  const fitsRightAligned = rect.right - width >= margin
  const preferLeftAligned = preferCenter
    ? (rect.left + rect.right) / 2 <= window.innerWidth / 2
    : true
  const useLeftAligned = preferLeftAligned ? fitsLeftAligned || !fitsRightAligned : fitsLeftAligned && !fitsRightAligned
  const left = useLeftAligned ? rect.left : Math.max(rect.right - width, margin)
  const spaceBelow = window.innerHeight - rect.bottom
  const top =
    spaceBelow >= estimatedHeight || spaceBelow >= rect.top
      ? rect.bottom + 4
      : Math.max(rect.top - estimatedHeight - 4, margin)
  return { top, left }
}

/** One piece of a parsed template string — either literal, freely-editable text, or a `{var}`/
 * `({var})`/`.{var}` token rendered as an atomic, non-editable chip with its own `×` remove
 * button.
 *
 * `key` is a stable per-render React key distinct from `value` — two chips with the same literal
 * token text (`{filename}_{filename}`) would otherwise collide on `value` alone. */
type TemplateSegment = { type: 'text'; value: string; key: string } | { type: 'token'; value: string; key: string }

/** A zero-visual-width but genuinely-present text-node character, inserted before and after
 * every chip `<span>` in `renderSegments` so a native browser caret has a text-node anchor to land
 * in near a chip boundary. Two adjacent `contentEditable={false}` chips (or a chip at the very
 * start/end of the editor) otherwise give the browser's caret-placement logic no text-node landing
 * spot at all, leaving clicks there silently placing no visible caret. Filtered back out in
 * `extractTemplateFromDom`/skipped in `setCursorAtOffset`'s length accounting. */
const CURSOR_ANCHOR = '\u200b'

/** Splits a template string into alternating text/token segments — `token` segments capture the
 * optional wrapping `(`/`)` and leading `.` a Shift-click insertion can add (see
 * `shiftClickInsertion`), so the whole thing round-trips back into `substituteFilenameTemplate`
 * unchanged as one atomic unit. Each segment's `key` is its own start offset in `template`, stable
 * across renders as long as nothing before it changed. */
function parseTemplateSegments(template: string): TemplateSegment[] {
  const segments: TemplateSegment[] = []
  let lastIndex = 0
  for (const match of template.matchAll(/\.?\(?\{[\w-]+\}\)?/g)) {
    const start = match.index
    if (start > lastIndex) {
      segments.push({ type: 'text', value: template.slice(lastIndex, start), key: `t${lastIndex}` })
    }
    segments.push({ type: 'token', value: match[0], key: `k${start}` })
    lastIndex = start + match[0].length
  }
  if (lastIndex < template.length) {
    segments.push({ type: 'text', value: template.slice(lastIndex), key: `t${lastIndex}` })
  }
  return segments
}

/** One atomic, non-editable `{var}`/`({var})`/`.{var}` chip inside {@link TemplateInput}.
 * `contentEditable={false}` makes the browser treat it as one indivisible unit inside the outer
 * editable container (arrow keys skip over it, Backspace at its edge deletes the whole thing).
 * `data-token` (read by `extractTemplateFromDom`) carries the chip's literal text separately from
 * its rendered label, since the label's sibling remove-button icon would otherwise pollute a plain
 * `textContent` read.
 *
 * Not draggable — chip-to-chip drag-and-drop reordering was tried and abandoned; see issue #2. */
function TemplateChip({
  value,
  onRemove,
}: {
  value: string
  /** Receives the chip's live DOM node, not a stale parse-time offset — see `offsetOfChipNode`. */
  onRemove: (chipNode: HTMLElement) => void
}) {
  const palette = useMenuPalette()
  const [hovered, setHovered] = useState(false)
  const removeColor = palette.border === 'transparent' ? palette.text : palette.border

  return (
    <span
      contentEditable={false}
      data-token={value}
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: 3,
        padding: '0 2px 0 5px',
        margin: '0 3px', // wide enough to make the gap between adjacent chips actually clickable
        borderRadius: 3,
        background: hovered ? 'rgba(0,0,0,0.16)' : 'rgba(0,0,0,0.08)',
        border: '1px solid rgba(128,128,128,0.4)', // neutral grey, not the theme accent, so it isn't mistaken for the text caret
        fontFamily: 'monospace',
        userSelect: 'none',
        whiteSpace: 'nowrap',
      }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
    >
      <span>{value}</span>
      <button
        type="button"
        aria-label="Remove"
        onClick={(e) => {
          e.preventDefault()
          e.stopPropagation()
          onRemove(e.currentTarget.parentElement as HTMLElement)
        }}
        style={{
          border: 'none',
          background: 'none',
          padding: 0,
          margin: 0,
          cursor: 'pointer',
          color: removeColor,
          fontSize: '0.85em',
          lineHeight: 1,
        }}
      >
        <i className="fa fa-times" aria-hidden="true"></i>
      </button>
    </span>
  )
}

/** A mixed text/chip editor for a filename template string — a `contentEditable` `<div>` (not a
 * plain `<input>`) since a token needs to render as an atomic, visually distinct chip with its own
 * `×` button sitting inline among freely-typed text, which a plain `<input>`'s single text node
 * can't represent. Each chip ({@link TemplateChip}) is `contentEditable={false}` (arrow keys skip
 * over it, Backspace deletes the whole thing) sitting inside the outer `contentEditable="true"`
 * container, which handles typing/selection via native browser behavior.
 *
 * Re-parses `template` into segments on every render (`parseTemplateSegments`), a controlled
 * `contentEditable` with React (not manual `innerHTML`) owning the DOM diff. `onInput` reads the
 * live DOM back out (`extractTemplateFromDom`) rather than tracking incremental text-node edits,
 * since `contentEditable`'s native mutations never go through React's render path.
 *
 * No drag-and-drop of any kind — both native HTML5 DnD and `@dnd-kit` were tried and abandoned;
 * see issue #2. Click/Shift-click (`onInsert`) is the only way to insert a variable; the `×`
 * button on each chip is the only way to remove one. */
function TemplateInput({
  template,
  onChange,
  onInsert,
}: {
  template: string
  onChange: (next: string) => void
  /** Called once (on mount) with this editor's real `insert(text)` function — the caller stashes
   * it (e.g. in a ref) and calls it later from its own template-variable buttons. */
  onInsert: (insert: (text: string) => void) => void
}) {
  const palette = useMenuPalette()
  const editorRef = useRef<HTMLDivElement>(null)
  // The JSX children rendered are derived from this, NOT directly from the `template` prop:
  // `contentEditable`'s native typing mutates the real DOM directly, bypassing React. If
  // `segments` were computed straight from `template` (which changes on every keystroke via
  // `onChange`), React would reconcile its stale virtual-DOM picture against the already-mutated
  // real DOM on every keystroke, getting text-node boundaries wrong and duplicating/misplacing the
  // just-typed character. Plain typing (`onInput` below) still updates `template` via `onChange`
  // for the parent's sake (resolved-filename preview stays live) but deliberately does not update
  // this local state — every other kind of change (insert-from-button, chip removal) does, since
  // those need React to re-render fresh chip JSX.
  const [renderedTemplate, setRenderedTemplate] = useState(template)
  // Set right before a chip is removed or moved, to its former character offset — consumed by the
  // `renderedTemplate`-keyed effect right after React re-renders, so the cursor lands exactly
  // where the chip used to be instead of wherever the browser defaults to.
  const pendingCursorOffsetRef = useRef<number | null>(null)
  const segments = useMemo(() => parseTemplateSegments(renderedTemplate), [renderedTemplate])

  /** Places the caret at flat character offset `offset` (as `extractTemplateFromDom` would count
   * it) by walking the editor's direct children, which are always either text nodes or one flat
   * level of chip `<span>`s (never nested) — a plain linear scan tracking how many flat characters
   * each child accounts for. Landing exactly on a chip boundary collapses to just after it (chips
   * are `contentEditable={false}`, so a native caret can never be placed *inside* one anyway). */
  function setCursorAtOffset(offset: number) {
    const root = editorRef.current
    if (!root) return
    const selection = window.getSelection()
    if (!selection) return
    let remaining = offset
    for (const node of root.childNodes) {
      // A pure `CURSOR_ANCHOR` text node (see that constant's own docs) contributes zero real
      // characters — it's a rendering-only caret landing spot, not part of the flat template
      // string `offset` is measured against, so it's skipped entirely rather than treated as a
      // normal zero/one-character text node candidate for the caret to land in.
      if (node.nodeType === Node.TEXT_NODE && node.textContent === CURSOR_ANCHOR) continue
      const length =
        node.nodeType === Node.TEXT_NODE
          ? (node.textContent?.length ?? 0)
          : node instanceof HTMLElement
            ? (node.dataset.token?.length ?? 0)
            : 0
      if (remaining <= length) {
        const range = document.createRange()
        if (node.nodeType === Node.TEXT_NODE) {
          range.setStart(node, remaining)
        } else {
          // A chip boundary (`remaining` is 0 or equals this chip's own length) — position the
          // range immediately before/after the chip node itself, since text offsets don't apply
          // inside an atomic, non-text chip.
          const parent = node.parentNode
          if (!parent) return
          const index = Array.prototype.indexOf.call(parent.childNodes, node)
          range.setStart(parent, remaining === 0 ? index : index + 1)
        }
        range.collapse(true)
        selection.removeAllRanges()
        selection.addRange(range)
        return
      }
      remaining -= length
    }
    // `offset` was past the end of every child (e.g. the deleted chip was the last segment) —
    // collapse to the very end of the editor instead.
    const range = document.createRange()
    range.selectNodeContents(root)
    range.collapse(false)
    selection.removeAllRanges()
    selection.addRange(range)
  }

  /** Reads the live DOM back into a flat template string — chips carry their own literal token
   * text on a `data-token` attribute (rather than reading `textContent`, which would include the
   * `×` button's own label) set by {@link TemplateChip} itself. Strips out every `CURSOR_ANCHOR`
   * character (the zero-width text nodes rendered around each chip purely so the browser has
   * somewhere to put a real caret) — those are a rendering-layer implementation detail, never part
   * of the actual template content this editor represents. */
  function extractTemplateFromDom(): string {
    const root = editorRef.current
    if (!root) return template
    let result = ''
    for (const node of root.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        result += node.textContent ?? ''
      } else if (node instanceof HTMLElement && node.dataset.token) {
        result += node.dataset.token
      }
    }
    return result.split(CURSOR_ANCHOR).join('')
  }

  /** Sums the flat-offset contribution of the editor's direct children up to (but not including)
   * `stopAt` — the shared walk both {@link offsetFromRange} and {@link offsetOfChipNode} need, so
   * "where does this DOM position/node fall in the flat template string" has exactly one
   * implementation. Returns `null` if `stopAt` is never found among `root`'s direct children. */
  function offsetUpTo(root: HTMLElement, stopAt: Node): number | null {
    let offset = 0
    for (const node of root.childNodes) {
      if (node === stopAt) return offset
      if (node.nodeType === Node.TEXT_NODE && node.textContent === CURSOR_ANCHOR) continue
      offset +=
        node.nodeType === Node.TEXT_NODE
          ? (node.textContent?.length ?? 0)
          : node instanceof HTMLElement
            ? (node.dataset.token?.length ?? 0)
            : 0
    }
    return null
  }

  /** Converts a real DOM `Range` (the current selection) into a flat character offset in the same
   * counting scheme `extractTemplateFromDom`/`setCursorAtOffset` use — the inverse of
   * `setCursorAtOffset`. Returns `null` if the range's container isn't a direct child of (or isn't)
   * the editor — e.g. it landed on the remove-button icon inside a chip. */
  function offsetFromRange(range: Range): number | null {
    const root = editorRef.current
    if (!root) return null
    let container = range.startContainer
    // The selection can land inside a chip's own child nodes (label/remove-button), not just the
    // chip `<span>` itself — walk up to the nearest direct child of the editor first.
    while (container.parentNode && container.parentNode !== root) {
      container = container.parentNode
    }
    if (container.parentNode !== root) return null
    const base = offsetUpTo(root, container)
    if (base === null) return null
    return base + (container.nodeType === Node.TEXT_NODE ? Math.min(range.startOffset, container.textContent?.length ?? 0) : 0)
  }

  /** Finds `chipNode`'s current flat offset in the live DOM ({@link offsetUpTo}) — deliberately
   * NOT derived from `segment.key`, which is only valid as of whatever render produced `segments`
   * and can be stale relative to the live DOM after a plain keystroke. Every structural op (remove,
   * insert) must locate its target fresh, from the DOM, at the moment it acts, or it risks
   * corrupting unrelated plain text elsewhere in the editor. */
  function offsetOfChipNode(chipNode: ChildNode): number | null {
    const root = editorRef.current
    if (!root) return null
    return offsetUpTo(root, chipNode)
  }

  /** Inserts `text` at flat offset `dropOffset` — used by the click/Shift-click insert path
   * (`onInsert`, below). Bases the edit on `extractTemplateFromDom()`, the true current content
   * read fresh, not `renderedTemplate` or the `template` prop, either of which can be one render
   * behind the live DOM that `dropOffset` was computed against. */
  function insertAtOffset(text: string, dropOffset: number) {
    const base = extractTemplateFromDom()
    pendingCursorOffsetRef.current = dropOffset + text.length
    const next = base.slice(0, dropOffset) + text + base.slice(dropOffset)
    setRenderedTemplate(next)
    onChange(next)
  }

  useEffect(() => {
    if (pendingCursorOffsetRef.current !== null) {
      editorRef.current?.focus()
      setCursorAtOffset(pendingCursorOffsetRef.current)
      pendingCursorOffsetRef.current = null
    }
  }, [renderedTemplate])

  useEffect(() => {
    editorRef.current?.focus()
    // Places the initial cursor right before the extension (the `.` of `.{ext}`), not the browser
    // default (start of the field) — the stem/CRC portion is what a user typically wants to edit
    // first. Only correct because the default template's shape (`{filename}_{crc}.{ext}`) is
    // fixed, so this offset doesn't need to know anything about the actual filename.
    setCursorAtOffset(template.length - '.{ext}'.length)
    // Runs once on mount only — subsequent re-renders are handled by the `renderedTemplate`-keyed
    // effect above; re-running this on every render would re-focus/move the cursor on every
    // keystroke.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    // Registers an `insert(text)` function the parent (the template-variable buttons) calls —
    // inserts at the current DOM selection (falling back to the end when the editor doesn't have
    // focus/a real selection inside it, e.g. right after this popover first opens and a button is
    // clicked before ever touching the editor itself).
    onInsert((text) => {
      const root = editorRef.current
      if (!root) return
      const selection = window.getSelection()
      const range =
        selection && selection.rangeCount > 0 && root.contains(selection.anchorNode)
          ? selection.getRangeAt(0)
          : null
      const offset = range ? (offsetFromRange(range) ?? extractTemplateFromDom().length) : extractTemplateFromDom().length
      insertAtOffset(text, offset)
    })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div
      ref={editorRef}
      contentEditable
      suppressContentEditableWarning
      role="textbox"
      aria-multiline="false"
      className="stdinput"
      style={{
        width: '100%',
        boxSizing: 'border-box',
        background: '#e8e8e8',
        color: '#000',
        // A distinct caret color (the theme accent, falling back to `palette.text` on themes
        // where `border` is transparent) so it isn't mistaken for a chip's neutral grey border.
        caretColor: palette.border === 'transparent' ? palette.text : palette.border,
        minHeight: '1.6em',
        outline: 'none',
        wordBreak: 'break-all',
      }}
      onInput={() => {
        // Deliberately does NOT update `renderedTemplate` — see that state's own docs above for
        // why plain typing must not feed back into the rendered JSX. `onChange` still fires so the
        // parent's resolved-filename preview stays live.
        onChange(extractTemplateFromDom())
      }}
      onKeyDown={(e) => {
        // A plain `Enter` inside a single-line field should submit the form, not insert a
        // newline `contentEditable` would otherwise happily create.
        if (e.key === 'Enter') e.preventDefault()
      }}
    >
      {segments.map((segment) =>
        segment.type === 'text' ? (
          segment.value
        ) : (
          // See `CURSOR_ANCHOR`'s own docs for why a chip needs one on both sides — added
          // unconditionally since it's harmless even when a real text node is already adjacent.
          <Fragment key={segment.key}>
            {CURSOR_ANCHOR}
            <TemplateChip
              value={segment.value}
              onRemove={(chipNode) => {
                const tokenStart = offsetOfChipNode(chipNode)
                if (tokenStart === null) return
                pendingCursorOffsetRef.current = tokenStart
                const dom = extractTemplateFromDom()
                const next = dom.slice(0, tokenStart) + dom.slice(tokenStart + segment.value.length)
                setRenderedTemplate(next)
                onChange(next)
              }}
            />
            {CURSOR_ANCHOR}
          </Fragment>
        ),
      )}
    </div>
  )
}

/** One `{var}`-insertion button in {@link RenamePopover}'s button row. `.stdbtn`'s theme default
 * carries `min-width: 150px`, wildly oversized for a small "insert this token" chip, so this
 * deliberately doesn't use that class (a plain themed-border button instead).
 *
 * Deliberately NOT draggable — drag-to-insert into `TemplateInput`'s `contentEditable` was tried
 * and abandoned (see issue #2): Chromium's `dragover`/`preventDefault()` fired correctly, yet
 * `drop` never dispatched, a known `contentEditable` vs. native drag-and-drop conflict
 * (whatwg/html#3114) with no working workaround. Click/Shift-click is the only way to insert a
 * variable — no drag-and-drop of any kind remains in this editor. */
function TemplateVarButton({
  label,
  palette,
  onClick,
}: {
  label: string
  palette: ReturnType<typeof useMenuPalette>
  onClick: (e: React.MouseEvent<HTMLButtonElement>) => void
}) {
  const [hovered, setHovered] = useState(false)
  return (
    <button
      type="button"
      style={{
        fontSize: FONT_SIZE_8PT,
        padding: '1px 5px',
        minWidth: 0,
        width: 'auto',
        border: '1px solid rgba(128,128,128,0.4)',
        borderRadius: 3,
        background: 'transparent',
        cursor: 'pointer',
        // `palette.border` is `transparent` on 2 of this app's 5 themes — an outline in that
        // color would be invisible on hover, so `palette.text` is used instead.
        outline: hovered ? `1px solid ${palette.text}` : 'none',
        outlineOffset: '1px',
      }}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onClick={onClick}
    >
      {label}
    </button>
  )
}

/** The "重命名并入库" popover — lets the user resolve a `PendingFilenameConflict` by cataloguing
 * the already-downloaded, already-staged bytes under a new filename instead of overwriting the
 * archive that owns the original one. The input holds a template string (starting from the
 * default `{filename}_{crc}.{ext}`), not an already-resolved filename — the buttons insert literal
 * `{var}` placeholders at the cursor, and only `onConfirm` substitutes real values, once, at
 * submit time. Styled as a `PopupMenu` item rather than a new popover component since a form
 * doesn't fit `PopupMenuItem`'s plain-clickable-row model. */
export function RenamePopover({
  anchor,
  conflict,
  itemTitle,
  itemNamespace,
  pending,
  onCancel,
  onConfirm,
}: {
  anchor: { x: number; y: number }
  conflict: PendingFilenameConflict
  itemTitle: string | null
  itemNamespace: string
  pending: boolean
  onCancel: () => void
  onConfirm: (filename: string) => void
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const { stem, ext } = splitFilenameStemAndExt(conflict.original_filename)
  const [template, setTemplate] = useState('{filename}_{crc}.{ext}')
  // 280px keeps the default `{filename}_{crc}.{ext}` template on one line once each token
  // renders as its own bordered/padded chip.
  const width = 280
  const { top, left } = anchoredPosition(
    new DOMRect(anchor.x, anchor.y, 0, 0),
    width,
    true,
  )

  const vars: Record<string, string> = {
    filename: stem,
    crc: conflict.crc32,
    title: itemTitle ?? '',
    ext,
    ...dateTemplateValues(),
    // Uppercased since a plugin namespace (e.g. `ehdl`) reads more like a stable identifier/tag
    // when shouted, unlike the other lowercase-content template variables here.
    namespace: itemNamespace.toUpperCase(),
  }
  const resolved = substituteFilenameTemplate(template, vars)

  // `TemplateInput` registers its own `insert(text)` function here once mounted (it needs access
  // to the live DOM selection inside its `contentEditable`, unreachable from this parent) — the
  // template-variable buttons below call whatever's currently registered.
  const insertRef = useRef<(text: string) => void>(() => {})

  return (
    <>
      <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={onCancel} />
      <PopupMenu style={{ position: 'fixed', top, left, zIndex: Z_OVERLAY_CONTENT }}>
        <li style={{ listStyle: 'none', padding: '6px 10px', width }}>
          <div style={{ fontSize: FONT_SIZE_10PT, marginBottom: 4 }}>{t('New filename')}</div>
          <TemplateInput
            template={template}
            onChange={setTemplate}
            onInsert={(insert) => {
              insertRef.current = insert
            }}
          />
          <div style={{ display: 'flex', gap: 4, marginTop: 6, flexWrap: 'wrap' }}>
            {TEMPLATE_VARS.map((key) => (
              <TemplateVarButton
                key={key}
                label={`{${key}}`}
                palette={palette}
                onClick={(e) => insertRef.current(e.shiftKey ? shiftClickInsertion(key) : `{${key}}`)}
              />
            ))}
          </div>
          {/* `{ext}` is an exception to the general "wrapped in parentheses" rule (see
              `shiftClickInsertion`), so it needs its own explanatory line. */}
          <div style={{ fontSize: FONT_SIZE_8PT, opacity: 0.6, marginTop: 4 }}>
            <div>{t('Shift-click to insert wrapped in parentheses')}</div>
            <div>{t('Shift-click {{ext}} to insert with a leading dot instead', { ext: '{ext}' })}</div>
          </div>
          <code
            style={{
              display: 'block',
              fontSize: FONT_SIZE_8PT,
              marginTop: 6,
              padding: '3px 5px',
              background: 'rgba(0,0,0,0.06)',
              borderRadius: 3,
              wordBreak: 'break-all',
              minHeight: '1.2em',
            }}
          >
            {resolved}
          </code>
          <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 6, marginTop: 8 }}>
            <button
              type="button"
              className="stdbtn"
              style={{ minWidth: 0, width: 'auto', padding: '2px 8px' }}
              onClick={onCancel}
            >
              {t('Cancel')}
            </button>
            <button
              type="button"
              className="stdbtn"
              style={{ minWidth: 0, width: 'auto', padding: '2px 8px' }}
              disabled={pending || !resolved.trim()}
              onClick={() => onConfirm(resolved)}
            >
              {t('Rename and Catalog')}
            </button>
          </div>
        </li>
      </PopupMenu>
    </>
  )
}
