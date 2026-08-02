import type { MouseEvent, ReactNode } from 'react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import Tooltip from '../../../components/Tooltip'
import { QuickAddTocPopover } from './shared'

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

/** One cell in the page-grid — split out from the inline map body so the hover state that both
 * `PageGridActionIcon`s need (see that component's own docs on why it can't track its own hover)
 * has somewhere to live: the parent `.quick-thumbnail` cell itself, exactly like legacy's own
 * `.quick-thumbnail:hover>.page-number` CSS rule keys off the same element. */
export function PageGridCell({
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
