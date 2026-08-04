import { useEffect } from "react"

import { usePublicTheme, useSettings } from "./api/hooks"
import { THEME_STORAGE_KEY } from "./storageKeys"

// Matches legacy's own theme file names and display data exactly (`Utils/Generic.pm::
// css_default_data`) — the `id` is stored verbatim in the shared `LRR_CONFIG` Redis hash under
// `theme`, so this list must stay in sync with legacy. `color` is each theme's real `body`
// background colour — note that Nadeko and H-Verse are actually light themes, easy to get
// backwards without checking the real file.
export const THEMES = [
  { id: "modern.css", name: "Hachikuji", color: "#34353B" },
  { id: "modern_red.css", name: "Nadeko", color: "#E9BBC5" },
  { id: "modern_clear.css", name: "Yotsugi", color: "#FCFCFC" },
  { id: "g.css", name: "H-Verse", color: "#E3E0D1" },
  { id: "ex.css", name: "Sad Panda", color: "#34353B" },
] as const

export const DEFAULT_THEME_ID = "modern.css"

/** Popup-menu palette per theme (right-click menu, index settings gear menu) — real colour values
 * read directly off each theme's own `.context-menu-list`/`.context-menu-item`/`.context-menu-
 * item.context-menu-hover`/`.context-menu-separator` rules, so the from-scratch Tailwind-based
 * `PopupMenu` component matches each theme's real popup styling without linking any menu-plugin
 * CSS file at all. */
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

// `lrr.css`/`allcollapsible.css` hardcode several rules in `pt` (some `!important`) — these are
// the `rem` equivalents used throughout the app instead (16px root font size assumed), so a
// pt-sized legacy rule and our own inline styles agree on the same rendered size. Naming reflects
// the legacy pt value they replace, not the rem number, since that's the meaningful constant.
export const FONT_SIZE_8PT = "0.667rem"

export const FONT_SIZE_9PT = "0.833rem"

export const FONT_SIZE_10PT = "0.75rem"

// Shared full-screen-overlay layering: a fixed, click-to-dismiss backdrop behind a floating menu/
// popup — used by Library's/the Reader's own context menus. `CONTENT` is exactly one level above
// `BACKDROP` so the popup itself always wins the stacking order.
export const Z_OVERLAY_BACKDROP = 1000

export const Z_OVERLAY_CONTENT = 1001

// `Tooltip` needs to win against *any* trigger it's attached to, including one that's itself
// already floating at `Z_OVERLAY_CONTENT` (e.g. a `RatingWidget` rendered as a row inside a
// `PopupMenu`, per `Library.tsx`'s context menu) — confirmed via a real screenshot where the
// tooltip rendered visibly *behind* the popup menu's own rows using `Z_OVERLAY_BACKDROP` (equal
// to the menu's backdrop layer, one level below the menu content itself). Comfortably above
// `Z_OVERLAY_CONTENT` rather than merely `+1`, so a future overlay layer inserted between the two
// doesn't silently reopen this same gap.
export const Z_OVERLAY_TOOLTIP = 1100

// Legacy's own real `.base-overlay` class (`lrr.css` — the Archive Overview modal's own outer
// class, `#archivePagesOverlay`) carries a hardcoded `z-index: 9000`, far above every generic
// overlay tier above — anything meant to render *on top of* that modal (rather than as a popup
// triggered from inside it, which the tiers above already cover) needs to clear 9000 specifically,
// not just the app's own internal overlay stack. `PageLightbox` (`ArchiveOverviewOverlay.tsx`) is
// the first, and so far only, real case of this: a confirmed live bug where its own `Z_OVERLAY_
// CONTENT` (1001) silently lost the stacking fight to the still-open Archive Overview modal
// underneath it, rendering the lightbox's own dark backdrop and content invisible behind that
// modal despite mounting later in the React tree (DOM/mount order doesn't override an explicit,
// much higher `z-index`).
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
    // Appended (not inserted at a fixed position) so it always lands after anything Tailwind's
    // own `@import`s already put in `<head>` at module-load time — same-specificity element
    // selectors (`body`, `.stdbtn`, ...) are decided by source order, and legacy's real CSS must
    // win that tiebreak for pixel parity, not just contribute a bunch of overridden dead rules.
    document.head.appendChild(link)
  }
  if (link.href !== href) link.href = href
}

/** Removes a `<link>` added by {@link ensureLink}. For CSS that's only meant to apply to a single
 * page (unlike the always-on structural/theme/vendor links below) — legacy loads it via a full
 * page load that naturally discards it on navigation, so an SPA route away must undo it by hand
 * to avoid leaking page-scoped rules (e.g. `config.css`'s global `input[type=checkbox]` override)
 * onto other routes. */
export function removeLink(id: string) {
  document.getElementById(id)?.remove()
}

const LEGACY_CONFIG_CSS_ID = "legacy-config-css"

