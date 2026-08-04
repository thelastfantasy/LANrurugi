import { useEffect, useMemo, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useTranslation } from "react-i18next"

import { useStats } from "./api/hooks"
import { PopupMenu, PopupMenuItem, useMenuPalette } from "./components/PopupMenu"
import { Tooltip } from "./components/Tooltip"
import { buildSearchToken } from "./lib/tagFormat"
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "./theme"

// Real, themed replacements for `window.prompt`/`window.confirm` — same call shape as those
// (`await promptDialog(message, defaultValue)` / `await confirmDialog(message)`, both resolving
// to what the plain browser function would have returned: the entered string or `null` if
// cancelled/empty for a prompt, a boolean for a confirm) so existing call sites need only add
// `await` and drop the `window.` prefix, not a structural rewrite. A *native* `window.prompt`/
// `window.confirm` is an unstyled OS dialog outside the page's own DOM/CSS entirely — it happens
// to pick up a vaguely similar red/cream palette on some Linux desktop themes purely by
// coincidence of that OS theme, never because of anything this app's own CSS controls. Legacy
// itself never used the native versions either — its real popups are SweetAlert2 (`LRR.showPopUp`/
// `Swal.fire`), which is exactly what `.swal2-popup`/`.swal2-actions>.stdbtn` (real classes already
// vendored per-theme, e.g. `~/LANraragi/public/themes/g.css:643,220`) exist to style — this module
// reuses those same classes rather than either the native dialogs or a new SweetAlert2 dependency,
// matching `Library.tsx`'s own pre-existing `DeleteConfirmDialog` in spirit (a from-scratch themed
// popup) but as a shared, promise-based module usable from any file instead of one bespoke
// component wired into one page's own local state.
//
// Architecture mirrors `toast.tsx` exactly: a module-level "current request" state that any file
// can push into via a plain function call, rendered by one `<DialogHost />` mounted once in
// `App.tsx` (matching `toast.tsx`'s own `<ToastContainer>` convention) — not a React hook, since
// call sites need this to work from plain event handlers exactly like `window.prompt` did, not
// only from inside a component's own render.

export type NewCategoryResult = { name: string; isDynamic: boolean; search: string }

/** One of the 8 points along a stamp's own selection-rectangle border (4 corners + 4 edge
 * midpoints) the icon can be anchored to — matches `Stamp::rect`'s own `anchor` segment on the
 * backend exactly (short codes, not full names, to keep the stored string compact). */
export type StampAnchor = "tl" | "t" | "tr" | "r" | "br" | "b" | "bl" | "l"

const STAMP_ANCHORS: StampAnchor[] = ["tl", "t", "tr", "r", "br", "b", "bl", "l"]

/** A stamp's optional selection rectangle — percent `x`/`y`/`width`/`height` of the page image
 * (matching `position`'s own percent convention), which of the 8 border points the icon sits on,
 * and the rect outline's own color. `null` (not e.g. an all-zero rect) means "this stamp has no
 * rectangle, just its plain point," the same "empty string is the absence, not a degenerate
 * value" convention `icon`/`rect` already use on the wire. */
/** How the rect's own interior is rendered — `solid`/`stripes` are plain translucent overlays
 * (`rect.color` layered on top of whatever's underneath); `mosaic`/`blur` instead obscure the
 * underlying page content itself via `backdrop-filter: blur()` at two different intensities (see
 * `MarkerLayer.tsx`'s own render for the actual CSS — there's no real CSS pixelation filter for
 * arbitrary backdrop content, so `mosaic` is a strong-blur approximation of an obscuring effect
 * rather than true square-block pixelation). A per-stamp choice, not a global reader setting,
 * same as `anchor`/`color`. */
export type StampFill = "solid" | "stripes" | "mosaic" | "blur"

/** Sharp (right-angle) vs rounded corners on the rect's own outline. */
export type StampCorner = "sharp" | "round"

