import { flip, offset, type Placement, shift, size, useFloating } from "@floating-ui/react"
import { Fragment, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import type { PendingFilenameConflict } from "@/api/types"
import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { FONT_SIZE_SM, FONT_SIZE_XS, Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

import { splitFilenameStemAndExt } from "./shared"

/** Floating-panel positioning against a fixed virtual point (a click position, not an element). */
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rect.top, rect.left, rect.right, rect.bottom, rect.width, rect.height])
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

/** Local-time `date-*` template values, computed fresh each render. */
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

/** Unrecognized `{var}` placeholders are left as-is, so typos stay visible. */
function substituteFilenameTemplate(template: string, vars: Record<string, string>): string {
  return template.replace(/\{([\w-]+)\}/g, (match, key: string) => vars[key] ?? match)
}

/** Shift-click insertion: `({var})`, except `ext` which gets `.{ext}`. */
function shiftClickInsertion(key: string): string {
  return key === "ext" ? ".{ext}" : `({${key}})`
}

/** Conflict-resolution dropdown; non-portaled inside the trigger's `position: relative` wrapper
 * to avoid a scroll-offset coordinate bug. Flips above/right on insufficient room. */
export function ConflictMenu({
  onOverwrite,
  onRename,
  onCompare,
}: {
  onOverwrite: () => void
  onRename: () => void
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

/** Resolves an anchor `DOMRect` to the preferred Floating UI placement (flip/shift do the rest). */
function preferredPlacement(rect: DOMRect, preferCenter: boolean): "bottom-start" | "bottom-end" {
  const preferLeftAligned = preferCenter
    ? (rect.left + rect.right) / 2 <= window.innerWidth / 2
    : true
  return preferLeftAligned ? "bottom-start" : "bottom-end"
}

/** One parsed segment: literal text, or an atomic non-editable token chip. `key` is a stable
 * per-render React key — duplicate token values would collide on `value` alone. */
type TemplateSegment = { type: "text"; value: string; key: string } | { type: "token"; value: string; key: string }

/** Zero-width caret landing spot rendered around each chip — without a text node there, the
 * browser silently places no caret near chip boundaries. Stripped back out on extraction. */
const CURSOR_ANCHOR = "\u200b"

/** Splits into alternating text/token segments; each `key` is its start offset in `template`. */
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

/** One atomic, non-editable token chip; `data-token` carries its literal text (the rendered
 * label's remove-button icon would pollute a plain `textContent` read). */
function TemplateChip({
  value,
  onRemove,
}: {
  value: string
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
        margin: "0 3px", // keeps the gap between adjacent chips clickable
        borderRadius: 3,
        background: hovered ? "rgba(0,0,0,0.16)" : "rgba(0,0,0,0.08)",
        border: "1px solid rgba(128,128,128,0.4)",
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

/** Mixed text/chip `contentEditable` template editor; `onInput` reads the live DOM back out since
 * native mutations bypass React. No drag-and-drop — click/Shift-click inserts, `×` removes. */
function TemplateInput({
  template,
  onChange,
  onInsert,
}: {
  template: string
  onChange: (next: string) => void
  /** Called once on mount with this editor's `insert(text)` function. */
  onInsert: (insert: (text: string) => void) => void
}) {
  const palette = useMenuPalette()
  const editorRef = useRef<HTMLDivElement>(null)
  const [renderedTemplate, setRenderedTemplate] = useState(template)
  const pendingCursorOffsetRef = useRef<number | null>(null)
  const segments = useMemo(() => parseTemplateSegments(renderedTemplate), [renderedTemplate])

  /** Places the caret at flat character offset `offset`, walking the editor's direct children. */
  function setCursorAtOffset(offset: number) {
    const root = editorRef.current
    if (!root) return
    const selection = window.getSelection()
    if (!selection) return
    let remaining = offset
    for (const node of root.childNodes) {
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
    const range = document.createRange()
    range.selectNodeContents(root)
    range.collapse(false)
    selection.removeAllRanges()
    selection.addRange(range)
  }

  /** Reads the live DOM back into a flat template string, stripping `CURSOR_ANCHOR` characters. */
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

  /** Flat-offset contribution of the editor's direct children up to (not including) `stopAt`. */
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

  /** Converts the current selection `Range` into a flat character offset (inverse of
   * `setCursorAtOffset`); `null` if it isn't inside the editor's direct children. */
  function offsetFromRange(range: Range): number | null {
    const root = editorRef.current
    if (!root) return null
    let container = range.startContainer
    while (container.parentNode && container.parentNode !== root) {
      container = container.parentNode
    }
    if (container.parentNode !== root) return null
    const base = offsetUpTo(root, container)
    if (base === null) return null
    return base + (container.nodeType === Node.TEXT_NODE ? Math.min(range.startOffset, container.textContent?.length ?? 0) : 0)
  }

  /** The chip node's current flat offset, read fresh from the DOM — `segment.key` can be stale
   * after a plain keystroke. */
  function offsetOfChipNode(chipNode: ChildNode): number | null {
    const root = editorRef.current
    if (!root) return null
    return offsetUpTo(root, chipNode)
  }

  /** Inserts `text` at flat offset `dropOffset`, based on the fresh DOM read. */
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
    setCursorAtOffset(template.length - ".{ext}".length)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
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
        onChange(extractTemplateFromDom())
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.preventDefault()
      }}
    >
      {segments.map((segment) =>
        segment.type === "text" ? (
          segment.value
        ) : (
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

/** One `{var}`-insertion button — not `.stdbtn`, whose 150px min-width dwarfs a token chip. */
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
        // `palette.border` is transparent on some themes — the text color stays visible.
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

/** The "重命名并入库" popover — holds a template string (default `{filename}_{crc}.{ext}`);
 * variables are substituted only at `onConfirm` time. */
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
  const width = 280
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
    namespace: itemNamespace.toUpperCase(),
  }
  const resolved = substituteFilenameTemplate(template, vars)

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
