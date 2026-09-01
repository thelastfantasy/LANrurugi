import { useEffect } from "react"

import { THEME_STORAGE_KEY } from "@/lib/storageKeys"

import { useLoginStatus, usePublicSettings, useSettings } from "./api/hooks"

// Matches legacy's own theme file names/display data (`Utils/Generic.pm::css_default_data`);
// `id` is stored verbatim under Redis `LRR_CONFIG`'s `theme` key.
export const THEMES = [
  { id: "modern.css", name: "Hachikuji", color: "#34353B" },
  { id: "modern_red.css", name: "Nadeko", color: "#E9BBC5" },
  { id: "modern_clear.css", name: "Yotsugi", color: "#FCFCFC" },
  { id: "g.css", name: "H-Verse", color: "#E3E0D1" },
  { id: "ex.css", name: "Sad Panda", color: "#34353B" },
] as const

export const DEFAULT_THEME_ID = "modern.css"

/** Popup-menu palette per theme — real colors read off each theme's own context-menu CSS rules,
 * so the from-scratch `PopupMenu` component matches without linking a menu-plugin CSS file. */
export const MENU_PALETTE: Record<
  (typeof THEMES)[number]["id"],
  { bg: string; border: string; text: string; hoverBg: string; hoverText: string; separator: string; shadow: string }
> = {
  "modern.css": {
    bg: "#34353B",
    border: "#363940",
    text: "#FFFFFF",
    hoverBg: "#43464E",
    hoverText: "#3b97ea",
    separator: "#43464E",
    shadow: "0 2px 5px 0 rgba(0,0,0,.16), 0 2px 10px 0 rgba(0,0,0,.12)",
  },
  "modern_red.css": {
    bg: "#DCDDCB",
    border: "transparent",
    text: "#414135",
    hoverBg: "#E9A53A",
    hoverText: "#F1F1F1",
    separator: "#414135",
    shadow: "0 2px 5px 0 rgba(0,0,0,.16), 0 2px 10px 0 rgba(0,0,0,.12)",
  },
  "modern_clear.css": {
    bg: "#E1E7E9",
    border: "transparent",
    text: "#34495E",
    hoverBg: "#34495E",
    hoverText: "#ed2553",
    separator: "#34495E",
    shadow: "0 2px 5px 0 rgba(0,0,0,.16), 0 2px 10px 0 rgba(0,0,0,.12)",
  },
  "g.css": {
    bg: "#EDEADA",
    border: "#5C0D11",
    text: "#5C0D11",
    hoverBg: "#F2EFDF",
    hoverText: "#5C0D11",
    separator: "#5C0D11",
    shadow: "none",
  },
  "ex.css": {
    bg: "#4f535b",
    border: "#000000",
    text: "#DDDDDD",
    hoverBg: "#3b97ea",
    hoverText: "#f1f1f1",
    separator: "#DDDDDD",
    shadow: "none",
  },
}

// `rem` equivalents of legacy's hardcoded `pt` sizes (16px root assumed). Named by relative size
// (XS/SM/MD), not the pt value, since pt-to-rem isn't a monotonic renaming.
export const FONT_SIZE_XS = "0.667rem"

export const FONT_SIZE_MD = "0.833rem"

export const FONT_SIZE_SM = "0.75rem"

// Shared overlay layering: a fixed backdrop behind a floating menu/popup; `CONTENT` sits one level above.
export const Z_OVERLAY_BACKDROP = 1000

export const Z_OVERLAY_CONTENT = 1001

// Must win against any trigger, including one already floating at `Z_OVERLAY_CONTENT`
// (e.g. a `RatingWidget` inside a `PopupMenu` row) — comfortably above, not merely `+1`.
export const Z_OVERLAY_TOOLTIP = 1100

// Fixed, theme-independent shadow for Base UI popups — not `useMenuPalette()`'s `shadow`, since
// two of the five themes set that to `"none"` (zero elevation cue).
export const FLOATING_POPUP_SHADOW = "0 8px 24px rgba(0,0,0,0.35), 0 2px 6px rgba(0,0,0,0.25)"

// Driven by Base UI's `data-starting-style`/`data-ending-style` hooks; pair with an inline
// `transformOrigin: "var(--transform-origin)"` so scale animates from the trigger-facing edge.
export const FLOATING_POPUP_TRANSITION_CLASSES =
  "transition-[transform,opacity] duration-150 ease-out data-[starting-style]:scale-95 data-[starting-style]:opacity-0 data-[ending-style]:scale-95 data-[ending-style]:opacity-0"