/** Whether the rect's own outline/fill is visible only while hovering the stamp's icon (`hover`,
 * the original/default behavior — a rect that's otherwise invisible clutter until you're actually
 * interested in it), or always painted on the page regardless of hover state (`always` — useful
 * for a rect meant to be a permanent, visible annotation rather than a hidden-until-inspected
 * one). Selecting a stamp (single-click, entering resize/adjust mode) always shows the outline
 * either way — this only governs the plain, unselected resting state. */
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
  // Stacking order among overlapping rect stamps on the *same page* — higher paints on top, no
  // fixed range (a "bring to front"/"send to back" keyboard shortcut just picks one past whatever
  // the current extremes on that page already are; see `MarkerLayer.tsx`'s own
  // `bringToFront`/`sendToBack`). Defaults to `0` for every stamp created before this field
  // existed, the same "zero value round-trips as the have-never-been-touched default" convention
  // every other `StampRect` field already uses.
  layer: number
}

// Remembers the last-picked anchor/fill/corner/display style across dialog opens within the same
// session — same "shouldn't have to re-pick a preference for every new stamp" reasoning as
// `lastPickedColor` (icon color) above, requested explicitly ("能够在下次绘制时就直接应用").
// `lastPickedAnchor` was originally left out of this group on the theory that "where on *this*
// rect the icon sits" has no sensible cross-stamp meaning — reported live as a real gap once it
// turned out users do expect it to carry over exactly like the other three, so it's included here
// too now.
let lastPickedAnchor: StampAnchor = "tl"
let lastPickedFill: StampFill = "solid"
let lastPickedCorner: StampCorner = "sharp"
let lastPickedDisplay: StampDisplay = "hover"

/** `"x,y,width,height,anchor,color,fill,corner,display,layer"` → `StampRect`, or `null` for an
 * empty/malformed string — matches `Stamp::rect`'s own wire format exactly (see that field's docs
 * on the backend, which stores this as an opaque string with no format validation of its own —
 * the frontend owns the whole schema). Every segment past `color` is optional on read (each falls
 * back to a sensible default if missing/unrecognized) so a rect stored before that segment existed
 * still parses successfully — the same forward-compatible convention `anchor`/`color` themselves
 * already established. */
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

/** `StampRect` → `"x,y,width,height,anchor,color,fill,corner,display,layer"`, the inverse of
 * `parseStampRect`. */
export function formatStampRect(rect: StampRect): string {
  return `${rect.x.toFixed(2)},${rect.y.toFixed(2)},${rect.width.toFixed(2)},${rect.height.toFixed(2)},${rect.anchor},${rect.color},${rect.fill},${rect.corner},${rect.display},${rect.layer}`
}

/** Where a given anchor code sits on a rect's own border, as a 0-100 percent pair *relative to
 * the rect's own box* (not the page image) — `0,0` is the rect's own top-left corner, `100,100`
 * its bottom-right. Shared by the marker's own icon placement (`MarkerLayer.tsx`) and the picker
 * UI below, so the two can't drift on what e.g. `'t'` actually means. */
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
      message: string
      resolve: (value: boolean) => void
    }
  | {
      kind: "newCategory"
      resolve: (value: NewCategoryResult | null) => void
    }
  | {
      kind: "stampEditor"
      defaultContent: string
      defaultIcon: string
      defaultRect: StampRect | null
      resolve: (value: StampEditorResult | null) => void
    }

let currentRequest: DialogRequest | null = null
let listeners: (() => void)[] = []

function setRequest(request: DialogRequest | null) {
  currentRequest = request
  listeners.forEach((l) => l())
}

/** Drop-in replacement for `window.prompt(message, defaultValue)` — resolves to the entered
 * string, or `null` if cancelled (matches the native function's own return shape exactly, so a
 * call site's existing `if (title && title.trim() !== '')` guard keeps working unchanged). */
export function promptDialog(message: string, defaultValue = ""): Promise<string | null> {
  return new Promise((resolve) => {
    setRequest({ kind: "prompt", message, defaultValue, resolve })
  })
}

/** Drop-in replacement for `window.confirm(message)`. */
export function confirmDialog(message: string): Promise<boolean> {
  return new Promise((resolve) => {
    setRequest({ kind: "confirm", message, resolve })
  })
}

/** One combined "New Category" dialog (name + a static/dynamic tab switcher, with a predicate
 * field that only appears in dynamic mode) shared by every quick-create entry point (the Reader
 * overview overlay, the Upload page, and — in spirit — `Categories.tsx`'s own longer-standing
 * pair of "New Static/New Dynamic" buttons), rather than two separate single-purpose buttons each
 * call site would otherwise have to lay out and wire up itself. Resolves `null` if cancelled. */
