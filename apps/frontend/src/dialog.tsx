import type { ReactNode } from "react"
import { useEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useTranslation } from "react-i18next"

import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { Tooltip } from "@/components/common-ui/Display"
import { Checkbox, Input } from "@/components/common-ui/Form"
import { SearchSyntaxHelp } from "@/components/Display"

import { useStats } from "./api/hooks"
import { useMenuPalette } from "./hooks/useMenuPalette"
import { buildSearchToken } from "./lib/tagFormat"
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "./theme"

// Themed prompt/confirm replacements driven by module-level state, rendered by <DialogHost />.

export type NewCategoryResult = { name: string; isDynamic: boolean; search: string; visibleToGuest: boolean }

/** One of 8 rect-border points the icon can be anchored to (wire-format short codes). */
export type StampAnchor = "tl" | "t" | "tr" | "r" | "br" | "b" | "bl" | "l"

const STAMP_ANCHORS: StampAnchor[] = ["tl", "t", "tr", "r", "br", "b", "bl", "l"]

/** How the rect's interior is rendered — mosaic/blur obscure via backdrop-filter (no real CSS
 * pixelation exists, so mosaic is a strong blur). */
export type StampFill = "solid" | "stripes" | "mosaic" | "blur"

/** Sharp (right-angle) vs rounded corners on the rect's outline. */
export type StampCorner = "sharp" | "round"

/** Resting-state outline visibility; selecting a stamp always shows the outline. */
export type StampDisplay = "hover" | "always"

export interface StampRect {
  x: number
  y: number
  width: number
  height: number
  anchor: StampAnchor
  color: string
  fill: StampFill
  corner: StampCorner
  display: StampDisplay
  // Stacking order among overlapping rects on a page; higher paints on top. Defaults to 0.
  layer: number
}

// Last-picked rect style across dialog opens this session.
let lastPickedAnchor: StampAnchor = "tl"
let lastPickedFill: StampFill = "solid"
let lastPickedCorner: StampCorner = "sharp"
let lastPickedDisplay: StampDisplay = "hover"

/** Wire string → `StampRect`, or null; segments past `color` are optional for older stored rects. */
export function parseStampRect(rect: string): StampRect | null {
  if (!rect) return null
  const [xStr, yStr, wStr, hStr, anchorStr, color, fillStr, cornerStr, displayStr, layerStr] = rect.split(",")
  const x = Number(xStr)
  const y = Number(yStr)
  const width = Number(wStr)
  const height = Number(hStr)
  if ([x, y, width, height].some((n) => Number.isNaN(n))) return null
  const anchor = STAMP_ANCHORS.includes(anchorStr as StampAnchor) ? (anchorStr as StampAnchor) : "tl"
  const fill: StampFill =
    fillStr === "stripes" || fillStr === "mosaic" || fillStr === "blur" ? fillStr : "solid"
  const corner: StampCorner = cornerStr === "round" ? "round" : "sharp"
  const display: StampDisplay = displayStr === "always" ? "always" : "hover"
  const layer = Number(layerStr)
  return {
    x,
    y,
    width,
    height,
    anchor,
    color: color && /^#[0-9a-fA-F]{6}$/.test(color) ? color : "#ff0000",
    fill,
    corner,
    display,
    layer: Number.isFinite(layer) ? layer : 0,
  }
}

/** `StampRect` → wire string, the inverse of `parseStampRect`. */
export function formatStampRect(rect: StampRect): string {
  return `${rect.x.toFixed(2)},${rect.y.toFixed(2)},${rect.width.toFixed(2)},${rect.height.toFixed(2)},${rect.anchor},${rect.color},${rect.fill},${rect.corner},${rect.display},${rect.layer}`
}

/** Where an anchor code sits on the rect's border, as a rect-relative 0-100 percent pair. */
export function anchorPercent(anchor: StampAnchor): { x: number; y: number } {
  switch (anchor) {
    case "tl":
      return { x: 0, y: 0 }
    case "t":
      return { x: 50, y: 0 }
    case "tr":
      return { x: 100, y: 0 }
    case "r":
      return { x: 100, y: 50 }
    case "br":
      return { x: 100, y: 100 }
    case "b":
      return { x: 50, y: 100 }
    case "bl":
      return { x: 0, y: 100 }
    case "l":
      return { x: 0, y: 50 }
  }
}

export type StampEditorResult = { content: string; icon: string; rect: StampRect | null }

type DialogRequest =
  | {
      kind: "prompt"
      message: string
      defaultValue: string
      resolve: (value: string | null) => void
    }
  | {
      kind: "confirm"
      message: ReactNode
      danger: boolean
      // The confirm button stays disabled until typed text matches the localized "delete" word.
      requireTypedConfirmation: boolean
      resolve: (value: boolean) => void
    }
  | {
      kind: "newCategory"
      resolve: (value: NewCategoryResult | null) => void
    }
  | {
      kind: "renameArchive"
      currentStem: string
      extension: string
      resolve: (value: string | null) => void
    }
  | {
      kind: "stampEditor"
      defaultContent: string
      defaultIcon: string
      defaultRect: StampRect | null
      resolve: (value: StampEditorResult | null) => void
    }
  | {
      kind: "info"
      message: ReactNode
      resolve: () => void
    }

let currentRequest: DialogRequest | null = null
let listeners: (() => void)[] = []

