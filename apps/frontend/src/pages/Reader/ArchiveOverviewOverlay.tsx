import { useQueryClient } from '@tanstack/react-query'
import type { MouseEvent, ReactNode } from 'react'
import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import {
  useAddTocEntry,
  useArchivePages,
  useCreateCategory,
  useRemoveTocEntry,
  useSetArchiveThumbnail,
  useSettings,
  useStampedPages,
} from '../../api/hooks'
import type { ArchiveMetadata, CategoryMetadata } from '../../api/types'
import { PopupMenu, PopupMenuItem, useMenuPalette } from '../../components/PopupMenu'
import RatingWidget from '../../components/RatingWidget'
import StarRatingDisplay from '../../components/StarRating'
import Tooltip from '../../components/Tooltip'
import { confirmDialog, newCategoryDialog, promptDialog } from '../../dialog'
import { parseRating } from '../../lib/rating'
import { formatTimestampForDisplay, getTagSearchURL, TIMESTAMP_NAMESPACE } from '../../lib/tagFormat'
import {
  displayTocName,
  isReservedTocIdentifier,
  TOC_CHAPTER_COUNT,
  TOC_IDENTIFIER_TABLE_OF_CONTENTS,
  tocChapterIdentifier,
} from '../../lib/tocValidation'
import { routes } from '../../routes'
import { Z_OVERLAY_ABOVE_LEGACY_MODAL, Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from '../../theme'
import { toast } from '../../toast'

// `TIMESTAMP_NAMESPACE` + `formatTimestampForDisplay` (re-exported from `lib/tagFormat`) live
// there rather than here so the Library grid card's tag tooltip (`colorCodeTags`) and this
// overview's tag table share the exact same timestamp-formatting logic — including the
// server-timezone setting both now thread through.

// How many `ChapterActionMenu` instances are currently mounted/open — a plain module-level
// counter, not React state, since it exists purely to answer one synchronous yes/no question
// inside another component's own capture-phase `keydown` handler (`PageLightbox`'s own Escape
// listener, below), not to drive any render.
//
// Why this exists: both `PageLightbox` and `ChapterActionMenu` register their own capture-phase
// `Escape` listener on `window` (`PageLightbox`'s to close the lightbox, `ChapterActionMenu`'s to
// close just the menu). Per the DOM spec, multiple capture-phase listeners on the *same* target
// fire in **registration order**, not "whichever opened most recently wins" — since
// `PageLightbox` always mounts (and registers) before a user can ever open a menu from inside it,
// `PageLightbox`'s own listener necessarily runs first on every Escape press, no matter what the
// menu's own listener does after it (a real, live-confirmed bug: opening "Delete Chapter" from
// inside the lightbox and pressing Escape closed the whole lightbox in one keypress instead of
// just backing out of that one menu — `ChapterActionMenu`'s own `stopImmediatePropagation()`
// fired too late to matter, since `PageLightbox`'s handler had already run and closed it first).
// Checking this counter lets `PageLightbox`'s handler recognize "a menu is currently open above
// me" and skip acting, so that same Escape press closes only the menu — exactly the layered-popup
// behavior a user would expect, achieved without needing to fight event-registration order at all.
let openChapterActionMenuCount = 0

function displayNamespace(key: string): string {
  if (key === 'date_added') return 'Date Added'
  return key.charAt(0).toUpperCase() + key.slice(1)
}

function formatTagValue(namespace: string, value: string, timezone: string): string {
  if (!TIMESTAMP_NAMESPACE.test(namespace)) return value
  return formatTimestampForDisplay(value, timezone)
}

/** Mirrors legacy's `splitTagsByNamespace` + `buildTagsDiv` (`~/LANraragi/public/js/mod/common.js`)
 * — groups a flat comma-separated tag string by its `namespace:value` prefix (untagged values fall
 * under `other`), rendered as a `caption-namespace` row per namespace with each value as a
 * clickable search-link chip. `rating:` gets its own gold-star rendering instead of the raw tag
 * value (see the `namespace === 'rating'` branch below) — legacy's own real overview page shows
 * the star icons in this table *in addition to* the separate interactive `RatingWidget` above it
 * (confirmed against a real screenshot of a rated archive), so this table must render it too, not
 * skip it. Still a real, working search-link chip underneath, though — legacy's own real rating
 * chip *is* clickable (a real user-confirmed link, e.g. `?q=rating%3A⭐⭐⭐⭐⭐$` against a live
 * legacy instance), a link this port's own `q=rating:2.5$` (the equivalent search against this
 * app's own decimal-encoded storage format — verified live: correctly returns exactly the archive
 * carrying that tag) actually and correctly answers, unlike an earlier version of this component
 * that dropped the link entirely on the assumption nobody would search by star count — wrong,
 * since legacy itself treats it as a completely ordinary searchable tag. No underline on it
 * specifically, though (a real, deliberate deviation, not a bug) — legacy's own underlined
 * rating-star link reads like a broken/dead link at a glance, which the star icons alone don't
 * need to invite. */
function TagsTable({ tags }: { tags: string }) {
  // Server timezone for `date_added`/`timestamp` tag display + search-URL date-range conversion
  // (see `lib/tagFormat.ts`'s `formatTimestampForDisplay`/`getTagSearchURL`). Falls back to the
  // browser's local timezone if settings haven't loaded yet, matching the pre-feature behavior.
  const settings = useSettings()
  const timezone = settings.data?.timezone ?? ''
  if (!tags) return null
  const byNamespace = new Map<string, string[]>()
  for (const raw of tags.split(',')) {
    const tag = raw.trim()
    if (!tag) continue
    const idx = tag.indexOf(':')
    const namespace = idx === -1 ? 'other' : tag.slice(0, idx).trim()
    const value = idx === -1 ? tag : tag.slice(idx + 1).trim()
    const list = byNamespace.get(namespace) ?? []
    list.push(value)
    byNamespace.set(namespace, list)
  }

  const namespaces = [...byNamespace.keys()].sort()
  if (namespaces.length === 0) return null

  return (
    <table className="itg" style={{ boxShadow: 'none', border: 'none', borderRadius: 0 }}>
      <tbody>
        {namespaces.map((namespace) => (
          <tr key={namespace}>
            <td className={`caption-namespace ${namespace.toLowerCase()}-tag`}>
              {displayNamespace(namespace)}:
            </td>
            <td>
              {namespace.toLowerCase() === 'rating' ? (
                <div className="gt">
                  <a
                    href={getTagSearchURL(namespace, (byNamespace.get(namespace) ?? [])[0] ?? '')}
                    onClick={(e) => e.stopPropagation()}
                    style={{ textDecoration: 'none' }}
                  >
                    <StarRatingDisplay rating={parseRating((byNamespace.get(namespace) ?? [])[0]) ?? 0} size={16} />
                  </a>
                </div>
              ) : (
                (byNamespace.get(namespace) ?? []).map((value) => (
                  <div className="gt" key={value}>
                    {/* `source` is a link to an external, third-party site — real `target="_blank"`
                        so it opens a new tab instead of navigating the reader away, matching
                        `TagTable.tsx`'s own real `source` branch (this table predates that shared
                        component and never got the same split when it landed there; this was a
                        real, independently-discovered bug, not a copy of an already-fixed one). */}
                    {namespace === 'source' ? (
                      <a
                        href={getTagSearchURL(namespace, value, timezone)}
                        target="_blank"
                        rel="noreferrer"
                        onClick={(e) => e.stopPropagation()}
                      >
                        {value}
                      </a>
                    ) : (
                      <a href={getTagSearchURL(namespace, value, timezone)} onClick={(e) => e.stopPropagation()}>
                        {formatTagValue(namespace, value, timezone)}
                      </a>
                    )}
                  </div>
                ))
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/** One page thumbnail in the overview grid — shows a spin icon (same `fa-circle-notch fa-spin`
 * class legacy's own equivalent uses) while its `<img>` hasn't loaded yet, removed once it has.
 *
 * Legacy's real equivalent (`reader.js`'s `updateArchiveOverlay`/`generateThumbnails`) instead
 * polls a Minion job's progress notes to know which pages have a generated thumbnail yet, since
 * legacy pre-extracts thumbnails as a separate background step. This app's own
 * `GET /archives/{id}/thumbnail?page=N` has no such split — a cache miss regenerates synchronously
 * and blocks the same request until it's ready (see that handler's own docs,
 * `crates/lanrurugi-api/src/archives.rs`) — so the browser's native `<img>` `onLoad` event already
 * *is* the real "this page's thumbnail is ready" signal, with nothing else to poll.
 *
 * Positioned via real `position: absolute` centering within the parent `.id3.quick-thumbnail`
 * (itself `position: relative` — set by the caller), rather than reusing `Library.tsx`'s
 * `.ttspinner` class as-is: that class's own CSS is a `top: -162px` *relative* offset tuned
 * specifically for `ArchiveCard`'s layout, where a full-size `wait_warmly.jpg` placeholder
 * `<img>` occupies real space immediately before it in DOM flow — this grid's cards have no such
 * placeholder image, so the same fixed offset pushed the icon above the card entirely (confirmed
 * via a real `getBoundingClientRect()` comparison: the spinner's rect landed above the card's own
 * top edge, not inside it).
 *
 * Hides the not-yet-loaded `<img>` with `visibility: hidden` (which still occupies real layout
 * space, so the browser can compute whether it intersects the viewport), never `display: none`
 * (which removes it from layout, and Chrome's real `loading="lazy"` never fires the network
 * request for an image in that state at all — confirmed live: an earlier version of this
 * component that used `display: none` here left every one of a 293-page archive's thumbnails
 * stuck on their spinner forever, `list_network_requests` showing zero `thumbnail?page=N`
 * requests ever fired). `Library.tsx`'s own `ArchiveCard` gets away with `display: none` only
 * because its thumbnail `<img>` was never marked `loading="lazy"` to begin with.
 *
 * The parent `.quick-thumbnail` cell has no width of its own (`lrr.css` only sets a
 * `min-width: 100px` "hint" for `loading="lazy"`) — its real width normally comes from the loaded
 * `<img>` inside stretching it via `object-fit: cover` at each page's own aspect ratio (confirmed
 * live via `getBoundingClientRect()`: fully-loaded cells in the same grid ranged 185-277px wide).
 * A cell whose image hasn't loaded yet has nothing to stretch it, so it collapses to that bare
 * 100px minimum — visibly narrower than its loaded siblings, producing the jarring width jump/grid
 * reflow issue #57 reports (confirmed live: a still-loading cell measured exactly 100px wide next
 * to 185-277px loaded ones). Setting an explicit `width` here — sized to a 1:√2 (ISO 216 A4/B4)
 * page aspect ratio against the grid's own fixed `max-height: 275px`/`width: 95%` image box
 * (`lrr.css`) rather than a phone-screen-like 9:16 (an earlier version of this fix used 9:16,
 * visibly too narrow against real manga scans, which are almost always printed on A4/B4 paper —
 * 1:√2 ≈ 0.707:1, not 9:16's 0.5625:1), i.e. `(275 / sqrt(2)) / 0.95 ≈ 205px` — gives the
 * placeholder a size in the same ballpark as a real loaded cell instead of the bare CSS minimum,
 * without needing to touch the shared legacy stylesheet itself. */
const PLACEHOLDER_WIDTH_PX = 205

function OverviewThumbnail({ src, alt }: { src: string; alt: string | undefined }) {
  const [loaded, setLoaded] = useState(false)
  return (
    // `height: 100%` always applies, loaded or not — this `div` sits inside `.quick-thumbnail`
    // (`.id3`, fixed at `height: 280px` via the active theme's own CSS), and the real `<img>`
    // inside relies on inheriting a concrete (non-`auto`) height through this wrapper for its own
    // `height: 100%` (`div.id3:not(.nocrop) img`, `lrr.css`) to resolve against — dropping this
    // wrapper's height once `loaded` flips true (an earlier version did `loaded ? undefined :
    // {...}`, removing the style entirely) left the wrapper's own height as "auto" (shrunk to the
    // image's natural content size, since nothing else constrains it), which broke that percentage
    // chain: `height: 100%` inside a wrapper whose own height depends on that same image both
    // being `auto` resolves to nothing sensible, and the image silently fell back to rendering at
    // its own natural aspect ratio instead of `object-fit: cover`-filling the cell — confirmed live
    // via `getBoundingClientRect()`: the image rendered visibly shorter than the 280px cell, with
    // real gaps top/bottom, and every position/size that assumes the image fills the cell (the
    // three corner action icons, the "第 N 页" label, the pulsing highlight outline) landed
    // relative to the *cell*'s own true bounds, not the visually-short image inside it — issue
    // (from a live screenshot) reported as icons "off in the corners" and background/position
    // looking shifted. `width` only needs to be set before load (see `PLACEHOLDER_WIDTH_PX`'s own
    // docs) — once loaded, `width: 95%` off the now-properly-tall wrapper is what actually
    // determines the image's rendered width, exactly matching a real loaded cell in this same grid.
    <div style={{ height: '100%', ...(loaded ? undefined : { width: PLACEHOLDER_WIDTH_PX }) }}>
      {!loaded && (
        // The centering transform lives on this plain, non-animated wrapper, not on the `<i>`
        // itself — `fa-spin`'s own CSS animation drives the icon's `transform` (a rotation) every
        // frame, which silently overwrote a `translate(-50%, -50%)` placed directly on the same
        // element (only one `transform` can apply at a time; they don't compose) and put the
        // icon's rotation pivot at the card's top-left corner instead of centered on it —
        // confirmed live via `getBoundingClientRect()`: the icon's rendered center sat well right
        // of the card's true horizontal center. Splitting the two transforms across parent/child
        // is what lets both apply independently.
        <span
          style={{
            position: 'absolute',
            top: '50%',
            left: '50%',
            transform: 'translate(-50%, -50%)',
          }}
        >
          <i className="fa fa-4x fa-circle-notch fa-spin" aria-hidden="true"></i>
        </span>
      )}
      <img
        loading="lazy"
        alt={alt}
        src={src}
        style={loaded ? undefined : { visibility: 'hidden' }}
        onLoad={() => setLoaded(true)}
      />
    </div>
  )
}

/** The "第 N 页" label shown over a page-grid cell — its own component (was previously a plain
 * `<span className="page-number">`, sharing that CSS class with the two hover-reveal buttons
 * below purely because legacy's own markup groups all three under it). Genuinely centered
 * (`left: 50%` + `translateX(-50%)`) rather than legacy's own real `left: 30%` (verified against
 * `lrr.css`) — that value was never actually a deliberate "off-center" design choice to preserve,
 * just an artifact of the label's own text width never being accounted for in a fixed percentage;
 * a real centering rule is what "第 N 页" visibly reads as trying to be. */
function PageNumberLabel({ children }: { children: ReactNode }) {
  return (
    <span
      className="page-number"
      style={{ left: '50%', transform: 'translateX(-50%)' }}
    >
      {children}
    </span>
  )
}

/** One of the two hover-revealed action buttons in a page-grid cell (`SetThumbnailButton`/
 * `AddChapterButton` below) — split out from a shared `page-number` class (legacy's own
 * `reader.js` markup puts the page-number label and both buttons under that one class, since all
 * three want the same `position: absolute` + hidden-until-hovered behavior) into its own
 * component with its own React-driven hover state, once their *positions* stopped actually
 * matching each other (the label is now genuinely centered — see `PageNumberLabel` — while the
 * buttons anchor to a `right`-anchored corner instead). Three unrelated things sharing one CSS
 * class/hover-reveal mechanism just because they used to occupy the same *area* was more coupling
 * than the actual relationship between them warranted once that stopped being true. */
function PageGridActionIcon({
  icon,
  corner,
  title,
  hovered,
  onClick,
  onContextMenu,
}: {
  icon: string
  /** `'top-right'`/`'bottom-right'` are legacy's own two real icons (`top: 2%`/`top: 80%`, both
   * computed from the top and always `right: 2%` — verified against `reader.js`). `'bottom-left'`
   * (the lightbox magnifying-glass icon) is purely additive, no legacy equivalent, mirroring the
   * same corner/margin convention on the opposite side. */
  corner: 'top-right' | 'bottom-right' | 'bottom-left'
  title: string | undefined
  /** Lifted to the parent `.quick-thumbnail` cell rather than tracked on this element itself —
   * at rest this icon sits at `z-index: -1`, *behind* the thumbnail `<img>`, so the pointer never
   * actually reaches it to fire its own `onMouseEnter` in the first place (confirmed live: a
   * version of this component with its own local hover state never once revealed itself, since
   * entering it was exactly the thing being behind another element prevented). Legacy's real
   * equivalent (`.quick-thumbnail:hover>.page-number`) sidesteps this the same way, by keying off
   * the *parent's* hover instead of the icon's own. */
  hovered: boolean
  onClick: (e: MouseEvent) => void
  /** Only the "add chapter" icon actually supplies this (see `QuickAddTocPopover`) — additive,
   * no legacy equivalent at all. */
  onContextMenu?: (e: MouseEvent) => void
}) {
  const vertical = corner === 'top-right' ? 'top' : 'bottom'
  const horizontal = corner === 'bottom-left' ? 'left' : 'right'
  return (
    <a
      href="#"
      title={title}
      className={`fas ${icon}`}
      onClick={onClick}
      onContextMenu={onContextMenu}
      style={{
        position: 'absolute',
        // Legacy's own two real values here are `top: 2%`/`top: 80%` (verified against
        // `reader.js`) — both computed from the top, so a `bottom`-anchored icon's exact position
        // depends on the cell's own height. `bottom: 2%` instead expresses the actually-intended
        // "pinned near this corner" relationship directly, independent of cell height — the same
        // reasoning that motivated `right`/`left` over a percentage for the horizontal axis.
        [vertical]: '2%',
        [horizontal]: '2%',
        padding: 12,
        fontSize: 20,
        color: 'lightskyblue',
        // Mirrors legacy's own real `.quick-thumbnail:hover>.page-number` rule (`lrr.css`) —
        // `z-index: -1` at rest (behind the thumbnail `<img>`, effectively invisible), `300` +
        // a black backdrop once actually hovered, driven here by this component's own React
        // state rather than that shared CSS selector (see this component's own docs for why).
        zIndex: hovered ? 300 : -1,
        backgroundColor: hovered ? '#000000' : undefined,
      }}
    />
  )
}

/** A ToC entry's own real span: which page it starts on, and how many pages belong to it (up to
 * but not including the next entry's start page, or the archive's own last page for the final
 * entry) — `count` includes the start page itself, matching how a real reader would count "第一章
 * (13)" as 13 pages total starting from the chapter-start page, not 13 pages *after* it. */
function tocChapterSpans(toc: { page: number; name: string }[], totalPages: number): { page: number; name: string; count: number }[] {
  const sorted = [...toc].sort((a, b) => a.page - b.page)
  return sorted.map((entry, i) => ({
    ...entry,
    count: (sorted[i + 1]?.page ?? totalPages + 1) - entry.page,
  }))
}

/** Which chapter (if any) a given page belongs to, per `tocChapterSpans` above, plus whether that
 * page is the chapter's own start page (`isStart`) — the lightbox's info bar and filmstrip labels
 * both need this same "which chapter, and is this the first page of it" distinction: the start
 * page gets bold/accent styling with no count suffix, later pages get plain styling plus a
 * "(N)" page-count suffix showing how many pages the chapter spans in total. */
function chapterForPage(
  spans: { page: number; name: string; count: number }[],
  page: number,
): { page: number; name: string; count: number; isStart: boolean; ordinal: number } | undefined {
  const span = [...spans].filter((c) => c.page <= page).sort((a, b) => b.page - a.page)[0]
  // `ordinal`: this specific page's own 1-based position within the chapter (the start page
  // itself is 1) — distinct from `count`, which is the chapter's fixed *total* span. The
  // filmstrip needs `ordinal` (a running "第 1 章 (5)" that increments frame to frame, showing
  // how far into the chapter this particular thumbnail is) rather than `count` (which would
  // repeat the same total on every frame and not actually distinguish them).
  return span ? { ...span, isStart: span.page === page, ordinal: page - span.page + 1 } : undefined
}

// Small, fixed palette of hue offsets (not fully random per render — a stable, deterministic hash
// of the chapter name so the same chapter always gets the same swatch color across re-renders/
// filmstrip scroll) rotated through HSL space; lightness/saturation are fixed at values that read
// clearly as a small color chip against either a light or dark themed background, adjusted per
// `chapterSwatchColor`'s own `onDark` param for contrast rather than picked once and hoping.
function hashString(s: string): number {
  let h = 0
  for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) >>> 0
  return h
}

/** Deterministic swatch color for a chapter name — same name always maps to the same hue, and
 * lightness is chosen for contrast against the active theme's own background (`onDark`: whether
 * that background reads as dark, per a simple luminance check on `palette.bg` at the call site)
 * rather than a single fixed lightness that would wash out on one theme or the other. */
function chapterSwatchColor(name: string, onDark: boolean): string {
  const hue = hashString(name) % 360
  return onDark ? `hsl(${hue}, 55%, 62%)` : `hsl(${hue}, 55%, 40%)`
}

/** Simple relative-luminance check on a `#rrggbb`/`rgb(...)` CSS color string — used only to
 * decide whether `chapterSwatchColor` should lean light or dark for contrast; doesn't need to be
 * colorimetrically precise, just consistent enough to pick the right side for each theme's own
 * (always solid, never gradient/transparent) `MENU_PALETTE` background color. */
function isDarkColor(css: string): boolean {
  const m = css.match(/\d+/g)
  if (!m || m.length < 3) return false
  const [r, g, b] = m.map(Number)
  return (0.299 * r + 0.587 * g + 0.114 * b) / 255 < 0.5
}

/** Positions a popup's `{top, left}` with its own **top-right corner** anchored to `anchor`'s
 * bottom-right corner (matching how a real button-triggered dropdown reads — "opens below and to
 * the same side as the button", same relationship `Library.tsx`'s own gear-icon `SettingsMenu`
 * uses) rather than at raw click coordinates: a popup anchored to wherever inside the button the
 * pointer happened to land reads as misaligned/floating, most visibly when the button sits near
 * the modal's own edge (confirmed live via a real screenshot — a menu triggered near the trash
 * icon rendered oddly offset instead of hanging directly off that icon's own corner).
 *
 * `width` must be the popup's own *real*, already-rendered width (`getBoundingClientRect().width`
 * — see `useAnchoredMenuPosition`'s own two-pass measure-then-reposition dance, the same "don't
 * know the real size until after layout" problem `Tooltip.tsx` already solves the same way) — an
 * earlier version of this function took a hardcoded width *estimate* instead, which silently
 * placed the menu's right edge wherever that guess said to rather than the button's own real
 * right edge, confirmed live via a real screenshot: a 180px estimate for an actually-97px-wide
 * menu left a visible ~80px gap between the menu and the button that triggered it.
 *
 * Also clamped so the result stays fully inside `#archivePagesOverlay` (this overlay's own modal
 * box, not just the browser viewport — also a real, live-confirmed bug: a menu near the modal's
 * own right edge rendered partly outside it). */
function anchorPopupToOverviewModal(anchor: DOMRect, width: number, height: number): { top: number; left: number } {
  const margin = 8
  const bounds = document.getElementById('archivePagesOverlay')?.getBoundingClientRect()
  const minLeft = (bounds?.left ?? 0) + margin
  const maxLeft = (bounds?.right ?? window.innerWidth) - width - margin
  const minTop = (bounds?.top ?? 0) + margin
  const maxTop = (bounds?.bottom ?? window.innerHeight) - height - margin
  return {
    left: Math.max(minLeft, Math.min(anchor.right - width, maxLeft)),
    top: Math.max(minTop, Math.min(anchor.bottom, maxTop)),
  }
}

/** Two-pass measure-then-reposition for a popup anchored via `anchorPopupToOverviewModal` — first
 * render parks the menu off-screen (so nothing flashes at a wrong position for a frame), a
 * `useLayoutEffect` then measures its own real rendered `getBoundingClientRect()` and recomputes
 * the real position from that, both applied before the browser actually paints. Shared by
 * `QuickAddTocPopover`/`ChapterActionMenu` rather than each hand-rolling the identical dance. */
function useAnchoredMenuPosition(anchor: DOMRect) {
  const [menuEl, setMenuEl] = useState<HTMLUListElement | null>(null)
  const [pos, setPos] = useState<{ top: number; left: number }>({ top: -9999, left: -9999 })

  useLayoutEffect(() => {
    if (!menuEl) return
    // This is exactly the "synchronize with an external system" case the underlying rule's own
    // description carves out as legitimate — measuring the menu's real, just-rendered DOM box
    // (`getBoundingClientRect()`, unknowable before this menu actually exists in the tree) to
    // reposition it, mirroring `Tooltip.tsx`'s own identical measure-then-reposition dance for the
    // identical reason (a real popup's true size is never known ahead of its own first render).
    const rect = menuEl.getBoundingClientRect()
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setPos(anchorPopupToOverviewModal(anchor, rect.width, rect.height))
    // `anchor` is a fresh `DOMRect` object every render (from `getBoundingClientRect()` at the
    // trigger's own click-time) — comparing it by reference would recompute every render for no
    // reason; comparing its own numeric fields is what actually reflects "the anchor moved".
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [menuEl, anchor.top, anchor.left, anchor.right, anchor.bottom])
  return { setMenuEl, pos }
}

/** The actual preset rows (封面/封底/目录/彩页 + 第N章 select) shared between `QuickAddTocPopover`
 * (wrapped in a `PopupMenu`, triggered from the page-grid icon's right-click) and
 * `PageLightbox` (rendered flat/inline instead of in a popup — see that component's own docs on
 * why: a lightbox already has room to show these permanently rather than behind another click). */
function QuickAddTocOptions({ onPick, asMenuItems }: { onPick: (title: string) => void; asMenuItems: boolean }) {
  const { t } = useTranslation()
  // `value`: what's actually stored (`onPick`'s argument). Table of Contents/Chapter N use the
  // reserved identifier (`toc`/`c1`-`c20`) so the backend can dedup by name (see
  // `lib/tocValidation.ts`'s own docs) — Cover/Back Cover/Color Pages have no such uniqueness
  // requirement (a user might legitimately want two "Color Pages" entries at different points in
  // a volume) and keep storing real display text exactly as before.
  const presets: { icon: string; title: string; value: string }[] = [
    { icon: 'fa-file-image', title: t('Cover') ?? 'Cover', value: t('Cover') ?? 'Cover' },
    { icon: 'fa-file-image', title: t('Back Cover') ?? 'Back Cover', value: t('Back Cover') ?? 'Back Cover' },
    { icon: 'fa-list', title: t('Table of Contents') ?? 'Table of Contents', value: TOC_IDENTIFIER_TABLE_OF_CONTENTS },
    { icon: 'fa-palette', title: t('Color Pages') ?? 'Color Pages', value: t('Color Pages') ?? 'Color Pages' },
    { icon: 'fa-gift', title: t('Omake') ?? 'Omake', value: t('Omake') ?? 'Omake' },
    { icon: 'fa-pen-nib', title: t('Afterword') ?? 'Afterword', value: t('Afterword') ?? 'Afterword' },
    { icon: 'fa-image', title: t('Illustration') ?? 'Illustration', value: t('Illustration') ?? 'Illustration' },
  ]
  const Row = asMenuItems ? PopupMenuItem : 'div'
  const chapterSelect = (
    <select
      className="favtag-btn"
      defaultValue=""
      style={{ marginLeft: 4 }}
      onClick={(e) => e.stopPropagation()}
      onChange={(e) => {
        if (!e.target.value) return
        onPick(tocChapterIdentifier(Number(e.target.value)))
        e.target.value = ''
      }}
    >
      <option value="" disabled>
        {t('Chapter…')}
      </option>
      {Array.from({ length: TOC_CHAPTER_COUNT }, (_, i) => i + 1).map((n) => (
        <option key={n} value={n}>
          {t('Chapter {{n}}', { n })}
        </option>
      ))}
    </select>
  )
  return (
    <>
      {presets.map(({ icon, title, value }) => (
        <Row
          key={title}
          onClick={(e: MouseEvent) => {
            // Without this, the click bubbles up to whatever's underneath (the page-grid cell's
            // own `onClick={() => onSelectPage(page)}` in the popover case) — the chapter got
            // added correctly, but something else also fired as an unwanted side effect.
            e.stopPropagation()
            onPick(value)
          }}
          {...(asMenuItems ? {} : { style: { cursor: 'pointer', padding: '4px 8px', display: 'flex', alignItems: 'center' } })}
        >
          <i className={`fa ${icon}`} style={{ width: 18 }}></i> {title}
        </Row>
      ))}
      {/* `display: flex; alignItems: center` in flat mode — without it, the `<select>`'s own
          default vertical metrics (a form control, not inline text) sit slightly off from the
          preset rows' plain icon+text baseline alignment above, a real visible mismatch confirmed
          on screenshot even though both rows use the same padding. */}
      <Row {...(asMenuItems ? { style: { cursor: 'default' } } : { style: { padding: '4px 8px', display: 'flex', alignItems: 'center' } })}>
        <i className="fa fa-book-medical" style={{ width: 18 }}></i>
        {chapterSelect}
      </Row>
    </>
  )
}

/** Right-click menu on the "add chapter" icon (`PageGridActionIcon`'s `fa-book-medical`) — a
 * purely additive shortcut, no legacy equivalent, for the handful of chapter titles common enough
 * in real doujin/manga scans to not need typing out via the plain left-click `promptDialog` flow
 * every time. Every option submits immediately on pick (no separate confirm step) — the point is
 * speed for a title that's already fully decided the moment it's clicked/selected, not a form. */
function QuickAddTocPopover({
  anchor,
  onPick,
  onClose,
}: {
  anchor: DOMRect
  onPick: (title: string) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { setMenuEl, pos } = useAnchoredMenuPosition(anchor)
  return (
    <>
      {/* `stopPropagation` — this backdrop's own click-to-close would otherwise bubble up to
          `#overlay-shade` (the outer Archive Overview modal's own click-to-close backdrop,
          covering the same full viewport) and close *that* too, since neither backdrop is a DOM
          ancestor/descendant of the other that a plain click could be scoped to (confirmed live:
          clicking outside this popover closed the whole overview modal along with it). */}
      <div
        style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
        }}
      />
      <PopupMenu
        ref={setMenuEl}
        style={{ position: 'fixed', top: pos.top, left: pos.left, zIndex: Z_OVERLAY_CONTENT }}
        mainLabel={{ icon: 'fa-bolt', text: t('Quick Add Chapter') ?? 'Quick Add Chapter' }}
      >
        <QuickAddTocOptions
          asMenuItems
          onPick={(title) => {
            onPick(title)
            onClose()
          }}
        />
      </PopupMenu>
    </>
  )
}

/** Delete-chapter menu — legacy's own `.remove-toc` (`reader.js`) only ever deletes whichever
 * chapter the reader currently happens to be scrolled into (`getCurrentChapter()`), with no way
 * to target a different one at all (see `handleRemoveToc`'s own docs for the real-source
 * confirmation). This lists every chapter in the archive (matching the Upload page's own
 * `ConflictMenu`/`RenamePopover` popup-menu visual pattern), so picking one to delete doesn't
 * require first navigating to it. Clicking an entry still goes through the same themed
 * `confirmDialog` as before (see `handleRemoveToc`) — this menu only changes *which* chapter that
 * confirmation is about, not whether one still happens. */
/** Shared by the edit and delete chapter icons — both need the exact same "list every chapter in
 * the archive, pick one" dance (legacy's own `.edit-toc`/`.remove-toc` only ever operate on
 * `getCurrentChapter()`, whichever chapter the reader/lightbox happens to be scrolled/previewing
 * into right now, with no way to target a different one at all — confirmed against
 * `reader.js:157-158,1702`, a real limitation in legacy itself, not a porting gap). Rather than
 * two near-identical popup-menu components, `mode` switches the icon/header text/`zIndex` tier
 * between them; `onPick`'s meaning is mode-dependent (edit: open the rename prompt for that
 * entry; delete: remove it) but the menu itself doesn't need to know which.
 *
 * `zIndexBase` lets a caller opt into `Z_OVERLAY_ABOVE_LEGACY_MODAL`'s tier instead of the
 * generic `Z_OVERLAY_BACKDROP`/`Z_OVERLAY_CONTENT` one — needed when this menu is triggered from
 * *inside* `PageLightbox` (already floating at that higher tier itself), otherwise the menu would
 * render invisible behind the lightbox's own backdrop, the same stacking bug `PageLightbox`'s own
 * docs describe for `Z_OVERLAY_ABOVE_LEGACY_MODAL`'s original introduction. */
function ChapterActionMenu({
  mode,
  anchor,
  chapters,
  zIndexBase = Z_OVERLAY_BACKDROP,
  onPick,
  onClose,
}: {
  mode: 'edit' | 'delete'
  anchor: DOMRect
  chapters: { page: number; name: string }[]
  zIndexBase?: number
  onPick: (entry: { page: number; name: string }) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const { setMenuEl, pos } = useAnchoredMenuPosition(anchor)
  const icon = mode === 'edit' ? 'fa-pencil-alt' : 'fa-trash-alt'
  const label = mode === 'edit' ? (t('Edit Chapter name') ?? 'Edit Chapter name') : (t('Delete Chapter') ?? 'Delete Chapter')
  // Tracks this menu's own open/closed lifetime in `openChapterActionMenuCount` (see that
  // constant's own docs for why a plain module-level counter, and the full registration-order
  // bug this exists to sidestep) — incremented on mount, decremented on unmount, so any
  // capture-phase `Escape` listener elsewhere (namely `PageLightbox`'s own) can check "is a menu
  // like this currently open?" before deciding whether it's safe to act.
  useEffect(() => {
    openChapterActionMenuCount++
    return () => {
      openChapterActionMenuCount--
    }
  }, [])
  // Registered on the capture phase specifically, mirroring `PageLightbox`'s own Escape handler
  // (see that component's docs for the full reasoning) — this menu can be triggered from *inside*
  // the lightbox, which has its own capture-phase Escape listener. `stopImmediatePropagation`,
  // not just `stopPropagation`, for the same same-target capture-vs-bubble reason documented
  // there (this alone doesn't fully solve the ordering problem — see `openChapterActionMenuCount`
  // for the other half of the real fix, on `PageLightbox`'s own side).
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== 'Escape') return
      e.stopImmediatePropagation()
      onClose()
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [onClose])
  return (
    <>
      <div
        style={{ position: 'fixed', inset: 0, zIndex: zIndexBase }}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
        }}
      />
      <PopupMenu
        ref={setMenuEl}
        style={{ position: 'fixed', top: pos.top, left: pos.left, zIndex: zIndexBase + 1, maxHeight: 260, overflowY: 'auto' }}
        mainLabel={{ icon, text: label }}
      >
        {chapters.map((entry) => {
          // Reserved preset identifiers (`c1`-`c20`/`toc`) aren't editable through this manual
          // free-text flow — their whole point is that the *stored* value and the *displayed*
          // text are deliberately different (see `lib/tocValidation.ts`'s own docs), so "editing"
          // one here would silently convert it into an unrelated free-text entry rather than
          // actually renaming the preset. Changing one of these is done by deleting it and
          // re-applying a (possibly different) preset instead, not by editing it in place.
          const isPreset = mode === 'edit' && isReservedTocIdentifier(entry.name)
          return (
            <PopupMenuItem
              key={entry.page}
              disabled={isPreset}
              onClick={(e) => {
                e.stopPropagation()
                onPick(entry)
                onClose()
              }}
            >
              <i className={`fas ${icon}`} style={{ width: 18 }}></i> {displayTocName(entry.name, t)}
            </PopupMenuItem>
          )
        })}
      </PopupMenu>
    </>
  )
}

// Mirrors legacy's `#archivePagesOverlay` (`updateArchiveOverlay`/`generateThumbnails` in
// `~/LANraragi/public/js/reader.js`) — thumbnail (left) + Admin Options/Categories/Rating (right)
// side by side via `.reader-thumbnail`'s `display:inline-block` (verified against
// `~/LANraragi/public/css/lrr.css`), the full per-namespace tags table below it, then a thumbnail
// grid scoped to the current chapter (or the whole archive if there's no TOC).
export default function ArchiveOverviewOverlay({
  archive,
  categories,
  loggedIn,
  currentPage,
  onClose,
  onSelectPage,
  autoFocus = false,
}: {
  archive: ArchiveMetadata
  categories: CategoryMetadata[] | undefined
  loggedIn: boolean
  currentPage: number
  onClose: () => void
  onSelectPage: (page: number) => void
  /** Scroll/briefly-highlight the current page's own thumbnail right after this overlay mounts
   * (see the effect below) — only meaningful for a real user click on the grid-toggle button;
   * `false` (the default) when `Reader.tsx`'s own `showOverlayByDefault` setting is what opened
   * this overlay instead, so a fresh page load doesn't also yank the scroll position on top of
   * auto-opening. */
  autoFocus?: boolean
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const createCategory = useCreateCategory()
  const staticCategories = (categories ?? []).filter((c) => !c.search)
  const archiveCategories = staticCategories.filter((c) => c.archives.includes(archive.arcid))

  // Legacy's `#filter-stamped` (`reader.js`'s `checkStampedPages`/`filterStampedOverlay`) — marks
  // each thumbnail `data-stamped=true` if `GET /archives/{id}/stamps` includes its page number,
  // then a toggle hides every non-stamped thumbnail so the grid becomes a stamped-pages-only view.
  const stampedPages = useStampedPages(archive.arcid)
  const stampedPageSet = new Set(stampedPages.data?.result ?? [])
  const [filterStamped, setFilterStamped] = useState(false)
  const [removeTocMenuAt, setRemoveTocMenuAt] = useState<DOMRect | null>(null)
  const [editTocMenuAt, setEditTocMenuAt] = useState<DOMRect | null>(null)
  const [lightboxPage, setLightboxPage] = useState<number | null>(null)

  const chapters = archive.toc.length > 0 ? archive.toc : null

  // Legacy's `getCurrentChapter` (`reader.js`) — the last ToC entry whose `startPage` is `<=` the
  // reader's current page; only leaf chapters (this port has no sub-chapter nesting) get
  // edit/delete icons (legacy: `currentChapter.chapters === null`).
  const currentChapter = chapters
    ? [...chapters].filter((c) => c.page <= currentPage).sort((a, b) => b.page - a.page)[0]
    : undefined

  const setThumbnail = useSetArchiveThumbnail(archive.arcid)
  const addTocEntry = useAddTocEntry(archive.arcid)
  const removeTocEntry = useRemoveTocEntry(archive.arcid)

  // `useSetArchiveThumbnail`'s own `onSuccess` invalidates the *metadata* query, but the cover
  // `<img>` below points at a plain, param-free `/api/archives/{id}/thumbnail` URL — a browser
  // caches an image response by URL alone, so a same-URL re-render after a successful "set as
  // thumbnail" click kept serving the old cached bytes instead of the just-regenerated ones (only
  // a full page reload, which bypasses the image cache incidentally rather than by design, ever
  // showed the update). Bumped on success and appended as a cache-busting query param below.
  // Legacy itself has no equivalent fix — its own `.set-thumbnail` handler (`reader.js`) never
  // re-fetches the cover `<img>` at all after a successful PUT, so the same staleness exists
  // there too (confirmed by reading that handler's full body — it only ever calls `Server.callAPI`
  // and shows a toast, nothing image-related) — this is a straightforward improvement, not a port
  // of some real legacy mechanism.
  const [thumbnailVersion, setThumbnailVersion] = useState(0)

  // Legacy's `.set-thumbnail` click handler (`reader.js`) — regenerates the cover thumbnail from
  // this page and shows a toast; `e.stopPropagation()` so the click doesn't also trigger the
  // thumbnail's own `onSelectPage` navigation.
  function handleSetThumbnail(e: MouseEvent, page: number) {
    e.preventDefault()
    e.stopPropagation()
    setThumbnail.mutate(page, {
      onSuccess: () => {
        setThumbnailVersion((v) => v + 1)
        toast({ text: t('Successfully set page {{n}} as the thumbnail!', { n: page }) ?? undefined })
      },
      onError: () => toast({ text: t('Error updating thumbnail') ?? undefined, icon: 'error' }),
    })
  }

  // Shared by `handleAddToc`/`handleEditToc`'s manual-entry flow — loops the same prompt (with an
  // error prefixed onto the message) until the user either cancels or enters something that isn't
  // a bare internal-identifier-style string (`c1`-`c15`/`toc`, case-insensitive). These are never
  // actually stored anywhere (the backend only ever sees real display text), but a user typing the
  // identifier itself rather than real chapter text is almost certainly confused about what the
  // field expects, not intentionally naming a chapter "c1" — real display text like "目录"/"第 1
  // 章" is left completely unrestricted, matching legacy's own free-text ToC title storage.
  async function promptTocTitle(defaultValue = ''): Promise<string | null> {
    let message = t('Enter a title for this chapter/section:') ?? ''
    let value = defaultValue
    for (;;) {
      const input = await promptDialog(message, value)
      if (input === null) return null
      const trimmed = input.trim()
      if (trimmed === '') return null
      if (isReservedTocIdentifier(trimmed)) {
        message = t('"{{value}}" is a reserved identifier and can\'t be used as a chapter title. Enter a title for this chapter/section:', { value: trimmed }) ?? ''
        value = trimmed
        continue
      }
      return trimmed
    }
  }

  // Legacy's `.add-toc` click handler + `addTocSection` (`reader.js`) — prompts for a chapter
  // title, then PUTs the new ToC entry. Empty/cancelled input adds nothing (matches legacy's own
  // `result.value.trim() !== ""` guard).
  async function handleAddToc(e: MouseEvent, page: number) {
    e.preventDefault()
    e.stopPropagation()
    const title = await promptTocTitle()
    if (title) {
      addTocEntry.mutate(
        { page, title },
        { onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }) },
      )
    }
  }

  // Right-click on the same "add chapter" icon (see `handleAddToc` above for its plain left-click
  // prompt-based flow) — a purely additive shortcut (no legacy equivalent) for the handful of
  // chapter titles that come up often enough in real doujin/manga scans to not need typing out
  // every time: 封面/封底/目录/彩页, plus 第N章 (1–15) via a `<select>`. Submits immediately on
  // pick — no separate confirm step, matching this popover's own single-click-and-done feel
  // rather than a form the user has to explicitly submit.
  function handleQuickAddToc(page: number, title: string) {
    addTocEntry.mutate(
      { page, title },
      { onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }) },
    )
  }

  // Legacy's `.edit-toc` click handler (`reader.js`: `addTocSection(currentChapter.startPage,
  // currentChapter.name)`) — re-prompts with the existing name pre-filled as a placeholder, then
  // re-adds the entry at the same page (the host's `add_toc_entry` replaces same-page entries
  // rather than duplicating them, matching legacy's own upsert-by-page semantics).
  //
  // Takes an explicit `entry` (defaulting to `currentChapter`, matching legacy's own real
  // limitation of only ever editing whichever chapter the reader is currently scrolled into) —
  // the `ChapterActionMenu`-driven callers below pass whichever entry was actually picked from
  // the "edit chapter" menu instead, the same real improvement over legacy's single-target
  // restriction `handleRemoveToc`'s own docs describe for delete.
  //
  // Refuses to edit a reserved preset identifier (`c1`-`c20`/`toc`) at all — `ChapterActionMenu`
  // already disables picking one of these in its own list (see that component's own docs), but
  // this plain left-click path (which always targets `currentChapter`, bypassing that menu
  // entirely) needs the same guard independently. The whole point of storing an identifier
  // instead of real text is that the two are deliberately different — silently "editing" one into
  // free text here would erase that distinction without the user necessarily intending to change
  // what kind of entry it is, not just its wording. Changing one of these is done by deleting it
  // and re-applying a (possibly different) preset instead.
  async function handleEditToc(entry = currentChapter) {
    if (!entry || isReservedTocIdentifier(entry.name)) return
    const title = await promptTocTitle(displayTocName(entry.name, t))
    if (title) {
      addTocEntry.mutate(
        { page: entry.page, title },
        { onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }) },
      )
    }
  }

  // Legacy's own `.remove-toc` (`reader.js`'s `removeTocSection`) only ever targets
  // `getCurrentChapter()` — whichever chapter the reader happens to be scrolled into right now —
  // with no way to pick a different one at all; a real, confirmed limitation in legacy itself,
  // not a porting gap (verified against `reader.js:157-158,1702` — `getCurrentChapter` really is
  // the only chapter `.edit-toc`/`.remove-toc` ever operate on). A real improvement over that: the
  // delete button now opens `ChapterActionMenu` listing every chapter in the archive, so deleting one
  // that isn't the currently-viewed one doesn't require first scrolling/navigating to it.
  async function handleRemoveToc(entry: { page: number; name: string }) {
    if (!(await confirmDialog(t('Are you sure you want to delete "{{name}}"?', { name: entry.name }) ?? ''))) return
    removeTocEntry.mutate(entry.page, {
      onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }),
    })
  }

  async function addToCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: 'PUT' })
    await queryClient.invalidateQueries({ queryKey: ['categories'] })
  }

  async function handleNewCategory() {
    const result = await newCategoryDialog()
    if (result === null) return
    try {
      const data = await createCategory.mutateAsync(result)
      if (!result.isDynamic) await addToCategory(data.category_id)
    } catch {
      toast({ heading: t('Error modifying category') ?? undefined, icon: 'error' })
    }
  }

  async function removeFromCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: 'DELETE' })
    await queryClient.invalidateQueries({ queryKey: ['categories'] })
  }

  async function deleteArchive() {
    if (
      !(await confirmDialog(
        t('This will delete both metadata and matching files from your system! Please use with caution.') ?? '',
      ))
    ) {
      return
    }
    await fetch(`/api/archives/${archive.arcid}`, { method: 'DELETE' })
    navigate(routes.library())
  }

  const pageCount = archive.pagecount

  // Scrolls to and briefly outlines the current page's own thumbnail once, right after the
  // overlay opens from a real click (`autoFocus`, see this component's own prop docs) — otherwise
  // the reader has to hunt for it by eye across a grid that can run into the hundreds of cells for
  // a long archive, with no indication at all of where "here" is. Additive; legacy's own
  // `#archivePagesOverlay` has no equivalent (it opens already scrolled to the top, same as this
  // port without this effect). Skipped entirely when `showOverlayByDefault` auto-opened this
  // overlay instead (`autoFocus` false) — auto-scrolling on every single page load in addition to
  // auto-opening was a real, reported annoyance, even though auto-opening itself is intentional.
  const [highlightedPage, setHighlightedPage] = useState<number | null>(null)
  useEffect(() => {
    if (!autoFocus) return
    // Deferred a tick rather than calling `setHighlightedPage` synchronously in the effect body
    // (the project's own lint rules flag that as cascading-render-prone) — also conveniently lets
    // the just-mounted grid finish its first paint before `scrollIntoView` runs against it.
    const startTimer = setTimeout(() => {
      const cell = document.querySelector(`[data-page-cell="${currentPage}"]`)
      if (!cell) return
      cell.scrollIntoView({ block: 'center' })
      setHighlightedPage(currentPage)
    }, 0)
    const clearTimer = setTimeout(() => setHighlightedPage(null), 3000)
    return () => {
      clearTimeout(startTimer)
      clearTimeout(clearTimer)
    }
    // Intentionally empty deps — this is a one-time "where am I" cue for whichever page the
    // overlay opened on, not something that should re-trigger on every `currentPage` change while
    // it stays open (e.g. from clicking around the grid itself).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <>
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — legacy's own JS explicitly shows it
          (`fadeTo`) when opening an overlay rather than relying on presence in the DOM, so this
          needs the same explicit override or clicking it (or even seeing it) does nothing. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div id="overlay-shade" style={{ display: 'block', opacity: 0.6 }} onClick={onClose} />
      <div id="archivePagesOverlay" className="id1 base-overlay page-overlay">
        <h2 className="ih" style={{ textAlign: 'center' }}>
          {t('Archive Overview')}
        </h2>

        <div id="tagContainer" className="caption caption-tags caption-reader">
          <br />
          <div style={{ marginBottom: 16 }}>
            {/* Legacy's own `.id3 img { max-height: 275px }` alone doesn't keep this narrow — a
                landscape-oriented cover (a raw panel image rather than a proper portrait cover,
                confirmed via a real archive that reproduces this) has plenty of headroom under
                that height cap to still render very wide, pushing Admin Options below instead of
                beside it. Legacy avoids this because `#archivePagesOverlay` itself carries `.id1`
                (`width: 228px`), which `.id3.nocrop img { max-width: 95% }` computes against —
                this port's own `#tagContainer` (`.caption-reader { min-width: 50% }`) has no such
                fixed width to inherit from, so the same 95%-of-ancestor rule alone doesn't
                reliably leave room for Admin Options beside it. 200px lands close to legacy's own
                effective ~217px (95% of 228px) without depending on an ancestor width this port
                doesn't have. */}
            <div className="id3 nocrop reader-thumbnail" style={{ maxWidth: 200 }}>
              <img
                alt=""
                src={`/api/archives/${archive.arcid}/thumbnail${thumbnailVersion > 0 ? `?v=${thumbnailVersion}` : ''}`}
                style={{ maxWidth: '100%' }}
              />
            </div>

            {loggedIn && (
              <div style={{ display: 'inline-block', verticalAlign: 'middle' }}>
                <h2>{t('Admin Options')}</h2>

                <input
                  className="stdbtn"
                  type="button"
                  value={t('Edit Archive Metadata') ?? undefined}
                  onClick={() => navigate(routes.edit(archive.arcid))}
                />
                <input
                  className="stdbtn"
                  type="button"
                  value={t('Delete Archive') ?? undefined}
                  onClick={() => void deleteArchive()}
                />
                <br />

                <h2>{t('Categories')}</h2>
                <div style={{ display: 'inline-block' }}>
                  {archiveCategories.map((c) => (
                    <div key={c.id} className="gt" style={{ fontSize: 14, padding: 4 }}>
                      <span className="label">{c.name}</span>
                      <a
                        href="#"
                        style={{ marginLeft: 4, marginRight: 2 }}
                        onClick={(e) => {
                          e.preventDefault()
                          void removeFromCategory(c.id)
                        }}
                      >
                        ×
                      </a>
                    </div>
                  ))}
                </div>

                <br />
                <span>{t('Add to : ')}</span>
                <select
                  id="category"
                  className="favtag-btn"
                  style={{ width: 200 }}
                  value=""
                  onChange={(e) => {
                    if (e.target.value) void addToCategory(e.target.value)
                  }}
                >
                  <option value="">{t(' -- No Category -- ')}</option>
                  {staticCategories.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
                <Tooltip label={t('New Category') ?? undefined}>
                  <a
                    href="#"
                    style={{ marginLeft: 6 }}
                    onClick={(e) => {
                      e.preventDefault()
                      void handleNewCategory()
                    }}
                  >
                    <i className="fas fa-plus" />
                  </a>
                </Tooltip>

                <h2>{t('Rating')}</h2>
                <RatingWidget archiveId={archive.arcid} tags={archive.tags} />
              </div>
            )}
          </div>

          <TagsTable tags={archive.tags} />
        </div>

        <br />
        <br />

        <div className="overlay-bar">
          <div className="overlay-bar-left">
            {stampedPageSet.size > 0 && (
              <a
                className={`fas fa-stamp${filterStamped ? ' toggled' : ''}`}
                id="filter-stamped"
                href="#"
                style={{ padding: 8, fontSize: 14 }}
                title={t('Filter stamped pages') ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  setFilterStamped((v) => !v)
                }}
              />
            )}
          </div>
          <h2 className="ih">{chapters ? t('Chapters') : t('Pages')}</h2>
          <div className="chapter-selector">
            {chapters && (
              <select
                id="chapter-select"
                className="favtag-btn"
                style={{ width: 200 }}
                onChange={(e) => {
                  const page = Number(e.target.value)
                  if (page > 0) onSelectPage(page)
                }}
              >
                {chapters.map((c) => (
                  <option key={c.page} value={c.page}>
                    {displayTocName(c.name, t)}
                  </option>
                ))}
              </select>
            )}
            {loggedIn && chapters && currentChapter && (
              <>
                {/* Left-click opens the same "pick any chapter" menu the delete icon already
                    has, rather than legacy's own left-click-edits-`currentChapter`-directly
                    shortcut — the plain-click path silently no-ops on a reserved preset
                    identifier now (see `handleEditToc`'s own guard), which reads as "broken" with
                    no explanation if it's still the default click behavior instead of routing
                    through the menu, where that same entry shows up visibly disabled instead. */}
                <a
                  className="fas fa-pencil-alt edit-toc"
                  href="#"
                  style={{ padding: 8, fontSize: 14 }}
                  title={t('Edit Chapter name') ?? undefined}
                  onClick={(e) => {
                    e.preventDefault()
                    setEditTocMenuAt(e.currentTarget.getBoundingClientRect())
                  }}
                />
                {editTocMenuAt && (
                  <ChapterActionMenu
                    mode="edit"
                    anchor={editTocMenuAt}
                    chapters={chapters}
                    onPick={(entry) => void handleEditToc(entry)}
                    onClose={() => setEditTocMenuAt(null)}
                  />
                )}
                <a
                  className="fas fa-trash-alt remove-toc"
                  href="#"
                  style={{ padding: 8, fontSize: 14 }}
                  title={t('Delete Chapter') ?? undefined}
                  onClick={(e) => {
                    e.preventDefault()
                    setRemoveTocMenuAt(e.currentTarget.getBoundingClientRect())
                  }}
                />
                {removeTocMenuAt && (
                  <ChapterActionMenu
                    mode="delete"
                    anchor={removeTocMenuAt}
                    chapters={chapters}
                    onPick={(entry) => void handleRemoveToc(entry)}
                    onClose={() => setRemoveTocMenuAt(null)}
                  />
                )}
              </>
            )}
          </div>
        </div>

        <div id="pages-section" style={{ textAlign: 'center' }}>
          {Array.from({ length: pageCount }, (_, i) => i + 1).map((page) => {
            const isStamped = stampedPageSet.has(String(page))
            if (filterStamped && !isStamped) return null
            return (
              <PageGridCell
                key={page}
                page={page}
                isStamped={isStamped}
                loggedIn={loggedIn}
                highlighted={page === highlightedPage}
                thumbnailSrc={`/api/archives/${archive.arcid}/thumbnail?page=${page}`}
                onSelectPage={onSelectPage}
                onSetThumbnail={handleSetThumbnail}
                onAddToc={handleAddToc}
                onQuickAddToc={handleQuickAddToc}
                onOpenLightbox={setLightboxPage}
              />
            )
          })}
        </div>
      </div>
      {lightboxPage !== null && (
        <PageLightbox
          archiveId={archive.arcid}
          initialPage={lightboxPage}
          toc={archive.toc}
          loggedIn={loggedIn}
          onQuickAddToc={handleQuickAddToc}
          onEditToc={(entry) => void handleEditToc(entry)}
          onRemoveToc={(entry) => void handleRemoveToc(entry)}
          onClose={() => setLightboxPage(null)}
        />
      )}
    </>
  )
}