export function newCategoryDialog(): Promise<NewCategoryResult | null> {
  return new Promise((resolve) => {
    setRequest({ kind: "newCategory", resolve })
  })
}

/** Replaces the old plain `promptDialog(t('Enter Stamp name:'))` call `MarkerLayer.tsx` used for
 * both placing a new stamp and renaming an existing one — same name field, plus a Windows
 * (Win+.)-style emoji/icon grid so a stamp can carry a custom marker instead of always the
 * default pin, and (when `defaultRect` is non-`null` — a rectangle-drag placement, not a plain
 * click) an anchor-position + outline-color picker for that rectangle. Resolves `null` if
 * cancelled. */
export function stampEditorDialog(
  defaultContent = "",
  defaultIcon = "",
  defaultRect: StampRect | null = null,
): Promise<StampEditorResult | null> {
  return new Promise((resolve) => {
    setRequest({ kind: "stampEditor", defaultContent, defaultIcon, defaultRect, resolve })
  })
}

/** A literal emoji character stored as-is; a Font Awesome icon stored `fa:`-prefixed to tell the
 * two apart at render time (an emoji could otherwise collide with a real fa- class name only by
 * extreme coincidence, but the prefix makes it unambiguous rather than relying on that). Shared
 * between the picker here and `MarkerLayer.tsx`'s own marker rendering, so the two can't drift. */
export function renderStampIcon(icon: string): React.ReactNode {
  if (!icon) return null
  if (icon.startsWith("fa:")) {
    const { cls, color } = parseFaIcon(icon)
    return <i className={`fas ${cls}`} style={color ? { color } : undefined} aria-hidden="true"></i>
  }
  return icon
}

/** `fa:<class>` or `fa:<class>:<#rrggbb>` — the color segment only ever applies to a Font Awesome
 * icon (an emoji already carries its own fixed color via the system emoji font; a "color" input
 * next to one wouldn't do anything, so the picker only ever shows it for this tab). Splitting this
 * out of the plain 3-way `.split(':')` inline at both call sites (here and the editor form below)
 * would've meant re-deriving the same "is the 3rd segment actually a valid hex color" check twice,
 * with two chances to drift out of sync. */
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
// `unicode-emoji-json`'s own group order/keys, from unicode.org's real CLDR emoji category data —
// the complete set (all 9 categories, ~1900 emoji total), not a hand-picked subset: a stamp marker
// with only 40-odd curated options isn't actually "an emoji picker," it's a slightly-nicer preset
// list, and the whole point of matching the Windows (Win+.) picker's own look was the real thing.
// One representative glyph stands in for each category's own tab (matching that same picker's
// icon-only category rail) rather than a text label, which wouldn't fit nine of them across a
// 320px-wide dialog. Loaded lazily (see `useEmojiGroups` below) — ~400KB of JSON nobody needs
// until they actually open this picker shouldn't ride along in the app's main bundle.
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

// Excluded from the Flags category specifically (not a general moderation list — everything else
// in the full unicode.org set stays as-is).
const EXCLUDED_EMOJI_SLUGS = new Set(["flag_taiwan"])

// Remembers the last color picked for a Font Awesome icon across dialog opens within the same
// session (module-level, not persisted to storage) — picking a color once per stamp would get old
// fast if every new stamp reset back to black regardless of what was just chosen. Deliberately
// separate from `lastPickedRectColor` below (`dialog.tsx`'s own `StampFill`/`StampCorner` section)
// — an icon's color and a selection rectangle's outline color are conceptually different choices;
// picking one shouldn't silently change the other's own remembered default.
let lastPickedColor = "#000000"

/** Last-picked rect anchor/outline color/fill/corner/display style, applied as the starting point
 * for a *newly drawn* rectangle (`MarkerLayer.tsx`'s `openEditorAndCreate`) — same "shouldn't have
 * to re-pick a preference for every new stamp" reasoning as `lastPickedColor` above, requested
 * explicitly ("能够在下次绘制时就直接应用"). Exported (unlike `lastPickedColor`, read only from
 * `StampEditorForm` in this same file) since `MarkerLayer.tsx` needs a real, already-known
 * `StampRect` value to hand to `stampEditorDialog` as `defaultRect` the moment a drag finishes —
 * there's no earlier point in that flow where `StampEditorForm` itself could inject a fallback
 * the way it does for `color` (whose fallback only ever needs to apply to a Font Awesome icon
 * that was already picked in the very same render). */
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