function setRequest(request: DialogRequest | null) {
  currentRequest = request
  listeners.forEach((l) => l())
}

/** Drop-in replacement for `window.prompt` — resolves the entered string, or null if cancelled. */
export function promptDialog(message: string, defaultValue = ""): Promise<string | null> {
  return new Promise((resolve) => {
    setRequest({ kind: "prompt", message, defaultValue, resolve })
  })
}

/** Drop-in replacement for `window.confirm`. `danger` styles the button red for destructive
 * actions; `requireTypedConfirmation` additionally requires typing the localized "delete" word. */
export function confirmDialog(
  message: ReactNode,
  danger = false,
  requireTypedConfirmation = false,
): Promise<boolean> {
  return new Promise((resolve) => {
    setRequest({ kind: "confirm", message, danger, requireTypedConfirmation, resolve })
  })
}

/** Single-button acknowledgement dialog for content that can't be recovered if dismissed unread. */
export function infoDialog(message: ReactNode): Promise<void> {
  return new Promise((resolve) => {
    setRequest({ kind: "info", message, resolve: () => resolve() })
  })
}

/** Combined "New Category" dialog (name + static/dynamic tabs). Resolves null if cancelled. */
export function newCategoryDialog(): Promise<NewCategoryResult | null> {
  return new Promise((resolve) => {
    setRequest({ kind: "newCategory", resolve })
  })
}

/** Filename-rename dialog — the extension is a fixed non-editable suffix. Resolves the new stem
 * only, or null if cancelled. */
export function renameArchiveDialog(currentStem: string, extension: string): Promise<string | null> {
  return new Promise((resolve) => {
    setRequest({ kind: "renameArchive", currentStem, extension, resolve })
  })
}

/** Stamp name field + emoji/icon grid, plus anchor/color pickers for a rect placement. */
export function stampEditorDialog(
  defaultContent = "",
  defaultIcon = "",
  defaultRect: StampRect | null = null,
): Promise<StampEditorResult | null> {
  return new Promise((resolve) => {
    setRequest({ kind: "stampEditor", defaultContent, defaultIcon, defaultRect, resolve })
  })
}

/** A literal emoji as-is; a Font Awesome icon `fa:`-prefixed to tell the two apart. */
export function renderStampIcon(icon: string): React.ReactNode {
  if (!icon) return null
  if (icon.startsWith("fa:")) {
    const { cls, color } = parseFaIcon(icon)
    return <i className={`fas ${cls}`} style={color ? { color } : undefined} aria-hidden="true"></i>
  }
  return icon
}

