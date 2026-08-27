import { flip, offset, type Placement, shift, size, useFloating } from "@floating-ui/react"
import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import type { PendingFilenameConflict } from "@/api/types"
import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FONT_SIZE_SM, FONT_SIZE_XS, Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

import { splitFilenameStemAndExt } from "./shared"

/** Positions a floating panel against a fixed virtual point — `RenamePopover`'s own anchor is a
 * click position, not a persistent element, so (unlike `ConflictMenu`, which hands Floating UI a
 * real element ref) a one-time virtual reference is correct here, not just simpler. */
function useAnchoredFloating(rect: DOMRect, placement: Placement) {
  const { refs, floatingStyles, isPositioned } = useFloating({
    strategy: "absolute",
    placement,
    middleware: [
      offset(4),
      flip({ padding: 8 }),
      shift({ padding: 8 }),
      size({
        padding: 8,
        apply({ availableHeight, elements }) {
          elements.floating.style.maxHeight = `${availableHeight}px`
          elements.floating.style.overflowY = "auto"
        },
      }),
    ],
  })
  useLayoutEffect(() => {
    refs.setReference({ getBoundingClientRect: () => rect })
    // `rect`'s own fields (not the `DOMRect` object identity, which is fresh every render even for
    // an anchor that hasn't actually moved) are the real dependency — comparing by value here is
    // what keeps this from redundantly re-registering the reference every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rect.top, rect.left, rect.right, rect.bottom, rect.width, rect.height])
  // See `ConflictMenu`'s own docs — `floatingStyles` carries stale/0x0-measured coordinates until
  // Floating UI's first real position pass completes; `isPositioned` gates that.
  return { refs, floatingStyles, isPositioned }
}

const TEMPLATE_VARS = [
  "filename",
  "crc",
  "title",
  "ext",
  "date-yyyymmdd",
  "date-yyyy-mm-dd",
  "namespace",
] as const

/** `YYYYMMDD`/`YYYY-MM-DD` (local time, computed fresh each render — the moment the rename is
 * happening, not the original download's timestamp). Keyed by the `date-*` template-variable
 * suffix so `substituteFilenameTemplate`'s lookup is a plain object-key hit. */
function dateTemplateValues(): Record<string, string> {
  const now = new Date()
  const yyyy = String(now.getFullYear())
  const mm = String(now.getMonth() + 1).padStart(2, "0")
  const dd = String(now.getDate()).padStart(2, "0")
  return {
    "date-yyyymmdd": `${yyyy}${mm}${dd}`,
    "date-yyyy-mm-dd": `${yyyy}-${mm}-${dd}`,
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
  return key === "ext" ? ".{ext}" : `({${key}})`
}

/** The dropdown offering both resolutions for a `PendingFilenameConflict` — "Overwrite" runs
 * immediately, "Rename and Catalog" signals the caller to open {@link RenamePopover}, anchored off
 * the trigger button's own rect rather than the clicked menu item (which disappears the instant
 * this menu closes).
 *
 * Renders NON-portaled (`portal={false}`) — the caller wraps its trigger button in a
 * `position: relative` container and renders this menu as a sibling inside it, so `position:
 * absolute` resolves against that wrapper directly. No Floating UI viewport/document coordinate
 * math involved, so the menu scrolls with the page as part of its trigger row's own content.
 * `@floating-ui/react`'s `strategy: "absolute"` was tried first and measured to have a real
 * coordinate bug in this exact DOM shape (portaled to `document.body`): `size()`'s own `apply`
 * callback showed `rects.reference.y` off from the trigger's real `getBoundingClientRect().y` by
 * exactly `window.scrollY`, never correctly subtracted back out. Sidestepping Floating UI's
 * coordinate system entirely avoids that bug rather than working around it.
 *
 * Flips to open ABOVE the trigger when there isn't enough room below, and to RIGHT-align (instead
 * of the default left-align) when there isn't enough room to the right — own `useLayoutEffect`
 * measuring real `getBoundingClientRect()`s, not Floating UI's `flip()`/`shift()` (those require
 * the same buggy coordinate pipeline this component exists to avoid). Reported live: dropping the
 * vertical check entirely left the menu clipped off the bottom of the viewport near the end of a
 * long list; a left-only horizontal anchor (no corresponding flip) left it clipped off the RIGHT
 * edge whenever the trigger button itself sat near the right edge of a narrow viewport (a wide
 * three-item menu overflowing a trigger with little room to its right). */
export function ConflictMenu({
  onOverwrite,
  onRename,
  onCompare,
}: {
  onOverwrite: () => void
  onRename: () => void
  /** Optional (issue #77's AI quality-comparison judgment, read-only) — the caller decides
   * whether this option is even worth offering (e.g. hidden if the existing archive's own file
   * has since gone missing), so this menu doesn't hard-require it. */
  onCompare?: () => void
}) {
  const { t } = useTranslation()
  const menuRef = useRef<HTMLUListElement | null>(null)
  const [openAbove, setOpenAbove] = useState<boolean | null>(null)
  const [alignRight, setAlignRight] = useState(false)
  useLayoutEffect(() => {
    const el = menuRef.current
    const wrapper = el?.parentElement
    if (!el || !wrapper) return
    const wrapperRect = wrapper.getBoundingClientRect()
    const menuRect = el.getBoundingClientRect()
    const spaceBelow = window.innerHeight - wrapperRect.bottom
    setOpenAbove(spaceBelow < menuRect.height + 8 && wrapperRect.top >= menuRect.height + 8)
    // Left-aligned (`left: 0`, the default) overflows the viewport's right edge whenever the
    // trigger doesn't have `menuRect.width` of room to its right — right-align instead
    // (`right: 0`, i.e. the menu's own right edge lines up with the trigger's right edge) so the
    // menu grows leftward into the room that actually exists, same idea as the vertical flip.
    const spaceRight = window.innerWidth - wrapperRect.left
    setAlignRight(spaceRight < menuRect.width + 8 && wrapperRect.right >= menuRect.width + 8)
  }, [])
  return (
    <PopupMenu
      ref={menuRef}
      portal={false}
      style={{
        position: "absolute",
        ...(openAbove ? { bottom: "100%", marginBottom: 4 } : { top: "100%", marginTop: 4 }),
        ...(alignRight ? { right: 0 } : { left: 0 }),
        visibility: openAbove === null ? "hidden" : "visible",
        zIndex: Z_OVERLAY_CONTENT,
      }}
    >
      {onCompare && (
        <PopupMenuItem onClick={onCompare}>
          <i className="fa fa-robot" aria-hidden="true" style={{ marginRight: 6 }}></i>
          {t("upload.aiSuggestion")}
        </PopupMenuItem>
      )}
      <PopupMenuItem onClick={onOverwrite}>
        <i className="fa fa-clone" aria-hidden="true" style={{ marginRight: 6 }}></i>
        {t("upload.overwrite")}
      </PopupMenuItem>
      <PopupMenuItem onClick={onRename}>
        <i className="fa fa-i-cursor" aria-hidden="true" style={{ marginRight: 6 }}></i>
        {t("upload.renameAndCatalog")}
      </PopupMenuItem>
    </PopupMenu>
  )
}

/** Resolves a `DOMRect` anchor (or `preferCenter`'s viewport-center-relative rule) to a Floating
 * UI `Placement` — `useAnchoredFloating` (below) hands this straight to `useFloating`'s own
 * `placement` option as the PREFERRED side/alignment before `flip`/`shift` correct it against real
 * measured content, so this only needs to express the same left/right PREFERENCE the old
 * hand-rolled `anchoredPosition` did, not the actual final on-screen position — that part is
 * `@floating-ui/react`'s job now (see `useAnchoredFloating`'s own docs for why: two real, reported
 * live bugs in a previous hand-rolled implementation — a wrong "which side has more room" flip
 * check, and no post-flip clamp to catch an overflowing result — are exactly the class of bug
 * `flip()` + `shift()` exist to rule out structurally, not just patch case-by-case). */
function preferredPlacement(rect: DOMRect, preferCenter: boolean): "bottom-start" | "bottom-end" {
  const preferLeftAligned = preferCenter
    ? (rect.left + rect.right) / 2 <= window.innerWidth / 2
    : true
  return preferLeftAligned ? "bottom-start" : "bottom-end"
}

/** One piece of a parsed template string — either literal, freely-editable text, or a `{var}`/
 * `({var})`/`.{var}` token rendered as an atomic, non-editable chip with its own `×` remove
 * button.
 *
 * `key` is a stable per-render React key distinct from `value` — two chips with the same literal
 * token text (`{filename}_{filename}`) would otherwise collide on `value` alone. */
type TemplateSegment = { type: "text"; value: string; key: string } | { type: "token"; value: string; key: string }

/** A zero-visual-width but genuinely-present text-node character, inserted before and after
 * every chip `<span>` in `renderSegments` so a native browser caret has a text-node anchor to land
 * in near a chip boundary. Two adjacent `contentEditable={false}` chips (or a chip at the very
 * start/end of the editor) otherwise give the browser's caret-placement logic no text-node landing
 * spot at all, leaving clicks there silently placing no visible caret. Filtered back out in
 * `extractTemplateFromDom`/skipped in `setCursorAtOffset`'s length accounting. */
const CURSOR_ANCHOR = "\u200b"

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
      segments.push({ type: "text", value: template.slice(lastIndex, start), key: `t${lastIndex}` })
    }
    segments.push({ type: "token", value: match[0], key: `k${start}` })
    lastIndex = start + match[0].length
  }
  if (lastIndex < template.length) {
    segments.push({ type: "text", value: template.slice(lastIndex), key: `t${lastIndex}` })
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
  const removeColor = palette.border === "transparent" ? palette.text : palette.border

  return (
    <span
      contentEditable={false}
      data-token={value}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 3,
        padding: "0 2px 0 5px",
        margin: "0 3px", // wide enough to make the gap between adjacent chips actually clickable
        borderRadius: 3,
        background: hovered ? "rgba(0,0,0,0.16)" : "rgba(0,0,0,0.08)",
        border: "1px solid rgba(128,128,128,0.4)", // neutral grey, not the theme accent, so it isn't mistaken for the text caret
        fontFamily: "monospace",
        userSelect: "none",
        whiteSpace: "nowrap",
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
          border: "none",
          background: "none",
          padding: 0,
          margin: 0,
          cursor: "pointer",
          color: removeColor,
          fontSize: "0.85em",
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
    let result = ""
    for (const node of root.childNodes) {
      if (node.nodeType === Node.TEXT_NODE) {
        result += node.textContent ?? ""
      } else if (node instanceof HTMLElement && node.dataset.token) {
        result += node.dataset.token
      }
    }
    return result.split(CURSOR_ANCHOR).join("")
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
    setCursorAtOffset(template.length - ".{ext}".length)
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
        width: "100%",
        boxSizing: "border-box",
        background: "#e8e8e8",
        color: "#000",
        // A distinct caret color (the theme accent, falling back to `palette.text` on themes
        // where `border` is transparent) so it isn't mistaken for a chip's neutral grey border.
        caretColor: palette.border === "transparent" ? palette.text : palette.border,
        minHeight: "1.6em",
        outline: "none",
        wordBreak: "break-all",
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
        if (e.key === "Enter") e.preventDefault()
      }}
    >
      {segments.map((segment) =>
        segment.type === "text" ? (
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
        fontSize: FONT_SIZE_XS,
        padding: "1px 5px",
        minWidth: 0,
        width: "auto",
        border: "1px solid rgba(128,128,128,0.4)",
        borderRadius: 3,
        background: "transparent",
        cursor: "pointer",
        // `palette.border` is `transparent` on 2 of this app's 5 themes — an outline in that
        // color would be invisible on hover, so `palette.text` is used instead.
        outline: hovered ? `1px solid ${palette.text}` : "none",
        outlineOffset: "1px",
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
  const [template, setTemplate] = useState("{filename}_{crc}.{ext}")
  // 280px keeps the default `{filename}_{crc}.{ext}` template on one line once each token
  // renders as its own bordered/padded chip.
  const width = 280
  // `anchor` is a plain point (this popover opens off a click position, not a trigger element's
  // own box) — wrapped in a zero-size `DOMRect` so `useAnchoredFloating` has a consistent
  // `DOMRect`-shaped anchor to work with regardless of which caller it's serving.
  const anchorRect = useMemo(() => new DOMRect(anchor.x, anchor.y, 0, 0), [anchor.x, anchor.y])
  const { refs, floatingStyles, isPositioned } = useAnchoredFloating(
    anchorRect,
    preferredPlacement(anchorRect, true),
  )

  const vars: Record<string, string> = {
    filename: stem,
    crc: conflict.crc32,
    title: itemTitle ?? "",
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
      <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={onCancel} />
      <PopupMenu
        ref={refs.setFloating}
        style={{
          ...floatingStyles,
          visibility: isPositioned ? "visible" : "hidden",
          zIndex: Z_OVERLAY_CONTENT,
        }}
      >
        <li style={{ listStyle: "none", padding: "6px 10px", width }}>
          <div style={{ fontSize: FONT_SIZE_SM, marginBottom: 4 }}>{t("upload.newFilename")}</div>
          <TemplateInput
            template={template}
            onChange={setTemplate}
            onInsert={(insert) => {
              insertRef.current = insert
            }}
          />
          <div style={{ display: "flex", gap: 4, marginTop: 6, flexWrap: "wrap" }}>
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
          <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.6, marginTop: 4 }}>
            <div>{t("upload.shiftclickToInsertWrappedIn")}</div>
            <div>{t("upload.shiftclickExtToInsertWith", { ext: "{ext}" })}</div>
          </div>
          <code
            style={{
              display: "block",
              fontSize: FONT_SIZE_XS,
              marginTop: 6,
              padding: "3px 5px",
              background: "rgba(0,0,0,0.06)",
              borderRadius: 3,
              wordBreak: "break-all",
              minHeight: "1.2em",
            }}
          >
            {resolved}
          </code>
          <div style={{ display: "flex", justifyContent: "flex-end", gap: 6, marginTop: 8 }}>
            <button
              type="button"
              className="stdbtn"
              style={{ minWidth: 0, width: "auto", padding: "2px 8px" }}
              onClick={onCancel}
            >
              {t("common.cancel")}
            </button>
            <button
              type="button"
              className="stdbtn"
              style={{ minWidth: 0, width: "auto", padding: "2px 8px" }}
              disabled={pending || !resolved.trim()}
              onClick={() => onConfirm(resolved)}
            >
              {t("upload.renameAndCatalog")}
            </button>
          </div>
        </li>
      </PopupMenu>
    </>
  )
}