/** Maximum number of recently-*submitted* colors remembered per picker (a 3-column x 2-row
 * swatch grid, requested explicitly at that exact shape) — old entries fall off the end as new
 * ones are recorded, most-recent first. */
const COLOR_HISTORY_LIMIT = 6

/** Separate history lists for the icon-color and rect-color pickers, same reasoning as
 * `lastPickedColor`/`lastPickedRectColor` being two different variables rather than one shared
 * one — the two colors are conceptually unrelated choices, so their own "recently used" lists
 * shouldn't bleed into each other either. Only ever appended to on a real dialog *submission*
 * (`recordColorHistory`, called from `StampEditorForm.submit()`), not on every `onChange` — a
 * color someone was just previewing by dragging the native picker's own hue slider around
 * without ever confirming it isn't "recently used" in any meaningful sense. */
let iconColorHistory: string[] = []
let rectColorHistory: string[] = []

/** Records `color` as the most-recent entry in `history` (an existing occurrence is moved to the
 * front rather than duplicated), capped at `COLOR_HISTORY_LIMIT`. Returns a new array — callers
 * reassign their own module-level `let` binding with the result, since a plain array push
 * wouldn't trigger anything to re-read it (these aren't React state; the tooltip that reads them
 * only ever does so at render time, when the dialog reopens). */
function recordColorHistory(history: string[], color: string): string[] {
  return [color, ...history.filter((c) => c !== color)].slice(0, COLOR_HISTORY_LIMIT)
}

interface EmojiZhNames {
  groups: Record<string, string>
  emojis: Record<string, string>
}