/** `fa:<class>` or `fa:<class>:<#rrggbb>` — the color segment only applies to a FA icon. */
function parseFaIcon(icon: string): { cls: string; color: string | null } {
  const [, cls, color] = icon.split(":")
  return { cls: cls ?? "", color: color && /^#[0-9a-fA-F]{6}$/.test(color) ? color : null }
}

interface EmojiEntry {
  emoji: string
  name: string
  slug: string
}
interface EmojiGroup {
  name: string
  slug: string
  emojis: EmojiEntry[]
}
// Full CLDR emoji set (9 groups), loaded lazily in `useEmojiGroups` below.
const EMOJI_GROUP_TAB_ICON: Record<string, string> = {
  smileys_emotion: "😀",
  people_body: "👋",
  animals_nature: "🐻",
  food_drink: "🍔",
  travel_places: "🚗",
  activities: "⚽",
  objects: "💡",
  symbols: "❤️",
  flags: "🏳️",
}

// Excluded from the Flags category specifically (not a general moderation list).
const EXCLUDED_EMOJI_SLUGS = new Set(["flag_taiwan"])

// Last icon color picked this session; separate from the rect outline color below.
let lastPickedColor = "#000000"

/** Last-picked rect style, applied as the starting point for a newly-drawn rectangle. */
export function lastPickedRectStyle(): {
  anchor: StampAnchor
  color: string
  fill: StampFill
  corner: StampCorner
  display: StampDisplay
} {
  return {
    anchor: lastPickedAnchor,
    color: lastPickedRectColor,
    fill: lastPickedFill,
    corner: lastPickedCorner,
    display: lastPickedDisplay,
  }
}
let lastPickedRectColor = "#ff0000"

/** Max recently-submitted colors remembered per picker (3-column x 2-row swatch grid). */
const COLOR_HISTORY_LIMIT = 6

/** Separate history lists per picker — appended to only on submission, not on every onChange. */
let iconColorHistory: string[] = []
let rectColorHistory: string[] = []

/** Records `color` as the most-recent entry, capped at COLOR_HISTORY_LIMIT; returns a new array. */
function recordColorHistory(history: string[], color: string): string[] {
  return [color, ...history.filter((c) => c !== color)].slice(0, COLOR_HISTORY_LIMIT)
}

interface EmojiZhNames {
  groups: Record<string, string>
  emojis: Record<string, string>
}

// Swaps in generated Chinese names for `zh`; other languages keep the package's English names.
function useEmojiGroups(): EmojiGroup[] | null {
  const { i18n } = useTranslation()
  const language = i18n.resolvedLanguage ?? i18n.language
  const [groups, setGroups] = useState<EmojiGroup[] | null>(null)
  useEffect(() => {
    let cancelled = false
    void Promise.all([
      import("unicode-emoji-json/data-by-group.json"),
      language === "zh" ? import("./i18n/emoji-names-zh.json") : Promise.resolve(null),
    ]).then(([mod, zhMod]) => {
      if (cancelled) return
      // TS reads the JSON's sequential numeric keys as an array — exactly the shape we need.
      const raw = mod.default as unknown as EmojiGroup[]
      const zhNames = zhMod?.default as unknown as EmojiZhNames | undefined
      setGroups(
        raw.map((g) => ({
          ...g,
          name: zhNames?.groups[g.slug] ?? g.name,
          emojis: g.emojis
            .filter((e) => !EXCLUDED_EMOJI_SLUGS.has(e.slug))
            .map((e) => ({ ...e, name: zhNames?.emojis[e.emoji] ?? e.name })),
        })),
      )
    })
    return () => {
      cancelled = true
    }
  }, [language])
  return groups
}

const STAMP_FA_ICONS = [
  "fa-star", "fa-heart", "fa-fire", "fa-check", "fa-xmark", "fa-question", "fa-exclamation",
  "fa-thumbtack", "fa-bookmark", "fa-flag", "fa-eye", "fa-comment", "fa-bell", "fa-bolt",
  "fa-crown", "fa-gem", "fa-skull", "fa-paw", "fa-leaf", "fa-music", "fa-circle", "fa-triangle-exclamation",
]

// Declared at module scope — components re-declared in render get new identities and remount
// on every keystroke.
function OptionButton({
  selected,
  title,
  onClick,
  palette,
  children,
}: {
  selected: boolean
  title?: string
  onClick: () => void
  palette: ReturnType<typeof useMenuPalette>
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      title={title}
      style={{
        width: 32,
        height: 32,
        fontSize: 16,
        flexShrink: 0,
        boxSizing: "border-box",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        // 2px currentColor border — palette.hoverBg reads too close to the unselected background.
        border: `2px solid ${selected ? "currentColor" : "transparent"}`,
        borderRadius: 4,
        background: selected ? palette.hoverBg : "transparent",
        color: selected ? palette.hoverText : "inherit",
        cursor: "pointer",
      }}
    >
      {children}
    </button>
  )
}

function TabButton({
  selected,
  title,
  onClick,
  palette,
  children,
}: {
  selected: boolean
  title?: string
  onClick: () => void
  palette: ReturnType<typeof useMenuPalette>
  children: React.ReactNode
}) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={selected}
      title={title}
      onClick={onClick}
      style={{
        width: 24,
        height: 24,
        flexShrink: 0,
        fontSize: 14,
        lineHeight: 1,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        boxSizing: "border-box",
        padding: 0,
        margin: 0,
        border: `1px solid ${selected ? palette.border : "transparent"}`,
        borderRadius: 3,
        background: selected ? palette.hoverBg : "transparent",
        color: selected ? palette.hoverText : "inherit",
        cursor: "pointer",
      }}
    >
      {children}
    </button>
  )
}

function IconPicker({ icon, onChange }: { icon: string; onChange: (icon: string) => void }) {
  const { t } = useTranslation()
  const emojiGroups = useEmojiGroups()
  const [tab, setTab] = useState<string>("icons")
  const palette = useMenuPalette()
  const activeGroup = emojiGroups?.find((g) => g.slug === tab)

  return (
    <div style={{ textAlign: "left", marginBottom: 12 }}>
      {/* Icon-only tabs (title carries the name) — text labels can't fit; nowrap + overflow
          guarantees a single row. */}
      <div role="tablist" style={{ display: "flex", flexWrap: "nowrap", overflowX: "auto", justifyContent: "space-between", gap: 2, marginBottom: 6 }}>
        <TabButton selected={tab === "icons"} title={t("app.icons") ?? undefined} onClick={() => setTab("icons")} palette={palette}>
          <i className="fas fa-icons" aria-hidden="true"></i>
        </TabButton>
        <div aria-hidden="true" style={{ flexShrink: 0, width: 1, alignSelf: "stretch", background: palette.border, margin: "0 2px" }} />
        {emojiGroups?.map((g) => (
          <TabButton key={g.slug} selected={tab === g.slug} title={g.name} onClick={() => setTab(g.slug)} palette={palette}>
            {EMOJI_GROUP_TAB_ICON[g.slug] ?? g.emojis[0]?.emoji}
          </TabButton>
        ))}
      </div>
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: 4,
          height: 148,
          overflowY: "auto",
          padding: 4,
          border: `1px solid ${palette.border}`,
          borderRadius: 4,
        }}
      >
        {tab === "icons"
          ? STAMP_FA_ICONS.map((cls) => {
              const value = `fa:${cls}`
              return (
                <OptionButton
                  key={cls}
                  selected={icon === value}
                  title={cls}
                  palette={palette}
                  onClick={() => onChange(icon === value ? "" : value)}
                >
                  <i className={`fas ${cls}`} aria-hidden="true"></i>
                </OptionButton>
              )
            })
          : activeGroup
            ? activeGroup.emojis.map((e) => (
                <OptionButton
                  key={e.emoji}
                  selected={icon === e.emoji}
                  title={e.name}
                  palette={palette}
                  onClick={() => onChange(icon === e.emoji ? "" : e.emoji)}
                >
                  {e.emoji}
                </OptionButton>
              ))
            : (
                <span style={{ opacity: 0.6, padding: 4 }}>{t("common.loading") ?? undefined}</span>
              )}
      </div>
    </div>
  )
}