/** `config.css` (real vendor file, `~/LANraragi/public/css/config.css`) is only ever linked by
 * legacy's own `config.html.tt2`/`plugins.html.tt2` — it globally restyles every
 * `input[type=checkbox]` on the page into the ON/OFF toggle look. Legacy gets this "for free"
 * since navigating away is a full page load; an SPA has to link/unlink it by hand so those rules
 * don't leak onto other routes. Shared by `Settings` and `Plugins` (the only two real legacy pages
 * that link it) rather than each hand-rolling the identical `useEffect`. */
export function useLegacyConfigCss() {
  useEffect(() => {
    ensureLink(LEGACY_CONFIG_CSS_ID, "/legacy/config.css")
    return () => removeLink(LEGACY_CONFIG_CSS_ID)
  }, [])
}

/** Links in legacy's own real stylesheets — `lrr.css` (structural, theme-independent, loaded
 * once) and whichever theme file is currently selected (swapped by changing one `<link>`'s
 * `href`, exactly how legacy's own settings page switches themes — not a CSS-variable
 * recalculation). Both are verbatim copies of legacy's own CSS (see `public/legacy/`), so
 * components written against legacy's own classnames get pixel-accurate styling for free.
 *
 * Also links a vendor stylesheet legacy's own templates load that isn't part of LANraragi's own
 * CSS (pulled from an npm package at legacy's build time), fetched from the same real package
 * here (`blueimp-file-upload`) rather than approximated:
 * - `fileupload-vendor.css`: the `.fileinput-button` positioning (an invisible, absolutely
 *   positioned `<input type=file>` layered over the visible button) every file-picker button
 *   depends on to not render a raw native input.
 * - `allcollapsible.css`: the `.collapsible`/`.collapsible-title`/`.collapsible-body`/
 *   `.caret-right` accordion classes used by the nav carousel, Stats, Plugins, and Batch.
 *
 * Popup menus are a from-scratch `PopupMenu` component styled with Tailwind + the `MENU_PALETTE`
 * table above — no menu-plugin CSS file is linked in for them at all. */
export function useApplyTheme() {
  const settings = useSettings()
  // Only ever actually fetched when `settings` has no data to offer (see the enabled check below)
  // — i.e. pre-login, where `/settings` 401s. Once authenticated, `settings.data` wins and this
  // stays idle.
  const publicTheme = usePublicTheme({ enabled: settings.data === undefined })

  useEffect(() => {
    ensureLink(LEGACY_STRUCTURAL_CSS_ID, "/legacy/lrr.css")
    ensureLink(LEGACY_FILEUPLOAD_CSS_ID, "/legacy/fileupload-vendor.css")
    ensureLink(LEGACY_COLLAPSIBLE_CSS_ID, "/legacy/allcollapsible.css")
  }, [])

  // `settings`/`publicTheme` are both `undefined` for one or more renders after mount, before
  // either query has actually resolved — naively falling back to `DEFAULT_THEME_ID` during that
  // window overwrites the theme `index.html`'s own inline script already applied synchronously
  // (from the server-injected `data-theme` attribute, or a cached `localStorage` value) with the
  // hardcoded default, then swaps it back once the real value arrives: a real, live-confirmed
  // flash-of-default-theme regression (issue #58 follow-up), not a hypothetical one — the *fix*
  // for that flash was itself re-introducing it. Only apply the `DEFAULT_THEME_ID` fallback once
  // both queries have actually settled (succeeded or errored) with no real value between them —
  // until then, leave whatever's already applied alone rather than guessing.
  const publicThemeEnabled = settings.data === undefined
  const settingsSettled = settings.isSuccess || settings.isError
  const publicThemeSettled = !publicThemeEnabled || publicTheme.isSuccess || publicTheme.isError
  const resolvedTheme = settings.data?.theme ?? publicTheme.data?.theme
  useEffect(() => {
    if (!resolvedTheme && !(settingsSettled && publicThemeSettled)) return
    const theme = resolvedTheme ?? DEFAULT_THEME_ID
    document.documentElement.dataset.theme = theme
    ensureLink(LEGACY_THEME_CSS_ID, `/legacy/themes/${theme}`)
    // Cache for `index.html`'s own inline script (see that file's own docs) to apply synchronously,
    // before paint, on the *next* visit — this run is always too late for that to help this one.
    // Only once a real theme value has actually arrived (not the `DEFAULT_THEME_ID` fallback) —
    // caching the fallback would let a slow network turn one single slow load into a permanently-
    // wrong cached theme for every visit after it.
    if (resolvedTheme) {
      try {
        localStorage.setItem(THEME_STORAGE_KEY, theme)
      } catch {
        // localStorage can throw (private browsing, disabled storage, etc.) — losing the cache is
        // harmless, just means the next visit won't get the synchronous head start.
      }
    }
  }, [resolvedTheme, settingsSettled, publicThemeSettled])
}