// unicode-emoji-json's own `name`/group `name` fields are English-only (CLDR's own upstream
// short-name annotations, not translated per-locale by that package) — for the one language this
// app has real localized names generated for (`scripts/generate-emoji-zh-names.mjs`, sourced from
// Unicode's own CLDR zh annotations, not machine translation), swap them in at load time rather
// than leaving all ~1900 emoji tooltips + all 9 category tab titles in English even when the rest
// of the UI is in Chinese. Loaded lazily and only for `zh` — the other 13 supported languages have
// no equivalent generated data yet, so they keep falling back to the English names exactly as
// before this file existed.
function useEmojiGroups(): EmojiGroup[] | null {
  const { i18n } = useTranslation()
  const language = i18n.resolvedLanguage ?? i18n.language
  const [groups, setGroups] = useState<EmojiGroup[] | null>(null)
  useEffect(() => {
    let cancelled = false
    // Deliberately doesn't reset `groups` to `null` before the new language's data resolves
    // (which would need a synchronous `setState` call in the effect body, flagged by
    // `react-hooks/set-state-in-effect` as a cascading-render footgun) — the brief window where a
    // language switch still shows the *previous* language's names until the new import resolves
    // is a cosmetic non-issue (this data is only ever visible inside an already-open picker, and
    // both JSON chunks are already warmed in the browser cache after the first load), not worth
    // fighting the lint rule for.
    void Promise.all([
      import("unicode-emoji-json/data-by-group.json"),
      language === "zh" ? import("./i18n/emoji-names-zh.json") : Promise.resolve(null),
    ]).then(([mod, zhMod]) => {
      if (cancelled) return
      // TypeScript's own JSON-module inference reads this file's `{"0": {...}, "1": {...}, ...}`
      // shape as an array (sequential 0-based numeric keys), which is also exactly the runtime
      // shape `Object.values` would have produced anyway — no conversion actually needed.
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

// This app already vendors the full Font Awesome font (every `fa-*` class throughout the UI) —
// reusing it here for the "icon library" half of the picker costs nothing extra to load, unlike
// adding a whole new icon-set dependency for a curated subset.
const STAMP_FA_ICONS = [
  "fa-star", "fa-heart", "fa-fire", "fa-check", "fa-xmark", "fa-question", "fa-exclamation",
  "fa-thumbtack", "fa-bookmark", "fa-flag", "fa-eye", "fa-comment", "fa-bell", "fa-bolt",
  "fa-crown", "fa-gem", "fa-skull", "fa-paw", "fa-leaf", "fa-music", "fa-circle", "fa-triangle-exclamation",
]

// Both button components below are declared at module scope, not inside `IconPicker`'s own body
// (an earlier version defined them there, closing over `icon`/`onChange`/`palette` directly) —
// `react-hooks/static-components` flags that as a real bug, not just style: a component
// re-declared on every render gets a *new* identity each time, which React treats as "a
// completely different component type" at that position in the tree, discarding and remounting
// it instead of reconciling — for `TabButton`, used ten times in a `.map()`, that would mean every
// single tab remounts from scratch on every keystroke in the name field above it.
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
        // A 2px `currentColor` border (this dialog's own themed text color) rather than
        // `palette.hoverBg`, whose actual color in this theme reads as too close to the
        // unselected/hover background to tell apart at a glance — reported live for the Fill
        // Style/Corner Style swatches specifically ("选中后看不出明显区别" — no visible
        // difference after selecting), the same underlying contrast problem `RectAnchorPicker`'s
        // own selection dots had. Kept as a border only (not a filled background, unlike that
        // fix) since this button's children are already visually rich content of their own (an
        // icon glyph, an emoji, or — for Fill/Corner — a small colored swatch) that a solid
        // background fill would compete with or obscure.
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
  // Defaults to the first real emoji group once loaded, not a hardcoded slug — avoids a render
  // where the tab rail shows 9 category icons but none of them are actually selected yet.
  const [tab, setTab] = useState<string>("icons")
  const palette = useMenuPalette()
  const activeGroup = emojiGroups?.find((g) => g.slug === tab)

  return (
    <div style={{ textAlign: "left", marginBottom: 12 }}>
      {/* Icon-only tabs (one representative glyph per category, `title` carrying the real name for
          a hover tooltip) — nine emoji-group names plus "Icons" as full text labels wouldn't fit
          across this dialog's own width at all, matching why the real Windows picker this is
          modeled on uses the same icon-only category rail rather than text tabs. Deliberately not
          `.favtag-btn` here (unlike the fit-mode/tab buttons elsewhere in this file) — its real
          theme CSS (`min-width: 50px`, a 2px border, `padding: 0 4px`, all on top of `content-box`
          sizing that a plain `width` style can't override) is built for text-labeled pill buttons,
          not compact icon squares; stacked onto ten of them it rendered far larger than intended
          and wrapped onto a second row instead of fitting across in one. `flexWrap: 'nowrap'` +
          `overflowX: 'auto'` guarantees a single row (with a scrollbar as the fallback, not a
          silent second row) rather than just hoping ten 24px cells + gaps happen to fit exactly.*/}
      <div role="tablist" style={{ display: "flex", flexWrap: "nowrap", overflowX: "auto", justifyContent: "space-between", gap: 2, marginBottom: 6 }}>
        <TabButton selected={tab === "icons"} title={t("Icons") ?? undefined} onClick={() => setTab("icons")} palette={palette}>
          <i className="fas fa-icons" aria-hidden="true"></i>
        </TabButton>
        {/* Separates the Font Awesome "Icons" tab from the nine emoji-category tabs that follow —
            those two groups aren't really the same kind of thing (a curated icon library vs. the
            full Unicode emoji set), so a plain unbroken row read as one undifferentiated list. */}
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
                // Still loading (`useEmojiGroups` hasn't resolved its dynamic import yet) — this
                // only shows for the first tab switch away from "Icons"; the ~400KB JSON chunk is
                // small enough that a real user won't see this for more than a frame or two on
                // any connection, but showing *nothing* at all during that gap would read as the
                // picker being broken rather than just loading.
                <span style={{ opacity: 0.6, padding: 4 }}>{t("Loading…") ?? undefined}</span>
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

/** A small preview square for one `StampFill` option, in the currently-picked `color` — `solid`/
 * `stripes` reuse the exact same CSS this fill produces on the real rect (a flat translucent tint,
 * or the `repeating-linear-gradient` stripe pattern); `mosaic`/`blur` can't show their own real
 * effect at this scale (there's no actual page content behind a 18x18px swatch for
 * `backdrop-filter` to blur), so they instead show a plain frosted-glass-style translucent white
 * layer — enough to communicate "this obscures what's underneath" as a *concept*, distinctly from
 * the tinted-but-transparent solid/stripes options, without pretending to preview the real effect. */
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

/** A native `<input type="color">` wrapped in a `Tooltip` showing up to `COLOR_HISTORY_LIMIT`
 * recently-*submitted* colors (3 columns x 2 rows, requested at that exact shape) as clickable
 * swatches — hovering the swatch next to the native picker reveals the grid; clicking a swatch
 * applies that color immediately without needing to reopen the native picker and re-navigate to
 * a shade already used before. Tooltip content is `null` (renders nothing extra) when `history`
 * is empty, e.g. the very first time either picker is ever used in a session. */
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

/** An 8-dot grid tracing a rectangle's own border (4 corners + 4 edge midpoints), one dot per
 * `StampAnchor` — picks which point on the stamp's own selection rectangle the icon sits on.
 * Deliberately a small square outline rather than e.g. a dropdown of 8 text labels: the spatial
 * layout *is* the explanation, matching where the icon will actually end up visually far better
 * than "Top Right" as plain text would on its own. `color` is the rect's own currently-picked
 * outline color (not `currentColor`/the dialog's own text color) — a solid black-filled dot read
 * as harsh/out of place next to the dialog's cream-and-red palette when it didn't match anything
 * else on screen; using the same color the rect itself will actually be drawn in ties the picker
 * visually back to the thing it's configuring. */
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
              // A selected dot grows and gets a colored ring (a soft tinted fill + a solid-color
              // border, in the rect's own outline color) rather than only swapping a 1px
              // border/background pair — at this dot's small scale, a thin ring in the dialog's
              // own low-contrast theme colors read as barely different from the unselected state
              // at a glance (reported live twice: once for being too subtle, once for the fix —
              // a flat solid-black fill — reading as visually harsh/out of place). A ring rather
              // than a flat fill keeps the dot looking like a "marker," not a plain black disc.
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
  // A brand-new stamp (no name typed yet anywhere) starts pre-filled with a real default value
  // rather than an empty field backed only by a placeholder — a placeholder disappears the moment
  // typing starts and submits as "" if left untouched, whereas a real value can just be accepted
  // as-is with one Enter/click. A freshly *dragged* rectangle defaults to "Selection" rather than
  // "Marker" — the two placement modes create meaningfully different things (a point pin vs. a
  // region), and the default name should say which one this actually is rather than reusing the
  // point-stamp wording for both.
  const [content, setContent] = useState(
    defaultContent || (defaultRect ? t("Selection") : t("Marker")) || "Marker",
  )
  // `icon` here is always the *base* value (`fa:<class>` or a plain emoji, never with a color
  // segment) — the one `IconPicker`'s own grid buttons compare their own (also always base) values
  // against for "is this cell the selected one." A color, when the current icon is a Font Awesome
  // one, is tracked separately and only ever recombined into the full `fa:<class>:<#rrggbb>` form
  // at the two points that actually need it: the live preview, and submission.
  const initialParsed = useMemo(() => parseFaIcon(defaultIcon), [defaultIcon])
  const [icon, setIcon] = useState(defaultIcon.startsWith("fa:") ? `fa:${initialParsed.cls}` : defaultIcon)
  // Falls back to whatever color was last picked in a previous stamp this session, not always
  // black — picking a color once shouldn't mean re-picking it for every stamp after.
  const [color, setColor] = useState(initialParsed.color ?? lastPickedColor)
  // `anchor`/`rectColor`/`fill`/`corner` all carry forward from the last-confirmed rect —
  // `MarkerLayer.tsx` already seeds `defaultRect` with `lastPickedRectStyle()` for a freshly drawn
  // rectangle, so `defaultRect`'s own values here are already correct in both cases (a genuinely-
  // remembered default for a new rect, this stamp's own real stored values when editing an
  // existing one); the `?? lastPickedAnchor`/`?? lastPickedRectColor` fallbacks below only matter
  // if `defaultRect` were ever missing a field for some other reason. (`anchor` was originally left
  // out of this group — see `lastPickedAnchor`'s own docs for why that turned out to be wrong.)
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
    // Recorded here — on actual submission — rather than on every `onChange` of either color
    // input: a color someone previewed by dragging the native picker's own hue slider around
    // without ever confirming isn't meaningfully "recently used," and would otherwise crowd out
    // real choices in a 6-slot history almost immediately.
    if (isFaIcon) iconColorHistory = recordColorHistory(iconColorHistory, color)
    if (rect) rectColorHistory = recordColorHistory(rectColorHistory, rectColor)
    onSubmit({ content: trimmed, icon: combinedIcon, rect })
  }

  return (
    <div onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{t("Enter Stamp name:")}</p>
      <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 12 }}>
        {/* A live preview of the currently-picked icon (or the default marker's own bare pin, once
            none is chosen) right next to the name field — the picker below is a whole grid to
            scroll through, so seeing what's actually selected without hunting for its highlighted
            cell is worth the one extra element. */}
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
          placeholder={t("Marker") ?? undefined}
          autoFocus
          onChange={(e) => setContent(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") submit()
          }}
        />
        {/* Only meaningful for a Font Awesome icon — an emoji already renders in its own fixed,
            full colors via the system emoji font, so picking a color wouldn't visibly do anything
            for one. Disabled rather than unmounted when an emoji is selected: removing the element
            entirely shifts the name input's own width every time the icon type is toggled, which
            reads as the layout jittering. The native picker (not a custom-built one) — this is
            exactly what `<input type="color">` exists for, and every browser already ships a
            themed, familiar one of its own. */}
        <ColorPickerWithHistory
          value={color}
          disabled={!isFaIcon}
          title={t("Icon Color") ?? undefined}
          history={iconColorHistory}
          onChange={(next) => {
            setColor(next)
            lastPickedColor = next
          }}
        />
      </div>
      <IconPicker icon={icon} onChange={setIcon} />
      {/* Only present for a rectangle stamp (dragged to create, not a plain click) — a plain point
          stamp has no rectangle for an anchor position or outline color to apply to. Laid out as
          two stacked rows (anchor picker alone on its own row, the three smaller controls sharing
          a second row below) rather than four items crammed into one `flex` row — the anchor
          picker's own box is taller than a single label+control pair, so packed side by side the
          three narrower controls' labels ended up misaligned with each other and the whole row
          read as cramped at this dialog's 320px width (reported live: "排版太丑了"). */}
      {defaultRect && (
        <div style={{ textAlign: "left", marginBottom: 12, paddingTop: 12, borderTop: "1px solid currentColor" }}>
          <p style={{ fontWeight: "bold", margin: "0 0 10px" }}>{t("Selection Rectangle")}</p>
          <div style={{ marginBottom: 12 }}>
            <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("Icon Position")}</span>
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
                {t("Rectangle Color")}
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
              <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("Fill Style")}</span>
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
              <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("Corner Style")}</span>
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
            <span style={{ display: "block", fontSize: 13, marginBottom: 4 }}>{t("Rectangle Display")}</span>
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
        <input type="button" className="stdbtn" value={t("Cancel") ?? "Cancel"} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t("OK") ?? "OK"} onClick={submit} />
      </div>
    </div>
  )
}