const FILL_STYLES: StampFill[] = ["solid", "stripes", "mosaic", "blur"]
const FILL_LABEL: Record<StampFill, string> = {
  solid: "Solid",
  stripes: "Stripes",
  mosaic: "Mosaic",
  blur: "Blur",
}

/** Preview swatch for one fill option — mosaic/blur show a frosted placeholder (nothing behind
 * a small swatch to blur). */
function FillSwatch({ fill, color }: { fill: StampFill; color: string }) {
  const SIZE = 18
  if (fill === "solid" || fill === "stripes") {
    return (
      <div
        style={{
          width: SIZE,
          height: SIZE,
          border: `1px solid ${color}`,
          background:
            fill === "solid"
              ? `${color}66`
              : `repeating-linear-gradient(45deg, ${color}66 0, ${color}66 2px, transparent 2px, transparent 5px)`,
        }}
      />
    )
  }
  return (
    <div
      style={{
        width: SIZE,
        height: SIZE,
        border: `1px solid ${color}`,
        background: "repeating-conic-gradient(#8888 0% 25%, #ccc8 0% 50%) 0 0 / 6px 6px",
        opacity: fill === "mosaic" ? 0.9 : 0.6,
      }}
    />
  )
}

const ANCHOR_LABEL: Record<StampAnchor, string> = {
  tl: "Top Left",
  t: "Top",
  tr: "Top Right",
  r: "Right",
  br: "Bottom Right",
  b: "Bottom",
  bl: "Bottom Left",
  l: "Left",
}

/** Native color input with a hover tooltip of recently-submitted colors (3x2 swatch grid). */
function ColorPickerWithHistory({
  value,
  onChange,
  history,
  disabled,
  title,
  id,
}: {
  value: string
  onChange: (color: string) => void
  history: string[]
  disabled?: boolean
  title?: string
  id?: string
}) {
  const input = (
    <input
      id={id}
      type="color"
      value={value}
      disabled={disabled}
      title={title}
      onChange={(e) => onChange(e.target.value)}
      style={{
        width: 25,
        height: 25,
        flexShrink: 0,
        padding: 0,
        border: "none",
        cursor: disabled ? "default" : "pointer",
        opacity: disabled ? 0.4 : 1,
      }}
    />
  )
  if (history.length === 0) return input
  return (
    <Tooltip
      maxWidth={100}
      label={
        <div style={{ display: "grid", gridTemplateColumns: "repeat(3, 1fr)", gap: 4, padding: 2 }}>
          {history.map((c) => (
            <button
              key={c}
              type="button"
              title={c}
              onClick={() => onChange(c)}
              style={{
                width: 22,
                height: 22,
                padding: 0,
                border: `1px solid ${c === value ? "white" : "rgba(255,255,255,0.4)"}`,
                borderRadius: 3,
                background: c,
                cursor: "pointer",
              }}
            />
          ))}
        </div>
      }
    >
      {input}
    </Tooltip>
  )
}