/** One frame in `PageLightbox`'s own bottom filmstrip gallery — a small thumbnail (reuses the
 * same cheap `/thumbnail?page=N` endpoint the page grid itself already uses, not the full-size
 * page image; a gallery of a few hundred full-size images loading at once would be far heavier
 * for no visual benefit at this size) that becomes the large preview on hover, without touching
 * the reader's own reading progress or the overview grid's own scroll position (both entirely
 * unaffected — this lightbox is its own, separate, throwaway "preview" state layered on top). */
function LightboxFilmstripFrame({
  archiveId,
  page,
  isPreview,
  accentColor,
  borderColor,
  chapter,
  onHover,
  onClick,
}: {
  archiveId: string
  page: number
  isPreview: boolean
  accentColor: string
  borderColor: string
  /** The chapter this page belongs to, if any — rendered as a small colored swatch + truncated
   * title strip beneath the thumbnail itself (see `chapterForPage`/`chapterSwatchColor`), so a
   * chapter's extent is visible at a glance while scrubbing the filmstrip instead of only showing
   * up once hovered into the large preview above. `label` is pre-formatted by the caller (plain
   * chapter name on the chapter's own start frame, "name (ordinal)" — e.g. "第 1 章 (5)" — on
   * later frames, an incrementing per-frame count rather than one fixed total repeated on every
   * frame, so scrubbing through a chapter shows *how far into it* each specific frame is). */
  chapter?: { label: string; swatch: string }
  onHover: () => void
  onClick: () => void
}) {
  return (
    <div data-filmstrip-page={page} style={{ flex: '0 0 auto', width: 90, display: 'flex', flexDirection: 'column', gap: 2 }}>
      <div
        onMouseEnter={onHover}
        onClick={onClick}
        style={{
          position: 'relative',
          width: 90,
          height: 120,
          cursor: 'pointer',
          outline: isPreview ? `3px solid ${accentColor}` : `1px solid ${borderColor}`,
          outlineOffset: -1,
        }}
      >
        <img
          src={`/api/archives/${archiveId}/thumbnail?page=${page}`}
          alt={`${page}`}
          loading="lazy"
          style={{ width: '100%', height: '100%', objectFit: 'cover' }}
        />
      </div>
      {chapter && (
        <div style={{ display: 'flex', alignItems: 'center', gap: 3, minWidth: 0 }} title={chapter.label}>
          <span style={{ flex: '0 0 auto', width: 8, height: 8, borderRadius: '50%', background: chapter.swatch }} />
          <span style={{ flex: '1 1 auto', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 10 }}>
            {chapter.label}
          </span>
        </div>
      )}
    </div>
  )
}