// Same fragment-matching/sort rule as `Library.tsx`'s own search-bar autocomplete
// (`loadTagSuggestions`): only the piece after the last `,`/`-`/whitespace, case-insensitive
// substring, sorted by tag weight descending. Used for the dynamic-category predicate field,
// whose value is itself a LANraragi search-query string, not a plain name.
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
        // Quoted when `s.text` has a space, unlike `label` (plain text shown in the dropdown) —
        // space is a real AND-separator in the search grammar now (issue #59).
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
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    nameRef.current?.select()
  }, [])

  function submit() {
    if (!name.trim()) return
    onSubmit({ name: name.trim(), isDynamic, search: isDynamic ? search : "" })
  }

  return (
    <div onKeyDown={(e) => e.key === "Escape" && onCancel()}>
      <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{t("New Category")}</p>
      {/* Segmented tab switcher — reuses `favtag-btn`/`.toggled`, the same pill-button-row pattern
          `Library.tsx`'s category filter bar already uses for a mutually-exclusive choice, rather
          than native radio inputs (visually inconsistent with the rest of the themed UI). */}
      <div role="tablist" style={{ display: "flex", gap: 4, marginBottom: 12 }}>
        <button
          type="button"
          role="tab"
          aria-selected={!isDynamic}
          className={`favtag-btn${!isDynamic ? " toggled" : ""}`}
          style={{ flex: 1 }}
          onClick={() => setIsDynamic(false)}
        >
          {t("Static Category")}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={isDynamic}
          className={`favtag-btn${isDynamic ? " toggled" : ""}`}
          style={{ flex: 1 }}
          onClick={() => setIsDynamic(true)}
        >
          {t("Dynamic Category")}
        </button>
      </div>
      <div style={{ textAlign: "left", marginBottom: 12 }}>
        <label htmlFor="new-category-name" style={{ display: "block", fontSize: 13, marginBottom: 4 }}>
          {t("Enter a name for the new category")}
        </label>
        {/* `height: 25` — matches the tab buttons/`Edit.tsx`'s own `.stdinput`/`<select>` override
            (issue #45), since legacy theme CSS's own `.stdinput` rule is a much shorter ~18px by
            default and looks visually cramped next to this dialog's other, taller controls. */}
        <input
          ref={nameRef}
          id="new-category-name"
          type="text"
          className="stdinput"
          style={{ width: "100%", height: 25, boxSizing: "border-box" }}
          value={name}
          placeholder={t("My Category") ?? undefined}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !isDynamic) submit()
          }}
        />
      </div>
      {isDynamic && (
        <div style={{ textAlign: "left", marginBottom: 12 }}>
          <label htmlFor="new-category-search" style={{ display: "flex", alignItems: "center", gap: 4, fontSize: 13, marginBottom: 4 }}>
            {t("Search Predicate")}
            <Tooltip
              label={
                t(
                  'Same syntax as the main search bar — a plain keyword (no namespace) matches the title or any tag, exactly like typing it into that search box. Separate multiple terms with a comma or space to require all of them; prefix a term with - to exclude it. A multi-word value needs quotes to keep its words together, e.g. female:"huge breasts". Example: language:chinese, -tag:full color, or just a keyword like 旗袍',
                ) ?? undefined
              }
            >
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
        <input type="button" className="stdbtn" value={t("Cancel") ?? "Cancel"} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t("OK") ?? "OK"} onClick={submit} />
      </div>
    </div>
  )
}