/** 8-dot grid picking which border point the icon sits on; dots use the rect's own outline color. */
function RectAnchorPicker({
  anchor,
  onChange,
  color,
}: {
  anchor: StampAnchor
  onChange: (anchor: StampAnchor) => void
  color: string
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const SIZE = 56
  const DOT = 14

  return (
    <div
      style={{
        position: "relative",
        width: SIZE,
        height: SIZE,
        flexShrink: 0,
        border: `1px dashed ${palette.border}`,
        borderRadius: 4,
      }}
    >
      {STAMP_ANCHORS.map((a) => {
        const pos = anchorPercent(a)
        const selected = a === anchor
        return (
          <button
            key={a}
            type="button"
            title={t(ANCHOR_LABEL[a]) ?? undefined}
            onClick={() => onChange(a)}
            style={{
              position: "absolute",
              left: `${pos.x}%`,
              top: `${pos.y}%`,
              // A selected dot grows + gets a colored ring — a thin theme-colored ring reads as
              // barely different from unselected at this scale.
              transform: `translate(-50%, -50%) scale(${selected ? 1.35 : 1})`,
              width: DOT,
              height: DOT,
              padding: 0,
              boxSizing: "border-box",
              borderRadius: "50%",
              border: `2px solid ${selected ? color : palette.border}`,
              background: selected ? `${color}33` : "transparent",
              cursor: "pointer",
              transition: "transform 0.1s ease",
            }}
          />
        )
      })}
    </div>
  )
}

function StampEditorForm({
  defaultContent,
  defaultIcon,
  defaultRect,
  onSubmit,
  onCancel,
}: {
  defaultContent: string
  defaultIcon: string
  defaultRect: StampRect | null
  onSubmit: (value: StampEditorResult) => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const [content, setContent] = useState(
    defaultContent || (defaultRect ? t("app.selection") : t("app.marker")) || "Marker",
  )
  // `icon` is always the base value; the color is recombined only for preview and submission.
  const initialParsed = useMemo(() => parseFaIcon(defaultIcon), [defaultIcon])
  const [icon, setIcon] = useState(defaultIcon.startsWith("fa:") ? `fa:${initialParsed.cls}` : defaultIcon)
  const [color, setColor] = useState(initialParsed.color ?? lastPickedColor)
  const [anchor, setAnchor] = useState<StampAnchor>(defaultRect?.anchor ?? lastPickedAnchor)
  const [rectColor, setRectColor] = useState(defaultRect?.color ?? lastPickedRectColor)
  const [fill, setFill] = useState<StampFill>(defaultRect?.fill ?? lastPickedFill)
  const [corner, setCorner] = useState<StampCorner>(defaultRect?.corner ?? lastPickedCorner)
  const [display, setDisplay] = useState<StampDisplay>(defaultRect?.display ?? lastPickedDisplay)
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    nameRef.current?.select()
  }, [])

  const isFaIcon = icon.startsWith("fa:")
  const combinedIcon = isFaIcon ? `${icon}:${color}` : icon

  function submit() {
    const trimmed = content.trim()
    if (!trimmed) return
    const rect = defaultRect ? { ...defaultRect, anchor, color: rectColor, fill, corner, display } : null
    if (isFaIcon) iconColorHistory = recordColorHistory(iconColorHistory, color)
    if (rect) rectColorHistory = recordColorHistory(rectColorHistory, rectColor)
    onSubmit({ content: trimmed, icon: combinedIcon, rect })
  }

  return (
    <div onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{t("app.enterStampName")}</p>
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 12 }}>
        <div
          style={{
            width: 25,
            height: 25,
            flexShrink: 0,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            fontSize: 16,
            border: "1px solid currentColor",
            borderRadius: 4,
            opacity: icon ? 1 : 0.5,
          }}
        >
          {icon ? renderStampIcon(combinedIcon) : <i className="fas fa-thumbtack" aria-hidden="true"></i>}
        </div>
        <input
          ref={nameRef}
          type="text"
          className="stdinput"
          style={{ flex: 1, height: 25, boxSizing: "border-box" }}
          value={content}
          placeholder={t("app.marker") ?? undefined}
          autoFocus
          onChange={(e) => setContent(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit()
          }}
        />
        <ColorPickerWithHistory
          value={color}
          disabled={!isFaIcon}
          title={t("app.iconColor") ?? undefined}
          history={iconColorHistory}
          onChange={(next) => {
            setColor(next)
            lastPickedColor = next
          }}
        />
      </div>
      <IconPicker icon={icon} onChange={setIcon} />
      {defaultRect && (
        <div style={{ textAlign: "left", marginBottom: 12, paddingTop: 12, borderTop: "1px solid currentColor" }}>
          <p style={{ fontWeight: "bold", margin: "0 0 10px" }}>{t("app.selectionRectangle")}</p>
          <div style={{ marginBottom: 12 }}>
            <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("app.iconPosition")}</span>
            <RectAnchorPicker
              anchor={anchor}
              onChange={(a) => {
                setAnchor(a)
                lastPickedAnchor = a
              }}
              color={rectColor}
            />
          </div>
          <div style={{ display: "flex", justifyContent: "space-between" }}>
            <div>
              <label htmlFor="stamp-rect-color" style={{ display: "block", fontSize: 13, marginBottom: 4 }}>
                {t("app.rectangleColor")}
              </label>
              <ColorPickerWithHistory
                id="stamp-rect-color"
                value={rectColor}
                history={rectColorHistory}
                onChange={(next) => {
                  setRectColor(next)
                  lastPickedRectColor = next
                }}
              />
            </div>
            <div>
              <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("app.fillStyle")}</span>
              <div role="tablist" style={{ display: "flex", gap: 2 }}>
                {FILL_STYLES.map((f) => (
                  <OptionButton
                    key={f}
                    selected={fill === f}
                    title={t(FILL_LABEL[f]) ?? undefined}
                    palette={palette}
                    onClick={() => {
                      setFill(f)
                      lastPickedFill = f
                    }}
                  >
                    <FillSwatch fill={f} color={rectColor} />
                  </OptionButton>
                ))}
              </div>
            </div>
            <div>
              <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("app.cornerStyle")}</span>
              <div role="tablist" style={{ display: "flex", gap: 2 }}>
                {(["sharp", "round"] as StampCorner[]).map((c) => (
                  <OptionButton
                    key={c}
                    selected={corner === c}
                    title={t(c === "sharp" ? "Sharp" : "Round") ?? undefined}
                    palette={palette}
                    onClick={() => {
                      setCorner(c)
                      lastPickedCorner = c
                    }}
                  >
                    <div
                      style={{
                        width: 18,
                        height: 18,
                        border: `1px solid ${rectColor}`,
                        borderRadius: c === "round" ? 6 : 0,
                      }}
                    />
                  </OptionButton>
                ))}
              </div>
            </div>
          </div>
          <div style={{ marginTop: 12 }}>
            <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("app.rectangleDisplay")}</span>
            <div role="tablist" style={{ display: "flex", gap: 2 }}>
              {(["hover", "always"] as StampDisplay[]).map((d) => (
                <OptionButton
                  key={d}
                  selected={display === d}
                  title={t(d === "hover" ? "On Hover" : "Always Visible") ?? undefined}
                  palette={palette}
                  onClick={() => {
                    setDisplay(d)
                    lastPickedDisplay = d
                  }}
                >
                  <i className={`fas ${d === "hover" ? "fa-hand-pointer" : "fa-eye"}`} aria-hidden="true"></i>
                </OptionButton>
              ))}
            </div>
          </div>
        </div>
      )}
      <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
        <input type="button" className="stdbtn" value={t("common.cancel") ?? "Cancel"} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t("common.ok") ?? "OK"} onClick={submit} />
      </div>
    </div>
  )
}

