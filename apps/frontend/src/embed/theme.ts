/**
 * The card's colours. `?theme=` (or `<lrrg-share theme>`) is either a preset name or a JSON object
 * of colours over a preset:
 *
 *   { "base": "ex", "bg": "#101820", "fg": "#f5f5f5", "muted": "#9aa", "border": "#333",
 *     "accent": "#f2aa4c", "tile": "#222", "qrFg": "#101820", "qrBg": "#f5f5f5", "qrFinder": "#f2aa4c" }
 *
 * Every key is optional. Without one, the card uses the sharing library's own theme.
 */

export const CARD_THEMES = ["system", "light", "dark", "g", "ex", "modern", "modern_red", "modern_clear"]

/** The colours an embedding page may set, each a `--card-*` variable of `card.css`. */
export const THEME_COLORS = ["bg", "fg", "muted", "border", "accent", "tile", "qrFg", "qrBg", "qrFinder"] as const

export type ThemeColor = (typeof THEME_COLORS)[number]

export interface CardTheme {
  base: string
  colors: Partial<Record<ThemeColor, string>>
}

/** Whether `value` is a CSS colour; only those reach the stylesheet, so a theme cannot inject
 * rules. */
const isCssColor = (value: string) => typeof CSS !== "undefined" && CSS.supports("color", value)

/** Reads `param` (`?theme=`); `library` is the theme the server put on the page (`g.css`). */
export function parseCardTheme(
  param: string | null,
  library: string | undefined,
  isColor: (value: string) => boolean = isCssColor,
): CardTheme {
  const fallback = library?.replace(/\.css$/, "")
  const preset = (name: unknown) =>
    typeof name === "string" && CARD_THEMES.includes(name.trim().toLowerCase()) ? name.trim().toLowerCase() : null
  const base = () => preset(fallback) ?? "system"
  const text = param?.trim() ?? ""
  if (!text.startsWith("{")) return { base: preset(text) ?? base(), colors: {} }
  let asked: unknown
  try {
    asked = JSON.parse(text)
  } catch {
    return { base: base(), colors: {} }
  }
  if (!asked || typeof asked !== "object") return { base: base(), colors: {} }
  const fields = asked as Record<string, unknown>
  const colors: CardTheme["colors"] = {}
  for (const key of THEME_COLORS) {
    const value = fields[key]
    if (typeof value === "string" && value.length <= 64 && isColor(value.trim())) colors[key] = value.trim()
  }
  return { base: preset(fields.base) ?? base(), colors }
}

/** Puts `theme` on the card root: the preset as `data-card-theme`, any colours as variables. */
export function applyCardTheme(root: HTMLElement, theme: CardTheme) {
  root.dataset.cardTheme = theme.base
  const entries = Object.entries(theme.colors)
  if (entries.length === 0) return
  const style = document.createElement("style")
  // `#card-root` outranks the preset rules (`[data-card-theme="…"] .lrrg-card`).
  style.textContent = `#${root.id} .lrrg-card { ${entries.map(([key, value]) => `--card-${key}: ${value};`).join(" ")} }`
  document.head.append(style)
}
