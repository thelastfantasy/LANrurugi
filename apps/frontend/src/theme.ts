import { useEffect } from 'react'

import { useSettings } from './api/hooks'

// Matches legacy's own theme file names and display data exactly (`Utils/Generic.pm::
// css_default_data`) — the `id` is stored verbatim in the shared `LRR_CONFIG` Redis hash under
// `theme`, so this list must stay in sync with legacy. `color` is each theme's real `body`
// background colour — note that Nadeko and H-Verse are actually light themes, easy to get
// backwards without checking the real file.
export const THEMES = [
  { id: 'modern.css', name: 'Hachikuji', color: '#34353B' },
  { id: 'modern_red.css', name: 'Nadeko', color: '#E9BBC5' },
  { id: 'modern_clear.css', name: 'Yotsugi', color: '#FCFCFC' },
  { id: 'g.css', name: 'H-Verse', color: '#E3E0D1' },
  { id: 'ex.css', name: 'Sad Panda', color: '#34353B' },
] as const

export const DEFAULT_THEME_ID = 'modern.css'

/** Popup-menu palette per theme (right-click menu, index settings gear menu) — real colour values
 * read directly off each theme's own `.context-menu-list`/`.context-menu-item`/`.context-menu-
 * item.context-menu-hover`/`.context-menu-separator` rules, so the from-scratch Tailwind-based
 * `PopupMenu` component matches each theme's real popup styling without linking any menu-plugin
 * CSS file at all. */
export const MENU_PALETTE: Record<
  (typeof THEMES)[number]['id'],
  { bg: string; border: string; text: string; hoverBg: string; hoverText: string; separator: string; shadow: string }
> = {
  'modern.css': {
    bg: '#34353B',
    border: '#363940',
    text: '#FFFFFF',
    hoverBg: '#43464E',
    hoverText: '#3b97ea',
    separator: '#43464E',
    shadow: '0 2px 5px 0 rgba(0,0,0,.16), 0 2px 10px 0 rgba(0,0,0,.12)',
  },
  'modern_red.css': {
    bg: '#DCDDCB',
    border: 'transparent',
    text: '#414135',
    hoverBg: '#E9A53A',
    hoverText: '#F1F1F1',
    separator: '#414135',
    shadow: '0 2px 5px 0 rgba(0,0,0,.16), 0 2px 10px 0 rgba(0,0,0,.12)',
  },
  'modern_clear.css': {
    bg: '#E1E7E9',
    border: 'transparent',
    text: '#34495E',
    hoverBg: '#34495E',
    hoverText: '#ed2553',
    separator: '#34495E',
    shadow: '0 2px 5px 0 rgba(0,0,0,.16), 0 2px 10px 0 rgba(0,0,0,.12)',
  },
  'g.css': {
    bg: '#EDEADA',
    border: '#5C0D11',
    text: '#5C0D11',
    hoverBg: '#F2EFDF',
    hoverText: '#5C0D11',
    separator: '#5C0D11',
    shadow: 'none',
  },
  'ex.css': {
    bg: '#4f535b',
    border: '#000000',
    text: '#DDDDDD',
    hoverBg: '#3b97ea',
    hoverText: '#f1f1f1',
    separator: '#DDDDDD',
    shadow: 'none',
  },
}

// `lrr.css`/`allcollapsible.css` hardcode several rules in `pt` (some `!important`) — these are
// the `rem` equivalents used throughout the app instead (16px root font size assumed), so a
// pt-sized legacy rule and our own inline styles agree on the same rendered size. Naming reflects
// the legacy pt value they replace, not the rem number, since that's the meaningful constant.
export const FONT_SIZE_8PT = '0.667rem'

export const FONT_SIZE_9PT = '0.833rem'

export const FONT_SIZE_10PT = '0.75rem'

// Shared full-screen-overlay layering: a fixed, click-to-dismiss backdrop behind a floating menu/
// tooltip/popup — used by Library's/the Reader's own context menus and Tooltip. `CONTENT` is
// exactly one level above `BACKDROP` so the popup itself always wins the stacking order.
export const Z_OVERLAY_BACKDROP = 1000

export const Z_OVERLAY_CONTENT = 1001

const LEGACY_STRUCTURAL_CSS_ID = 'legacy-structural-css'
const LEGACY_THEME_CSS_ID = 'legacy-theme-css'
const LEGACY_FILEUPLOAD_CSS_ID = 'legacy-fileupload-css'
const LEGACY_COLLAPSIBLE_CSS_ID = 'legacy-collapsible-css'

export function ensureLink(id: string, href: string) {
  let link = document.getElementById(id) as HTMLLinkElement | null
  if (!link) {
    link = document.createElement('link')
    link.id = id
    link.rel = 'stylesheet'
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

  useEffect(() => {
    ensureLink(LEGACY_STRUCTURAL_CSS_ID, '/legacy/lrr.css')
    ensureLink(LEGACY_FILEUPLOAD_CSS_ID, '/legacy/fileupload-vendor.css')
    ensureLink(LEGACY_COLLAPSIBLE_CSS_ID, '/legacy/allcollapsible.css')
  }, [])

  useEffect(() => {
    const theme = settings.data?.theme ?? DEFAULT_THEME_ID
    document.documentElement.dataset.theme = theme
    ensureLink(LEGACY_THEME_CSS_ID, `/legacy/themes/${theme}`)
  }, [settings.data?.theme])
}
