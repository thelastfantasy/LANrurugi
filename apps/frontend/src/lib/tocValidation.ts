import { z } from 'zod'

// Real, stored ToC-entry identifiers for the "presets" (quick-add popover/lightbox rows, and the
// lightbox's own 0-9 keyboard shortcut) — `toc`, `c1`-`c20`. Unlike free-text chapter titles a
// user types by hand, these are meant to be *unique per archive*: re-setting "chapter 4" on a new
// page should move it, not leave a second stale "chapter 4" behind at the old page. Storing the
// identifier itself (rather than already-localized display text) is what makes that dedup
// possible on the backend (`add_toc_entry` in `crates/lanrurugi-api/src/archives.rs`) without the
// backend needing to know anything about locales — see that function's own docs for the dedup
// logic, and `TOC_CHAPTER_COUNT` below for why 20.
//
// The real, live-visible tradeoff: a legacy LANraragi instance (or any client that doesn't apply
// this app's own `displayTocName` mapping below) will show the raw "c4"/"toc" string instead of
// localized text if it opens an archive whose ToC was set through this app. Deliberately accepted
// (per explicit user decision) in exchange for guaranteed uniqueness — the fallback title case
// (free-text titles typed by hand, not sent by a preset) is completely unaffected and keeps
// storing real display text as before.
export const TOC_CHAPTER_COUNT = 20

export const TOC_IDENTIFIER_TABLE_OF_CONTENTS = 'toc'

export function tocChapterIdentifier(n: number): string {
  return `c${n}`
}

const RESERVED_TOC_IDENTIFIERS = new Set([
  TOC_IDENTIFIER_TABLE_OF_CONTENTS,
  ...Array.from({ length: TOC_CHAPTER_COUNT }, (_, i) => tocChapterIdentifier(i + 1)),
])

export function isReservedTocIdentifier(value: string): boolean {
  return RESERVED_TOC_IDENTIFIERS.has(value.trim().toLowerCase())
}

/** Maps a stored ToC entry's `name` to what should actually be shown in the UI — a reserved
 * identifier (`c1`-`c20`/`toc`) resolves through `t()` to localized display text ("第 1 章"/
 * "目录"), anything else (a free-text title the user typed by hand) is shown completely as-is,
 * since it was never anything but real display text to begin with. */
export function displayTocName(name: string, t: (key: string, opts?: Record<string, unknown>) => string | null): string {
  const lower = name.trim().toLowerCase()
  if (lower === TOC_IDENTIFIER_TABLE_OF_CONTENTS) return t('Table of Contents') ?? 'Table of Contents'
  if (/^c([0-9]{1,2})$/.test(lower)) {
    const n = Number(lower.slice(1))
    if (n >= 1 && n <= TOC_CHAPTER_COUNT) return t('Chapter {{n}}', { n }) ?? `Chapter ${n}`
  }
  return name
}

// Manual "add/edit chapter" prompt (`promptDialog`-backed, see `ArchiveOverviewOverlay.tsx`'s own
// `promptTocTitle`) still only ever stores real, free-text display text — it has no UI concept of
// "pick chapter 4", so there's no legitimate reason for a hand-typed title to collide with the
// reserved identifier namespace above. Blocking that collision (rather than silently letting a
// manually-typed "c4" alias onto the same stored value the preset would produce) avoids a
// confusing case where typing "c4" by hand and clicking the "Chapter 4" preset would overwrite
// each other despite looking like two unrelated entries in the UI.
export const tocTitleSchema = z
  .string()
  .trim()
  .min(1)
  .refine((value) => !isReservedTocIdentifier(value), {
    message: 'reserved-toc-identifier',
  })