/** Mounted once, app-wide (see `App.tsx`) — matches `toast.tsx`'s own `<ToastContainer>`
 * convention exactly: a single always-present host that any file's plain `promptDialog`/
 * `confirmDialog`/`newCategoryDialog` call can push a request into, regardless of which component
 * tree is currently mounted where. */
export function DialogHost() {
  const { t } = useTranslation()
  const [, forceUpdate] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const listener = () => forceUpdate((n) => n + 1)
    listeners.push(listener)
    return () => {
      listeners = listeners.filter((l) => l !== listener)
    }
  }, [])

  const request = currentRequest

  useEffect(() => {
    if (request?.kind === "prompt") inputRef.current?.select()
  }, [request])

  if (!request) return null

  function close() {
    setRequest(null)
  }

  function submitPrompt() {
    if (request?.kind !== "prompt") return
    const value = inputRef.current?.value ?? ""
    request.resolve(value)
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
        }}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel()
          if (e.key === "Enter" && request.kind === "confirm") onConfirm()
        }}
      >
        <p style={{ fontWeight: "bold", margin: "0 0 12px" }}>{request.message}</p>
        {request.kind === "prompt" && (
          <input
            ref={inputRef}
            type="text"
            className="stdinput"
            // `height: 25` matches `SettingsOverlay.tsx`'s own `CONTROL_HEIGHT` — legacy theme
            // CSS's own `.stdinput` rule defaults to a much shorter ~18px, which read as visibly
            // undersized/cramped for a modal whose only content is this one field.
            style={{ width: "100%", height: 25, boxSizing: "border-box", marginBottom: 12 }}
            defaultValue={request.defaultValue}
            placeholder={request.defaultValue || undefined}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === "Enter") onConfirm()
            }}
          />
        )}
        <div className="swal2-actions" style={{ display: "flex", justifyContent: "center", gap: 8 }}>
          <input type="button" className="stdbtn" value={t("Cancel") ?? "Cancel"} onClick={onCancel} />
          <input type="button" className="stdbtn" value={t("OK") ?? "OK"} onClick={onConfirm} />
        </div>
      </div>
    </>,
    document.body,
  )
}