// Matches Library.tsx's autocomplete rule: fragment after the last separator, case-insensitive
// substring, weight-descending.
function TagSearchField({
  id,
  value,
  onChange,
  autoFocus,
  onEnter,
  placeholder,
}: {
  id: string
  value: string
  onChange: (next: string) => void
  autoFocus: boolean
  onEnter: () => void
  placeholder?: string
}) {
  const stats = useStats()
  const [open, setOpen] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)

  const currentFragment = value.match(/[^,\s-]*$/)?.[0] ?? ""
  const suggestions = useMemo(() => {
    if (!currentFragment) return []
    const needle = currentFragment.toLowerCase()
    return (stats.data ?? [])
      .map((s) => ({
        label: s.namespace ? `${s.namespace}:${s.text}` : s.text,
        // Quoted when the text has a space — a real AND-separator in the search grammar.
        insertValue: buildSearchToken(s.namespace ?? "", s.text),
      }))
      .filter((s) => s.label.toLowerCase().includes(needle))
      .slice(0, 15)
  }, [stats.data, currentFragment])

  return (
    <span style={{ position: "relative", display: "block" }}>
      <input
        ref={inputRef}
        id={id}
        type="text"
        className="stdinput"
        style={{ width: "100%", height: 25, boxSizing: "border-box" }}
        value={value}
        placeholder={placeholder}
        autoComplete="off"
        autoFocus={autoFocus}
        onChange={(e) => {
          onChange(e.target.value)
          setOpen(true)
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        onKeyDown={(e) => {
          if (e.key === "Enter") onEnter()
          if (e.key === "Escape") setOpen(false)
        }}
      />
      {open && suggestions.length > 0 && (
        <PopupMenu portal={false} style={{ position: "absolute", top: "100%", left: 0, zIndex: Z_OVERLAY_CONTENT, minWidth: "100%", maxHeight: 180, overflowY: "auto" }}>
          {suggestions.map((s) => (
            <PopupMenuItem
              key={s.label}
              onMouseDown={(e) => {
                e.preventDefault()
                onChange(`${value.replace(/[^,\s-]*$/, "")}${s.insertValue}`)
                setOpen(false)
                inputRef.current?.focus()
              }}
            >
              {s.label}
            </PopupMenuItem>
          ))}
        </PopupMenu>
      )}
    </span>
  )
}

function NewCategoryForm({ onSubmit, onCancel }: { onSubmit: (value: NewCategoryResult) => void; onCancel: () => void }) {
  const { t } = useTranslation()
  const [name, setName] = useState("")
  const [isDynamic, setIsDynamic] = useState(false)
  const [search, setSearch] = useState("")
  const [visibleToGuest, setVisibleToGuest] = useState(false)
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    nameRef.current?.select()
  }, [])

  function submit() {
    if (!name.trim()) return
    onSubmit({ name: name.trim(), isDynamic, search: isDynamic ? search : "", visibleToGuest })
  }

  return (
    <div onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{t("common.newCategory")}</p>
      <div role="tablist" style={{ display: "flex", gap: 4, marginBottom: 12 }}>
        <button
          type="button"
          role="tab"
          aria-selected={!isDynamic}
          className={`favtag-btn${!isDynamic ? " toggled" : ""}`}
          style={{ flex: 1 }}
          onClick={() => setIsDynamic(false)}
        >
          {t("app.staticCategory")}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={isDynamic}
          className={`favtag-btn${isDynamic ? " toggled" : ""}`}
          style={{ flex: 1 }}
          onClick={() => setIsDynamic(true)}
        >
          {t("app.dynamicCategory")}
        </button>
      </div>
      <div style={{ textAlign: "left", marginBottom: 12 }}>
        <label htmlFor="new-category-name" style={{ display: "block", fontSize: 13, marginBottom: 4 }}>
          {t("app.enterANameForThe")}
        </label>
        {/* height 25 matches the other controls here; legacy .stdinput defaults much shorter. */}
        <input
          ref={nameRef}
          id="new-category-name"
          type="text"
          className="stdinput"
          style={{ width: "100%", height: 25, boxSizing: "border-box" }}
          value={name}
          placeholder={t("app.myCategory") ?? undefined}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !isDynamic) submit()
          }}
        />
      </div>
      <div style={{ textAlign: "left", marginBottom: 12 }}>
        <Checkbox
          id="new-category-visible-to-guest"
          name="visible_to_guest"
          checked={visibleToGuest}
          onCheckedChange={setVisibleToGuest}
        />{" "}
        <label htmlFor="new-category-visible-to-guest">{t("categories.visibleToGuest")}</label>
      </div>
      {isDynamic && (
        <div style={{ textAlign: "left", marginBottom: 12 }}>
          <label htmlFor="new-category-search" style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 13, marginBottom: 4 }}>
            {t("app.searchPredicate")}
            <Tooltip label={<SearchSyntaxHelp />}>
              <i className="fas fa-question-circle" style={{ fontSize: 14, cursor: "help" }} aria-hidden="true"></i>
            </Tooltip>
          </label>
          <TagSearchField
            id="new-category-search"
            value={search}
            onChange={setSearch}
            autoFocus={false}
            onEnter={submit}
            placeholder="language:chinese"
          />
        </div>
      )}
      <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
        <input type="button" className="stdbtn" value={t("common.cancel") ?? "Cancel"} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t("common.ok") ?? "OK"} onClick={submit} />
      </div>
    </div>
  )
}