// Legacy's `.base-overlay` (the Archive Overview modal) hardcodes `z-index: 9000` — anything
// rendering on top of that modal itself (not just a popup triggered from inside it) must clear it.
export const Z_OVERLAY_ABOVE_LEGACY_MODAL = 9500

const LEGACY_STRUCTURAL_CSS_ID = "legacy-structural-css"
const LEGACY_THEME_CSS_ID = "legacy-theme-css"
const LEGACY_FILEUPLOAD_CSS_ID = "legacy-fileupload-css"
const LEGACY_COLLAPSIBLE_CSS_ID = "legacy-collapsible-css"

export function ensureLink(id: string, href: string) {
  let link = document.getElementById(id) as HTMLLinkElement | null
  if (!link) {
    link = document.createElement("link")
    link.id = id
    link.rel = "stylesheet"
    // Appended (not inserted at a fixed position) so it lands after Tailwind's `@import`s —
    // same-specificity selectors are decided by source order, and legacy's CSS must win that.
    document.head.appendChild(link)
  }
  if (link.href !== href) link.href = href
}

/** Removes a `<link>` added by {@link ensureLink} — needed for page-scoped CSS an SPA route
 * change must undo by hand (legacy discards it via full page load instead). */
export function removeLink(id: string) {
  document.getElementById(id)?.remove()
}

const LEGACY_CONFIG_CSS_ID = "legacy-config-css"

/** `config.css` globally restyles every `input[type=checkbox]` into an ON/OFF toggle — shared by
 * `Settings` and `Plugins`, the two pages that link it. */
export function useLegacyConfigCss() {
  useEffect(() => {
    ensureLink(LEGACY_CONFIG_CSS_ID, "/legacy/config.css")
    return () => removeLink(LEGACY_CONFIG_CSS_ID)
  }, [])
}

/** Links legacy's real stylesheets: `lrr.css` (structural, loaded once), the currently-selected
 * theme file, and the `blueimp-file-upload`/collapsible vendor CSS legacy's templates also load. */
export function useApplyTheme() {
  const loginStatus = useLoginStatus()
  const settings = useSettings({ enabled: loginStatus.data?.logged_in === true })
  const publicTheme = usePublicSettings({ enabled: settings.data === undefined })

  useEffect(() => {
    ensureLink(LEGACY_STRUCTURAL_CSS_ID, "/legacy/lrr.css")
    ensureLink(LEGACY_FILEUPLOAD_CSS_ID, "/legacy/fileupload-vendor.css")
    ensureLink(LEGACY_COLLAPSIBLE_CSS_ID, "/legacy/allcollapsible.css")
  }, [])

  // Only fall back to `DEFAULT_THEME_ID` once both queries have genuinely settled — applying it
  // too early overwrites `index.html`'s own synchronous theme application, flashing the default.
  const settingsDisabledAndLoginStatusSettled =
    settings.data === undefined && (loginStatus.isSuccess || loginStatus.isError)
  const publicThemeEnabled = settings.data === undefined
  const settingsSettled = settings.isSuccess || settings.isError || settingsDisabledAndLoginStatusSettled
  const publicThemeSettled = !publicThemeEnabled || publicTheme.isSuccess || publicTheme.isError
  const resolvedTheme = settings.data?.theme ?? publicTheme.data?.theme
  useEffect(() => {
    if (!resolvedTheme && !(settingsSettled && publicThemeSettled)) return
    const theme = resolvedTheme ?? DEFAULT_THEME_ID
    document.documentElement.dataset.theme = theme
    ensureLink(LEGACY_THEME_CSS_ID, `/legacy/themes/${theme}`)
    // Only a real admin session's own theme belongs in `lrrTheme` — it's `index.html`'s
    // dev-server-only fallback for *that* value specifically (see that file's own docs), read
    // before any session exists. Writing a guest's resolved theme here would leave the *next*
    // guest visit's dev-mode first paint reading a stale admin-or-guest value that has nothing to
    // do with the admin's actual preference.
    if (settings.data?.theme) {
      try {
        localStorage.setItem(THEME_STORAGE_KEY, theme)
      } catch {
        /* empty */
      }
    } else if (publicThemeSettled) {
      // A confirmed guest/logged-out resolution — proactively clear any `lrrTheme` left behind by
      // an admin session (or, historically, by the `get_settings` guest-leak bug this same commit
      // fixes server-side) rather than leaving it for `index.html`'s own dev-mode fallback to keep
      // reading on every future guest visit until someone manually clears it.
      try {
        localStorage.removeItem(THEME_STORAGE_KEY)
      } catch {
        /* empty */
      }
    }
  }, [resolvedTheme, settingsSettled, publicThemeSettled, settings.data?.theme])
}
