import { useEffect } from 'react'

import { useSettings } from './api/hooks'

// Matches legacy's own theme file names and display data exactly
// (`Utils/Generic.pm::css_default_data`) — the `id` is stored verbatim in the shared `LRR_CONFIG`
// Redis hash under `theme`, so this list must stay in sync with what a legacy instance (or its
// own settings page) would write there. `color` is each theme's real `body` background colour
// (read straight out of `~/LANraragi/public/themes/*.css`, not an approximation) — note that
// Nadeko and H-Verse are actually *light* themes, easy to get backwards without checking the
// real file.
export const THEMES = [
  { id: 'modern.css', name: 'Hachikuji', color: '#34353B' },
  { id: 'modern_red.css', name: 'Nadeko', color: '#E9BBC5' },
  { id: 'modern_clear.css', name: 'Yotsugi', color: '#FCFCFC' },
  { id: 'g.css', name: 'H-Verse', color: '#E3E0D1' },
  { id: 'ex.css', name: 'Sad Panda', color: '#34353B' },
] as const

export const DEFAULT_THEME_ID = 'modern.css'

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
 * recalculation). Both files are verbatim copies of legacy's own `public/css/lrr.css` and
 * `public/themes/*.css` (see `public/legacy/`), so components written against legacy's own
 * classnames (`.id1`, `.stdbtn`, `.favtag-btn`, ...) get pixel-accurate styling for free.
 *
 * Also links in two *vendor* stylesheets legacy's own templates load that aren't part of
 * LANraragi's own CSS at all, easy to miss since neither ships as a source file in the legacy
 * repo (both are pulled from their npm packages at legacy's own build time, same as the
 * Geist/Inter fonts) — fetched from the same real packages here (`blueimp-file-upload`,
 * `allcollapsible`) rather than approximated:
 * - `jquery.fileupload.css`: the `.fileinput-button` positioning (a huge, invisible, absolutely
 *   positioned `<input type=file>` layered over the visible button) every file-picker button on
 *   Upload/Backup/Plugins depends on to not render a raw native input.
 * - `allcollapsible.css`: the `.collapsible`/`.collapsible-title`/`.collapsible-body`/
 *   `.caret-right` accordion classes used by the nav carousel, Stats, Plugins, and Batch's
 *   flyouts — real padding/border/box-shadow and the `::after` caret glyph, not just a bare
 *   list. */
export function useApplyTheme() {
  const settings = useSettings()

  useEffect(() => {
    ensureLink(LEGACY_STRUCTURAL_CSS_ID, '/legacy/lrr.css')
    ensureLink(LEGACY_FILEUPLOAD_CSS_ID, '/legacy/jquery.fileupload.css')
    ensureLink(LEGACY_COLLAPSIBLE_CSS_ID, '/legacy/allcollapsible.css')
  }, [])

  useEffect(() => {
    const theme = settings.data?.theme ?? DEFAULT_THEME_ID
    document.documentElement.dataset.theme = theme
    ensureLink(LEGACY_THEME_CSS_ID, `/legacy/themes/${theme}`)
  }, [settings.data?.theme])
}