function RenameArchiveForm({
  currentStem,
  extension,
  onSubmit,
  onCancel,
}: {
  currentStem: string
  extension: string
  onSubmit: (stem: string) => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const [stem, setStem] = useState(currentStem)
  const stemRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    stemRef.current?.select()
  }, [])

  function submit() {
    if (!stem.trim()) return
    onSubmit(stem.trim())
  }

  return (
    <div onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{t("app.enterTheNewFileName")}</p>
      {/* Both halves are .stdinput so theming matches; the read-only suffix shrinks to its text. */}
      <div style={{ display: "flex", alignItems: "center", marginBottom: 12 }}>
        <input
          ref={stemRef}
          type="text"
          className="stdinput"
          style={{ flex: 1, height: 25, boxSizing: "border-box" }}
          value={stem}
          onChange={(e) => setStem(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit()
          }}
        />
        {extension && (
          <input
            type="text"
            className="stdinput"
            readOnly
            tabIndex={-1}
            aria-label={t("app.fileExtensionNotEditable") ?? undefined}
            size={extension.length + 1}
            style={{ flex: "0 0 auto", width: "auto", height: 25, boxSizing: "border-box", marginLeft: 4, textAlign: "center" }}
            value={`.${extension}`}
          />
        )}
      </div>
      <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
        <input type="button" className="stdbtn" value={t("common.cancel") ?? "Cancel"} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t("common.ok") ?? "OK"} onClick={submit} />
      </div>
    </div>
  )
}