/** Continuous-scroll hot zone at one edge of the filmstrip — hovering it scrolls the filmstrip
 * toward that edge for as long as the pointer stays over it (not a single fixed-distance nudge
 * per click/hover), matching a real "hover the edge, it just keeps going" gallery scrubber. Visible
 * chevron buttons (rather than an invisible hover margin) so the affordance is discoverable at a
 * glance, matching the Library carousel's own real `.carousel-prev`/`.carousel-next` chevrons. */
function LightboxFilmstripEdge({ direction, onScroll }: { direction: 'left' | 'right'; onScroll: (delta: number) => void }) {
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)
  function start() {
    stop()
    intervalRef.current = setInterval(() => onScroll(direction === 'left' ? -16 : 16), 16)
  }
  function stop() {
    if (intervalRef.current !== null) clearInterval(intervalRef.current)
    intervalRef.current = null
  }
  useEffect(() => stop, [])
  return (
    <div
      onMouseEnter={start}
      onMouseLeave={stop}
      style={{
        position: 'absolute',
        top: 0,
        bottom: 0,
        [direction]: 0,
        width: 48,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        cursor: 'pointer',
        zIndex: 1,
        // Deliberately theme-independent (unlike the rest of the lightbox, which follows
        // `useMenuPalette()`) — this is a dark photo-viewer-style scrim overlaid *on top of* the
        // filmstrip thumbnails themselves, not a themed content surface, so a fixed dark
        // gradient + white icon (reliable contrast against any thumbnail's own colors) makes more
        // sense here than chasing the active theme's own (possibly light) palette.
        background: `linear-gradient(to ${direction}, transparent, rgba(0,0,0,0.6))`,
      }}
    >
      <i className={`fa fa-3x fa-chevron-${direction}`} style={{ color: 'white' }} aria-hidden="true"></i>
    </div>
  )
}