/** Mounted once app-wide — renders whatever dialog the module-level API pushed. */
export function DialogHost() {
  const { t, i18n } = useTranslation()
  const [, forceUpdate] = useState(0)
  const [typedConfirmation, setTypedConfirmation] = useState("")
  const [promptValue, setPromptValue] = useState("")

  useEffect(() => {
    const listener = () => forceUpdate((n) => n + 1)
    listeners.push(listener)
    return () => {
      listeners = listeners.filter((l) => l !== listener)
    }
  }, [])

  const request = currentRequest

  // In-render reset on a new request (React's adjusting-state pattern), not an effect.
  const previousRequestRef = useRef<DialogRequest | null>(null)
  if (previousRequestRef.current !== request) {
    previousRequestRef.current = request
    if (typedConfirmation !== "") setTypedConfirmation("")
    if (request?.kind === "prompt") setPromptValue(request.defaultValue)
  }

  if (!request) return null

  const requiredConfirmationWord =
    request.kind === "confirm" && request.requireTypedConfirmation
      ? i18n.resolvedLanguage === "en"
        ? (t("common.delete") ?? "DELETE").toUpperCase()
        : (t("common.delete") ?? "DELETE")
      : null

  function close() {
    setRequest(null)
  }

  function submitPrompt() {
    if (request?.kind !== "prompt") return
    request.resolve(promptValue)
    close()
  }

  function cancelPrompt() {
    if (request?.kind !== "prompt") return
    request.resolve(null)
    close()
  }

  function confirmYes() {
    if (request?.kind !== "confirm") return
    request.resolve(true)
    close()
  }

  function confirmNo() {
    if (request?.kind !== "confirm") return
    request.resolve(false)
    close()
  }

  function cancelNewCategory() {
    if (request?.kind !== "newCategory") return
    request.resolve(null)
    close()
  }

  function submitNewCategory(value: NewCategoryResult) {
    if (request?.kind !== "newCategory") return
    request.resolve(value)
    close()
  }

  function cancelRenameArchive() {
    if (request?.kind !== "renameArchive") return
    request.resolve(null)
    close()
  }

  function submitRenameArchive(stem: string) {
    if (request?.kind !== "renameArchive") return
    request.resolve(stem)
    close()
  }

  function cancelStampEditor() {
    if (request?.kind !== "stampEditor") return
    request.resolve(null)
    close()
  }

  function submitStampEditor(value: StampEditorResult) {
    if (request?.kind !== "stampEditor") return
    request.resolve(value)
    close()
  }

  function acknowledgeInfo() {
    if (request?.kind !== "info") return
    request.resolve()
    close()
  }

  if (request.kind === "newCategory") {
    return createPortal(
      <>
        <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }} onClick={cancelNewCategory} />
        <div
          role="dialog"
          aria-modal="true"
          className="swal2-popup"
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            zIndex: Z_OVERLAY_CONTENT,
            display: "block",
            width: 360,
            padding: 20,
            textAlign: "center",
            borderRadius: ".2em",
            boxShadow: "0 2px 10px rgba(0,0,0,.4)",
          }}
        >
          <NewCategoryForm onSubmit={submitNewCategory} onCancel={cancelNewCategory} />
        </div>
      </>,
      document.body,
    )
  }

  if (request.kind === "renameArchive") {
    return createPortal(
      <>
        <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }} onClick={cancelRenameArchive} />
        <div
          role="dialog"
          aria-modal="true"
          className="swal2-popup"
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            zIndex: Z_OVERLAY_CONTENT,
            display: "block",
            width: 400,
            padding: 20,
            textAlign: "center",
            borderRadius: ".2em",
            boxShadow: "0 2px 10px rgba(0,0,0,.4)",
          }}
        >
          <RenameArchiveForm
            currentStem={request.currentStem}
            extension={request.extension}
            onSubmit={submitRenameArchive}
            onCancel={cancelRenameArchive}
          />
        </div>
      </>,
      document.body,
    )
  }

  if (request.kind === "info") {
    return createPortal(
      <>
        <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }} onClick={acknowledgeInfo} />
        <div
          role="dialog"
          aria-modal="true"
          className="swal2-popup"
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            zIndex: Z_OVERLAY_CONTENT,
            display: "block",
            width: 420,
            padding: 20,
            textAlign: "center",
            borderRadius: ".2em",
            boxShadow: "0 2px 10px rgba(0,0,0,.4)",
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape" || e.key === "Enter") acknowledgeInfo()
          }}
        >
          <div style={{ margin: "0 0 12px" }}>{request.message}</div>
          <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
            <input type="button" className="stdbtn" value={t("common.ok") ?? "OK"} onClick={acknowledgeInfo} autoFocus />
          </div>
        </div>
      </>,
      document.body,
    )
  }

  if (request.kind === "stampEditor") {
    return createPortal(
      <>
        <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }} onClick={cancelStampEditor} />
        <div
          role="dialog"
          aria-modal="true"
          className="swal2-popup"
          style={{
            position: "fixed",
            top: "50%",
            left: "50%",
            transform: "translate(-50%, -50%)",
            zIndex: Z_OVERLAY_CONTENT,
            display: "block",
            width: 320,
            padding: 20,
            textAlign: "center",
            borderRadius: ".2em",
            boxShadow: "0 2px 10px rgba(0,0,0,.4)",
          }}
        >
          <StampEditorForm
            defaultContent={request.defaultContent}
            defaultIcon={request.defaultIcon}
            defaultRect={request.defaultRect}
            onSubmit={submitStampEditor}
            onCancel={cancelStampEditor}
          />
        </div>
      </>,
      document.body,
    )
  }

  const onCancel = request.kind === "prompt" ? cancelPrompt : confirmNo
  const onConfirm = request.kind === "prompt" ? submitPrompt : confirmYes
  const confirmationBlocked = requiredConfirmationWord !== null && typedConfirmation !== requiredConfirmationWord

  return createPortal(
    <>
      <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }} onClick={onCancel} />
      <div
        role="dialog"
        aria-modal="true"
        className="swal2-popup"
        style={{
          position: "fixed",
          top: "50%",
          left: "50%",
          transform: "translate(-50%, -50%)",
          zIndex: Z_OVERLAY_CONTENT,
          display: "block",
          width: 360,
          padding: 20,
          textAlign: "center",
          borderRadius: ".2em",
          boxShadow: "0 2px 10px rgba(0,0,0,.4)",
          animation: "confirm-pop-in 0.3s cubic-bezier(0.2, 0.9, 0.3, 1.2)",
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel()
          if (e.key === "Enter" && request.kind === "confirm" && !confirmationBlocked) onConfirm()
        }}
      >
        {request.kind === "confirm" && (
          <i className="fa fa-exclamation-triangle fa-2x" style={{ color: "#d33" }} aria-hidden="true"></i>
        )}
        <div style={{ fontWeight: "bold", margin: request.kind === "confirm" ? "12px 0" : "0 0 12px" }}>{request.message}</div>
        {request.kind === "prompt" && (
          <Input
            rows={1}
            value={promptValue}
            onValueChange={(value) => setPromptValue(value)}
            placeholder={request.defaultValue || undefined}
            autoFocus
            style={{ width: "100%", boxSizing: "border-box", marginBottom: 12, textAlign: "left" }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault()
                onConfirm()
              }
            }}
          />
        )}
        {requiredConfirmationWord !== null && (
          <div style={{ textAlign: "left", marginBottom: 12 }}>
            <label htmlFor="dialog-typed-confirmation" style={{ display: "block", fontSize: 13, marginBottom: 4 }}>
              {t("common.typeToConfirm", { word: requiredConfirmationWord }) ?? undefined}
            </label>
            <input
              id="dialog-typed-confirmation"
              type="text"
              className="stdinput"
              style={{ width: "100%", height: 25, boxSizing: "border-box" }}
              value={typedConfirmation}
              autoFocus
              autoComplete="off"
              onChange={(e) => setTypedConfirmation(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !confirmationBlocked) onConfirm()
              }}
            />
          </div>
        )}
        <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
          <input type="button" className="stdbtn" value={t("common.cancel") ?? "Cancel"} onClick={onCancel} />
          <input
            type="button"
            disabled={confirmationBlocked}
            className={request.kind === "confirm" && request.danger ? "stdbtn stdbtn-danger" : "stdbtn"}
            style={confirmationBlocked ? { opacity: 0.5, cursor: "not-allowed" } : undefined}
            value={t("common.ok") ?? "OK"}
            onClick={onConfirm}
          />
        </div>
      </div>
    </>,
    document.body,
  )
}