/** Full-page-image preview, opened via the magnifying-glass icon on a page-grid cell — the small
 * grid thumbnails (~100px tall) are too small to actually judge a page's content by eye, which
 * makes deciding exactly where a chapter starts/ends (the whole point of the quick-add-chapter
 * feature) guesswork at that size. Layered *on top of* the Archive Overview modal (which stays
 * open underneath, unlike a normal "close the old one first" modal stack) rather than replacing
 * it, so closing the lightbox returns to exactly where the grid was.
 *
 * The bottom filmstrip's own hover-to-preview is intentionally decoupled from both the reader's
 * real reading progress (`useUpdateProgress` is never called here) and the overview grid's own
 * scroll position underneath (this component owns its own `previewPage` state, entirely separate
 * from `currentPage`/`highlightedPage`) — this is a scratch "look around" tool, not a navigation
 * action, and shouldn't have any lasting side effect just from hovering around in it. */
function PageLightbox({
  archiveId,
  initialPage,
  toc,
  loggedIn,
  onQuickAddToc,
  onEditToc,
  onRemoveToc,
  onClose,
}: {
  archiveId: string
  initialPage: number
  toc: { page: number; name: string }[]
  loggedIn: boolean
  onQuickAddToc: (page: number, title: string) => void
  onEditToc: (entry: { page: number; name: string }) => void
  onRemoveToc: (entry: { page: number; name: string }) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const palette = useMenuPalette()
  const pages = useArchivePages(archiveId)
  const totalPages = pages.data?.pages.length ?? 0
  const [previewPage, setPreviewPage] = useState(initialPage)
  // Same `ChapterActionMenu` the overview's own chapter-selector row uses (see that JSX further
  // up this file) — duplicated here as a second, independent trigger next to the lightbox's own
  // chapter `<select>`, so editing/deleting a chapter doesn't require closing the lightbox first.
  const [editTocMenuAt, setEditTocMenuAt] = useState<DOMRect | null>(null)
  const [removeTocMenuAt, setRemoveTocMenuAt] = useState<DOMRect | null>(null)
  // Keyed by the page it was measured for, rather than reset via a separate effect whenever
  // `previewPage` changes — `img.onLoad`'s own measurement is naturally already scoped to
  // whichever page's `<img>` just loaded, so comparing `measured.page === previewPage` below is
  // enough to tell a stale previous-page measurement from a current one, with no extra effect
  // needed just to null out state on every page change.
  const [measured, setMeasured] = useState<{ page: number; width: number; height: number; sizeKb: number | null } | null>(null)
  const filmstripRef = useRef<HTMLDivElement>(null)
  // Set right before an arrow-key-driven `setPreviewPage` call, read (and cleared) by the
  // scroll-into-view effect below — hover-driven page changes deliberately do NOT scroll the
  // filmstrip (the user's own scroll position while scrubbing shouldn't be fought), but keyboard
  // navigation should keep the active frame visible since there's no other way to see where you
  // landed once it scrolls out of the visible strip.
  const scrollFilmstripOnNextPage = useRef(false)
  // Set for the duration of a keyboard-triggered `scrollIntoView` below (and briefly after) —
  // see the filmstrip frames' own `onHover` docs for why: scrolling the strip out from under a
  // stationary mouse cursor fires a real `mouseenter` on whichever frame the pointer ends up
  // over, which would otherwise immediately override the arrow key's own page change. Starts
  // `true` (not `false`) for the same reason, generalized to the moment the lightbox itself opens
  // — the cursor is very likely already sitting somewhere over the lightbox (right where the
  // magnifying-glass icon that opened it was clicked), and if that happens to land over a
  // filmstrip frame once the mount-time scroll-to-`initialPage` effect below runs, the resulting
  // `mouseenter` would immediately clobber `initialPage` with whatever page the mouse happens to
  // be resting over — cleared 1s after mount by the effect right below this ref.
  const suppressHoverRef = useRef(true)

  const pageUrl = pages.data?.pages[previewPage - 1]
  const dimensions = measured?.page === previewPage ? measured : null

  useEffect(() => {
    const timer = setTimeout(() => {
      suppressHoverRef.current = false
    }, 1000)
    return () => clearTimeout(timer)
  }, [])

  useEffect(() => {
    if (!scrollFilmstripOnNextPage.current) return
    scrollFilmstripOnNextPage.current = false
    suppressHoverRef.current = true
    filmstripRef.current
      ?.querySelector(`[data-filmstrip-page="${previewPage}"]`)
      ?.scrollIntoView({ block: 'nearest', inline: 'nearest' })
    // `scrollIntoView` with `behavior: 'auto'` (the default, and what this container's own
    // `scrollBehavior: 'auto'` style also specifies) completes synchronously before the next
    // paint, but the resulting `mouseenter` is dispatched by the browser on its own event loop
    // turn slightly after — a short timeout (rather than 0) reliably outlasts that, confirmed
    // live: a 0ms timeout still occasionally let the stray `mouseenter` through.
    const timer = setTimeout(() => {
      suppressHoverRef.current = false
    }, 100)
    return () => clearTimeout(timer)
  }, [previewPage])

  // Scrolls the filmstrip to the page the lightbox was opened on — without this, a magnifying-
  // glass click far down the page grid (or on any page other than whichever one the filmstrip
  // happens to already be scrolled to, which starts at its natural left edge) opened the lightbox
  // with the *active* frame off-screen in the strip below, with no indication of where it was
  // (a real, live-reported gap: clicking the icon on page 71 opened the large preview correctly,
  // but the filmstrip stayed scrolled to its start instead of showing page 71's own frame).
  // Waits on `pages.data` since the frames themselves don't exist in the DOM to scroll to until
  // that query resolves.
  //
  // Computed manually via `offsetLeft`/`clientWidth` rather than `scrollIntoView({ inline:
  // 'center' })` — the native call was confirmed live to *not* reliably center (or sometimes not
  // move the strip at all) inside this nested `position: fixed` modal's own `overflow-x: auto`
  // container, unlike the keyboard-driven effect above which only needs `'nearest'` (a much
  // simpler case the browser handles fine) rather than true centering.
  useLayoutEffect(() => {
    if (!pages.data) return
    const strip = filmstripRef.current
    const frame = strip?.querySelector<HTMLElement>(`[data-filmstrip-page="${initialPage}"]`)
    if (!strip || !frame) return
    strip.scrollLeft = frame.offsetLeft - strip.clientWidth / 2 + frame.clientWidth / 2
    // `initialPage` deliberately excluded — this should only run once, when `pages.data` first
    // resolves after mount, not re-fire if `initialPage` ever changed identity for unrelated
    // reasons (it doesn't currently, but `PageLightbox` never remounts on prop change since
    // there's exactly one instance keyed by nothing — the effect body itself already reads
    // whatever `initialPage` was at mount time via closure, which is exactly the desired page).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [pages.data])

  // Latest-value refs for the keydown listener below — kept in a ref rather than read directly
  // from the closure so the listener itself can be registered exactly once (see that effect's own
  // docs on why: re-registering on every `previewPage` change was the real cause of a live-
  // confirmed bug where a fast arrow-key repeat occasionally jumped to an unexpected earlier
  // page — the effect tearing down and re-adding the `window` listener on every keystroke raced
  // against the browser's own OS-level key-repeat firing the next `keydown` before React's next
  // commit had finished swapping the old closure for the new one, so a stale `previewPage` value
  // from the *previous* closure could still be captured by that in-flight event).
  const latest = useRef({ previewPage, totalPages, loggedIn, onQuickAddToc, onClose })
  useLayoutEffect(() => {
    latest.current = { previewPage, totalPages, loggedIn, onQuickAddToc, onClose }
  })

  // Registered on the `capture` phase specifically — `Reader.tsx`'s own global `window.keydown`
  // listener (bubble phase, no capture) also treats `Escape`/arrow keys as its own reader
  // navigation, which would move the actual reading position or close the Archive Overview modal
  // underneath this lightbox if it ran first. Capture-phase listeners always run before
  // bubble-phase ones regardless of registration order, so this reliably intercepts the key first.
  // Registered exactly once (empty deps) — see `latest` ref above for why re-registering per
  // keystroke was itself the bug, not just unnecessary churn.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const { totalPages, loggedIn, onQuickAddToc, onClose } = latest.current
      if (e.key === 'Escape') {
        // Skip entirely if a `ChapterActionMenu` (edit/delete chapter) is currently open above
        // this lightbox — see `openChapterActionMenuCount`'s own docs for why a plain counter
        // check, not `stopImmediatePropagation`, is what actually solves this: this listener is
        // registered (at lightbox-mount time) *before* any menu's own listener could possibly
        // exist (menus only mount later, on click), and capture-phase listeners on the same
        // `window` target fire in registration order — so without this check, this handler
        // always ran first and closed the whole lightbox on Escape even while a menu was open
        // above it, no matter what that menu's own listener tried to do afterward (a real,
        // live-confirmed bug).
        if (openChapterActionMenuCount > 0) return
        // `stopImmediatePropagation`, not just `stopPropagation` — both this listener and
        // `Reader.tsx`'s own are registered on the *same* `window` object (just different phases,
        // capture vs. bubble); per the DOM spec, plain `stopPropagation` only blocks propagation
        // to *other* targets in the tree, not other listeners already registered on this same
        // target, so it alone did not actually stop `Reader.tsx`'s bubble-phase listener from
        // also firing (confirmed live: Escape closed both the lightbox *and* the Archive Overview
        // modal underneath it even with plain `stopPropagation` in place).
        e.stopImmediatePropagation()
        onClose()
        return
      }
      if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
        e.stopImmediatePropagation()
        e.preventDefault()
        scrollFilmstripOnNextPage.current = true
        // Functional update, not `previewPage + delta` off a value read from `latest.current` —
        // React guarantees a functional updater always sees the most recently *queued* state, so
        // it can't go stale even under fast repeated calls, unlike reading a plain closed-over/
        // ref-cached value (a real, live-confirmed bug: rapid ArrowRight presses could jump
        // backward mid-chapter when a stale `previewPage` snapshot got used for the next update
        // before the ref had synced past it).
        setPreviewPage((p) => Math.min(totalPages, Math.max(1, p + (e.key === 'ArrowLeft' ? -1 : 1))))
        return
      }
      // Point 6: 0-9 (top row or numpad) sets a chapter at the current preview page — 0 = Table of
      // Contents, 1-9 = that chapter number. Only fires while logged in (same guard as the
      // flattened `QuickAddTocOptions` row below, which this is a keyboard shortcut for).
      if (loggedIn && /^(Digit|Numpad)[0-9]$/.test(e.code)) {
        const n = e.code.slice(-1)
        e.stopImmediatePropagation()
        e.preventDefault()
        const title = n === '0' ? TOC_IDENTIFIER_TABLE_OF_CONTENTS : tocChapterIdentifier(Number(n))
        onQuickAddToc(latest.current.previewPage, title)
      }
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [])

  const chapterSpans = tocChapterSpans(toc, totalPages)
  const currentChapter = chapterForPage(chapterSpans, previewPage)
  const filmstripChapterByPage = new Map(
    (pages.data?.pages ?? []).map((_, i) => {
      const page = i + 1
      const chapter = chapterForPage(chapterSpans, page)
      return [page, chapter] as const
    }),
  )
  const onDarkBg = isDarkColor(palette.bg)

  function scrollFilmstrip(delta: number) {
    filmstripRef.current?.scrollBy({ left: delta })
  }

  return (
    <>
      {/* `Z_OVERLAY_ABOVE_LEGACY_MODAL`, not `Z_OVERLAY_BACKDROP`/`Z_OVERLAY_CONTENT` — this
          lightbox layers on top of the still-open Archive Overview modal underneath it
          (`#archivePagesOverlay`, legacy's own `.base-overlay` class carrying a hardcoded
          `z-index: 9000`), which the app's own generic overlay tiers don't clear (a real,
          live-confirmed bug: the lightbox rendered fully invisible behind that modal despite
          mounting later, since z-index — not DOM/mount order — decides paint order here).
          Background is the active theme's own `palette.bg` at reduced opacity (not a fixed
          `rgba(0,0,0,...)` scrim) — still reads as "dim what's behind", but the dimming color
          itself now follows the theme instead of always going pure black regardless of it. */}
      <div
        style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_ABOVE_LEGACY_MODAL, background: palette.bg, opacity: 0.9 }}
        onClick={(e) => {
          // `stopPropagation` — same real, live-confirmed bug class as `QuickAddTocPopover`'s own
          // backdrop earlier in this file: without it, this click bubbles up to `#overlay-shade`
          // (the Archive Overview modal's own click-to-close backdrop, still mounted underneath
          // and covering the same full viewport) and closes *that* too, not just this lightbox.
          e.stopPropagation()
          onClose()
        }}
      />
      <div
        style={{
          position: 'fixed',
          inset: '3vh 3vw',
          zIndex: Z_OVERLAY_ABOVE_LEGACY_MODAL + 1,
          display: 'flex',
          flexDirection: 'column',
          background: palette.bg,
          color: palette.text,
          border: `1px solid ${palette.border}`,
          borderRadius: 8,
          overflow: 'hidden',
        }}
        onClick={(e) => e.stopPropagation()}
      >

        {/* Large preview + info bar + flattened quick-add-chapter row. */}
        <div style={{ flex: '1 1 auto', display: 'flex', flexDirection: 'column', minHeight: 0, padding: 16 }}>
          <div style={{ flex: '1 1 auto', minHeight: 0, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
            {pageUrl && (
              <img
                key={pageUrl}
                src={pageUrl}
                alt={t('Page {{n}}', { n: previewPage }) ?? undefined}
                style={{ maxWidth: '100%', maxHeight: '100%', objectFit: 'contain' }}
                onLoad={(e) => {
                  const img = e.currentTarget
                  const page = previewPage
                  setMeasured({ page, width: img.naturalWidth, height: img.naturalHeight, sizeKb: null })
                  fetch(img.src, { method: 'HEAD' })
                    .then((res) => {
                      const bytes = Number(res.headers.get('Content-Length'))
                      if (Number.isNaN(bytes)) return
                      setMeasured((prev) => (prev?.page === page ? { ...prev, sizeKb: Math.floor(bytes / 1024) } : prev))
                    })
                    .catch(() => undefined)
                }}
              />
            )}
          </div>

          <div style={{ flex: '0 0 auto', textAlign: 'center', padding: '8px 0', fontSize: 13 }}>
            {t('Page {{n}}', { n: previewPage })}
            {' :: '}
            {pageUrl ? (new URL(pageUrl, window.location.origin).searchParams.get('path') ?? '') : ''}
            {dimensions && ` :: ${dimensions.width} x ${dimensions.height}`}
            {dimensions?.sizeKb !== null && dimensions?.sizeKb !== undefined && ` :: ${dimensions.sizeKb} KB`}
            {currentChapter && (
              <>
                {' :: '}
                {/* Bold/accent color only on the chapter's own start page — later pages belonging
                    to the same chapter show it in plain text plus a "(N)" total-page-count
                    suffix, so scrubbing through a long chapter still shows which one you're in
                    without visually implying *this* page is where it was set. */}
                {currentChapter.isStart ? (
                  <span style={{ fontWeight: 'bold', color: palette.hoverText }}>{displayTocName(currentChapter.name, t)}</span>
                ) : (
                  <span>{t('{{name}} ({{count}})', { name: displayTocName(currentChapter.name, t), count: currentChapter.count })}</span>
                )}
              </>
            )}
          </div>

          {loggedIn && (
            <div style={{ flex: '0 0 auto', display: 'flex', justifyContent: 'center', alignItems: 'center', gap: 12, flexWrap: 'wrap', padding: '4px 0' }}>
              {/* Same icon+text pairing as `QuickAddTocPopover`'s own `mainLabel` header (the
                  popover version of this same preset row) — reusing that already-translated
                  string here too instead of introducing a second, differently-worded label for
                  what is otherwise the identical feature just rendered inline. */}
              <span style={{ fontWeight: 'bold', opacity: 0.85 }}>
                <i className="fa fa-bolt" style={{ width: 18 }} aria-hidden="true"></i> {t('Quick Add Chapter')}
              </span>
              <QuickAddTocOptions asMenuItems={false} onPick={(title) => onQuickAddToc(previewPage, title)} />
              <Tooltip label={t("Press 0 for Table of Contents, or 1-9 for that chapter number, to set the current page as that chapter's start.") ?? ''}>
                <i className="fa fa-keyboard" aria-hidden="true" style={{ cursor: 'help', color: palette.text, opacity: 0.7 }}></i>
              </Tooltip>
              {toc.length > 0 && (
                <>
                  <a
                    className="fas fa-pencil-alt"
                    href="#"
                    style={{ padding: 4, fontSize: 14, color: palette.text }}
                    title={t('Edit Chapter name') ?? undefined}
                    onClick={(e) => {
                      e.preventDefault()
                      setEditTocMenuAt(e.currentTarget.getBoundingClientRect())
                    }}
                  />
                  {editTocMenuAt && (
                    <ChapterActionMenu
                      mode="edit"
                      anchor={editTocMenuAt}
                      chapters={toc}
                      zIndexBase={Z_OVERLAY_ABOVE_LEGACY_MODAL + 2}
                      onPick={(entry) => {
                        onEditToc(entry)
                        setEditTocMenuAt(null)
                      }}
                      onClose={() => setEditTocMenuAt(null)}
                    />
                  )}
                  <a
                    className="fas fa-trash-alt"
                    href="#"
                    style={{ padding: 4, fontSize: 14, color: palette.text }}
                    title={t('Delete Chapter') ?? undefined}
                    onClick={(e) => {
                      e.preventDefault()
                      setRemoveTocMenuAt(e.currentTarget.getBoundingClientRect())
                    }}
                  />
                  {removeTocMenuAt && (
                    <ChapterActionMenu
                      mode="delete"
                      anchor={removeTocMenuAt}
                      chapters={toc}
                      zIndexBase={Z_OVERLAY_ABOVE_LEGACY_MODAL + 2}
                      onPick={(entry) => {
                        onRemoveToc(entry)
                        setRemoveTocMenuAt(null)
                      }}
                      onClose={() => setRemoveTocMenuAt(null)}
                    />
                  )}
                </>
              )}
            </div>
          )}
        </div>

        {/* Bottom filmstrip gallery. */}
        {/* Height accounts for the 8px top/bottom padding + 120px frame + the chapter-label row
            each `LightboxFilmstripFrame` now renders beneath its thumbnail (point 2) — without
            the extra room, that label row pushed total content past the old fixed 136px and
            triggered an unwanted vertical scrollbar (confirmed live via a real screenshot). */}
        <div style={{ position: 'relative', flex: '0 0 auto', height: 158, background: palette.bg, borderTop: `1px solid ${palette.separator}` }}>
          <LightboxFilmstripEdge direction="left" onScroll={scrollFilmstrip} />
          <LightboxFilmstripEdge direction="right" onScroll={scrollFilmstrip} />
          <div
            ref={filmstripRef}
            style={{ display: 'flex', gap: 4, height: '100%', overflowX: 'auto', padding: '8px 48px', scrollBehavior: 'auto' }}
            // Mouse wheel scrolls the filmstrip horizontally (a plain vertical wheel gesture,
            // which browsers don't natively redirect into horizontal scroll on this element) —
            // `stopPropagation` so the wheel event doesn't also reach the Archive Overview modal's
            // own scroll underneath, which would otherwise scroll the page grid at the same time.
            onWheel={(e) => {
              e.stopPropagation()
              filmstripRef.current?.scrollBy({ left: e.deltaY })
            }}
          >
            {(pages.data?.pages ?? []).map((_, i) => {
              const page = i + 1
              const chapter = filmstripChapterByPage.get(page)
              return (
                <LightboxFilmstripFrame
                  key={i}
                  archiveId={archiveId}
                  page={page}
                  isPreview={page === previewPage}
                  accentColor={palette.hoverText}
                  borderColor={palette.border === 'transparent' ? palette.text : palette.border}
                  chapter={
                    chapter && {
                      // Swatch color is keyed on the raw stored `chapter.name` (the identifier,
                      // when it's a preset) rather than the display text — display text is
                      // locale-dependent, and the swatch should stay the same color regardless of
                      // which language the UI happens to be showing at the moment.
                      label: chapter.isStart
                        ? displayTocName(chapter.name, t)
                        : (t('{{name}} ({{count}})', { name: displayTocName(chapter.name, t), count: chapter.ordinal }) ?? displayTocName(chapter.name, t)),
                      swatch: chapterSwatchColor(chapter.name, onDarkBg),
                    }
                  }
                  onHover={() => {
                    // Suppressed right after a keyboard-driven scroll — `scrollIntoView` moving
                    // the filmstrip out from under a *stationary* mouse cursor still fires a real
                    // `mouseenter` on whatever frame ends up under the pointer, which otherwise
                    // clobbered the arrow key's own `setPreviewPage` call an instant after it ran
                    // (a real, live-confirmed bug: pressing → at the last visible frame scrolled
                    // the strip, and the resulting `mouseenter` silently jumped the preview back
                    // to an earlier page under the now-stationary cursor).
                    if (suppressHoverRef.current) return
                    setPreviewPage(page)
                  }}
                  onClick={() => setPreviewPage(page)}
                />
              )
            })}
          </div>
        </div>
      </div>
    </>
  )
}

/** One cell in the page-grid — split out from the inline map body so the hover state that both
 * `PageGridActionIcon`s need (see that component's own docs on why it can't track its own hover)
 * has somewhere to live: the parent `.quick-thumbnail` cell itself, exactly like legacy's own
 * `.quick-thumbnail:hover>.page-number` CSS rule keys off the same element. */
function PageGridCell({
  page,
  isStamped,
  loggedIn,
  highlighted,
  thumbnailSrc,
  onSelectPage,
  onSetThumbnail,
  onAddToc,
  onQuickAddToc,
  onOpenLightbox,
}: {
  page: number
  isStamped: boolean
  loggedIn: boolean
  /** Briefly true right after the overlay opens, for whichever page it opened on — see
   * `ArchiveOverviewOverlay`'s own `highlightedPage` docs. Rendered as a pulsing accent outline
   * (a plain animated `boxShadow`, not a static one, so it actually draws the eye across a grid
   * that can run into the hundreds of otherwise-identical cells) rather than anything relying on
   * a new global CSS class, since this is the only place that needs it. */
  highlighted: boolean
  thumbnailSrc: string
  onSelectPage: (page: number) => void
  onSetThumbnail: (e: MouseEvent, page: number) => void
  onAddToc: (e: MouseEvent, page: number) => void
  onQuickAddToc: (page: number, title: string) => void
  onOpenLightbox: (page: number) => void
}) {
  const { t } = useTranslation()
  const [hovered, setHovered] = useState(false)
  const [quickAddAt, setQuickAddAt] = useState<DOMRect | null>(null)
  return (
    <div
      // Not `className="id1"` — that class's own real intended purpose is the library grid's
      // `ArchiveCard` (verified against legacy's own `reader.js`: its real page-grid markup, line
      // 1791, wraps each cell in exactly `class='${thumbCss} quick-thumbnail'`, never `id1` at
      // all). Every currently-active theme's own `.id1` rule (e.g. `g.css`) carries a real
      // `min-height: 335px` tuned for that taller card (thumbnail + title + tags), which a page
      // cell here — just a shorter thumbnail with no title/tags below it — has no use for; the
      // extra ~55px was rendering as a real, visible gap of solid background color underneath
      // every cell (confirmed live via `getBoundingClientRect()`: `.id1` computed `335px` tall
      // while its own `.quick-thumbnail` child inside was only `280px`, the difference exactly
      // matching what the screenshot showed).
      data-page-cell={page}
      style={{ display: 'inline-block', cursor: 'pointer' }}
      onClick={() => onSelectPage(page)}
    >
      <div
        className="id3 quick-thumbnail"
        data-stamped={isStamped || undefined}
        style={{
          position: 'relative',
          ...(highlighted && {
            outline: '3px solid #3b97ea',
            outlineOffset: 2,
            animation: 'lrr-overview-highlight-pulse 0.6s ease-in-out 4',
          }),
        }}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <PageNumberLabel>{t('Page {{n}}', { n: page })}</PageNumberLabel>
        <OverviewThumbnail src={thumbnailSrc} alt={t('Page {{n}}', { n: page }) ?? undefined} />
        {/* Not gated behind `loggedIn` (unlike the two icons below) — this is a read-only preview
            tool, useful regardless of edit permissions; only the quick-add-chapter section
            *inside* the lightbox itself needs a login check (see `PageLightbox`'s own render). */}
        <PageGridActionIcon
          icon="fa-magnifying-glass"
          corner="bottom-left"
          title={t('View Full Page') ?? undefined}
          hovered={hovered}
          onClick={(e) => {
            e.preventDefault()
            e.stopPropagation()
            onOpenLightbox(page)
          }}
        />
        {loggedIn && (
          <>
            <PageGridActionIcon
              icon="fa-file-image"
              corner="top-right"
              title={t('Set this Page as Thumbnail') ?? undefined}
              hovered={hovered}
              onClick={(e) => onSetThumbnail(e, page)}
            />
            {/* `wrapperStyle={{ position: 'static' }}` — `Tooltip`'s own default wrapper is
                `position: relative`, which silently became this icon's *new* positioning
                containing block once it started wrapping it (the icon itself is `position:
                absolute; bottom: 2%` — a real, live-confirmed regression: without this override,
                that 2% resolved against the wrapper's own ~44px shrink-to-fit height instead of
                `.quick-thumbnail`'s real ~280px, landing the icon far too high).
                `anchor="cursor"` — a second, related consequence of the same `static` override:
                with no `position: relative`/`absolute` of its own, the wrapper's bounding box
                collapses around nothing once its only child escapes into `position: absolute`
                layout, so `Tooltip`'s default `anchor="element"` mode (which measures *that* box)
                placed the bubble near the icon's old, wrong pre-fix position instead of its real
                one (another real, live-reported bug). Following the cursor instead sidesteps
                needing a meaningful wrapper box at all. */}
            <Tooltip
              label={t('Add Chapter at this Page') + ' ' + t('(right-click for quick presets)')}
              wrapperStyle={{ position: 'static' }}
              anchor="cursor"
            >
              <PageGridActionIcon
                icon="fa-book-medical"
                corner="bottom-right"
                title={undefined}
                hovered={hovered}
                onClick={(e) => onAddToc(e, page)}
                onContextMenu={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  setQuickAddAt(e.currentTarget.getBoundingClientRect())
                }}
              />
            </Tooltip>
          </>
        )}
      </div>
      {quickAddAt && (
        <QuickAddTocPopover
          anchor={quickAddAt}
          onPick={(title) => onQuickAddToc(page, title)}
          onClose={() => setQuickAddAt(null)}
        />
      )}
    </div>
  )
}
