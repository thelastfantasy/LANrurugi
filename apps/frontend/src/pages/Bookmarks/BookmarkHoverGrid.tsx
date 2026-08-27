import { useEffect, useLayoutEffect, useRef, useState } from "react"
import { createPortal } from "react-dom"
import { useTranslation } from "react-i18next"
import { FaStamp, FaTrashCan } from "react-icons/fa6"
import { useNavigate } from "react-router-dom"

import { useBookmarksForArchive, useRemoveBookmark } from "@/api/hooks"
import { IconButton } from "@/components/common-ui/Display"
import { Confirm } from "@/components/Display"
import { useSupportsHover } from "@/hooks/useSupportsHover"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, Z_OVERLAY_BACKDROP, Z_OVERLAY_TOOLTIP } from "@/theme"
import { toast } from "@/toast"

import { sortPagesByOrder, useHoverGridPageOrder } from "./useHoverGridPageOrder"

/** Every cell (including the first) is this size — matches the cover's own on-screen box
 * (`anchorRect`), so the grid reads as one uniform set of thumbnails rather than a giant first
 * cell next to tiny ones. */
function cellSize(anchorRect: DOMRect) {
  return { width: anchorRect.width, height: anchorRect.height }
}

const BORDER_WIDTH = 1
/** How far outside `anchorRect` the bordered frame extends — kept off the grid's own box-model
 * (see `outer` below) so the first cell's image still lines up pixel-for-pixel with the card's
 * real cover instead of being squeezed smaller by the frame's own padding. */
const FRAME_PADDING = 6
const GRID_GAP = 6
/** Caps the grid at 3×3 regardless of viewport space or page count — an unbounded `maxWidth`
 * (an earlier version used `window.innerWidth - anchorRect.left`) let the frame stretch across
 * nearly the entire remaining screen width even for just two bookmarked pages, since a CSS Grid
 * container doesn't shrink to its actual content by default. Capping the *cell count* (rather
 * than a raw pixel size) keeps the frame's own width/height genuinely elastic — 1–2 pages produce
 * a small frame, 4–9 wrap across up to 3 rows, and beyond 9 the grid scrolls vertically inside a
 * fixed 3-row-tall viewport instead of the frame ever growing past 3×3. */
const MAX_COLUMNS = 3
const MAX_ROWS = 3
/** How long a wheel gesture has to keep scrolling in the same direction past this grid's own
 * vertical scroll boundary before `handleWheel` actually hands it off to `onWheelPassthrough` —
 * see that prop's own docs for why a hand-off exists at all. Long enough that the tail end of a
 * single continuous scroll gesture (mouse wheel inertia, a trackpad's own momentum scroll) reliably
 * finishes *before* this elapses, so it doesn't accidentally count as "the user scrolled again on
 * purpose"; short enough that a genuinely new, deliberate scroll input still reads as responsive
 * rather than laggy. */
const EDGE_HANDOFF_DELAY_MS = 500
/** How much extra width the grid's own CSS `width` reserves (via `scrollbarGutter: "stable"`
 * below) so a vertical scrollbar's appearance never squeezes the grid's explicit column tracks —
 * live-measured against this repo's actual dev environment (Chromium/Linux) at ~10px; a couple px
 * of margin on top costs nothing (worst case: a few px of harmless blank space past the last
 * column) while under-reserving visibly clips the last column's content (confirmed live: this is
 * exactly what happened before this constant existed — the third column's delete button got cut
 * off by `overflowX: "hidden"` once enough bookmarks triggered a vertical scrollbar). Real
 * scrollbar width does vary by OS/browser (macOS overlay scrollbars can render at ~0px), but this
 * only ever needs to be *at least as wide* as whatever the current browser actually reserves. */
const SCROLLBAR_WIDTH_ESTIMATE = 12
/** Caption line's own footprint below each thumbnail — `FONT_SIZE_SM`'s line-height plus the
 * `marginTop` below, measured against this component's own rendered caption (`Math.ceil` so a
 * sub-pixel line-height never gets rounded down into visually clipping the caption's last row). */
const CAPTION_HEIGHT = 20

/** How many `cellWidth`/`cellHeight`-sized tracks (plus `GRID_GAP` between them) fit in
 * `availablePx` — same "how much room is actually there" question `Tooltip.tsx`'s own
 * `recompute()` asks of `spaceBelow`/`spaceAbove`, just answered in track counts instead of a
 * single `maxHeight`. Never returns less than 1: a hover preview that can't fit even one cell
 * should still show one (scrolling/clipping past the edge) rather than collapsing to nothing —
 * this is also what keeps a narrow (e.g. mobile-width) viewport at a real, usable single column
 * instead of forcing the `MAX_COLUMNS` cap regardless of available space. */
function tracksThatFit(availablePx: number, trackPx: number): number {
  return Math.max(1, Math.floor((availablePx + GRID_GAP) / (trackPx + GRID_GAP)))
}

/** Hover preview for a bookmarked archive — expands into a grid of that archive's bookmarked-page
 * thumbnails (max 3×3, scrolling past that — see `MAX_COLUMNS`/`MAX_ROWS`), with one cell placed
 * to exactly cover the card's own cover thumbnail (`anchorRect`) rather than floating beside it
 * like `components/Display/Tooltip.tsx` does. Deliberately doesn't reuse that component's
 * `recompute()` positioning: this needs the *inverse* behavior — no `GAP`, since the anchored
 * cell's position is a hard constraint (must equal the cover's own box), not something free to
 * relocate.
 *
 * *Which* cell is anchored isn't always the grid's top-left corner — `opensLeft`/`opensUp` pick
 * whichever corner has actual viewport room on that side (same "does this side have space" check
 * `Tooltip.tsx` does, just per-axis and per-corner instead of a single preferred side), and
 * `reorderForCorner` rearranges `livePages` so the anchored page always ends up in that corner via
 * plain grid auto-flow, no explicit per-cell `gridColumn`/`gridRow` needed.
 *
 * The bordered `.swal2-popup` frame (every theme's own "this is a popup" skin, same class
 * `Tooltip.tsx` uses) is sized to `anchorRect` *plus* `FRAME_PADDING` on every side and positioned
 * so its edge on the anchored corner's side sits at `FRAME_PADDING` past `anchorRect` — it grows
 * outward around the grid rather than padding inward, so it never eats into the anchored cell's
 * own pixel-for-pixel alignment with the cover underneath it. */
export function BookmarkHoverGrid({
  anchorRect,
  archiveId,
  archiveTitle,
  pages,
  onClose,
  onWheelPassthrough,
}: {
  anchorRect: DOMRect
  archiveId: string
  /** For the delete-confirm dialog's own message — naming which archive a bookmark belongs to
   * matters here specifically because this grid can show pages from any bookmarked archive
   * (unlike, say, the reader's own single-archive bookmark toggle, which never needs to say which
   * archive it's talking about). */
  archiveTitle: string
  /** Ascending — `pages[0]` is the *archive's* first bookmarked page, i.e. `livePages[0]`, the
   * page `reorderForCorner` anchors to `anchorRect` regardless of which corner it ends up
   * rendered in. */
  pages: number[]
  onClose: () => void
  /** Forwards a wheel gesture the grid itself doesn't consume — only ever passed by
   * `RecentlyAddedCarousel`'s own bookmark-mode call site (via `BookmarkedArchiveHoverCard`), so
   * it can keep driving its horizontal Lenis scroll while the pointer happens to be sitting over
   * an open hover preview instead of the carousel strip underneath it (a real, live-reported bug:
   * the preview's own portal captures every wheel event, so the carousel silently stopped
   * responding to the scroll wheel entirely whenever a preview was open). `undefined` on the
   * standalone `/bookmarks` page, which has no horizontal strip for a passthrough to feed into —
   * that call site simply doesn't pass this prop, leaving the grid's own scroll container's
   * default (page-level) wheel behavior untouched. See `handleWheel` below for the actual
   * three-way dispatch logic this drives.
   *
   * Takes both deltas (not just `deltaY`) so the call site can replay a faithful synthetic
   * `WheelEvent` onto its own Lenis-driven element — see `RecentlyAddedCarousel`'s own
   * `handleBookmarkWheelPassthrough` for why a hand-computed scroll offset isn't good enough. */
  onWheelPassthrough?: (deltaX: number, deltaY: number) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const supportsHover = useSupportsHover()
  const rootRef = useRef<HTMLDivElement>(null)
  // The grid's own internal scroll container — read by both the `opensUp`-and-`willScroll`
  // scroll-to-bottom `useLayoutEffect` below and the wheel-passthrough listener further down.
  const scrollElRef = useRef<HTMLDivElement>(null)

  // Touch-only (see the backdrop below, gated the same way): locks `body`'s own scroll for as
  // long as this grid is open. Fixes a real reported bug — mobile Chrome/Firefox auto-hide their
  // `body` scrollbar (and shrink the URL bar) on scroll-down and restore it on scroll-up, which
  // changes the viewport's real available height out from under `anchorRect` (a one-time snapshot
  // taken when the grid opened), visibly shifting this grid's position relative to the card it's
  // anchored to. Locking scroll prevents that resize from happening at all while the grid is open,
  // rather than trying to track and re-anchor against a moving viewport. Same
  // `overflow`/`overscrollBehavior` pattern as `Reader.tsx`'s boundary-overlay lock and
  // `ComparisonResultModal.tsx`'s own modal lock.
  useEffect(() => {
    if (supportsHover) return
    const prevOverflow = document.body.style.overflow
    const prevOverscroll = document.body.style.overscrollBehavior
    document.body.style.overflow = "hidden"
    document.body.style.overscrollBehavior = "none"
    return () => {
      document.body.style.overflow = prevOverflow
      document.body.style.overscrollBehavior = prevOverscroll
    }
  }, [supportsHover])
  // Filenames come bundled in the same response as the page numbers themselves (`GET
  // /archives/{id}/bookmarks`) — no separate `GET /archives/{id}/files` call (which would scan
  // the archive's entire page list just to resolve two or three filenames out of it).
  const bookmarkedPages = useBookmarksForArchive(archiveId)
  const removeBookmark = useRemoveBookmark()
  const [pendingDelete, setPendingDelete] = useState<number | null>(null)
  const [pageOrder] = useHoverGridPageOrder()
  // The actual render source is this query's own live data, not the `pages` prop — the prop is
  // just a one-time snapshot taken when the hover/tap first opened (`BookmarkedArchiveHoverCard`),
  // so it wouldn't reflect a page removed via the delete button below without a full close/reopen.
  // Falling back to `pages` only while the query's own first fetch is still in flight avoids an
  // empty flash on open (the query is already warm from `BookmarkedArchiveHoverCard`'s own use of
  // the same `queryKey`, but a cold-cache first hover would otherwise show nothing for a frame).
  // `bookmarked_at: 0` for that fallback path — the prop itself only ever carries page numbers
  // (see its own docs), so a "sort by when bookmarked" order briefly has nothing real to sort by
  // until the query's real data replaces it a render or two later; `pageAsc`'s own tie-break
  // (`page` itself) keeps that brief window looking identical to the pre-this-setting behavior.
  const sortedPages = sortPagesByOrder(
    bookmarkedPages.data ?? pages.map((page) => ({ page, filename: null, bookmarked_at: 0, stamp_count: 0 })),
    pageOrder,
  )
  const livePages = sortedPages.map((b) => b.page)
  const { width: cellWidth, height: cellHeight } = cellSize(anchorRect)
  const rowHeight = cellHeight + CAPTION_HEIGHT


  // Viewport space actually available to the right/below `anchorRect` — same question
  // `Tooltip.tsx`'s own `recompute()` asks (there, of `spaceBelow`/`spaceAbove`) before deciding
  // how much room a popup really has, not just how much content it *wants* to show. Recomputed on
  // resize; `useState`'s initializer covers the very first render before any resize fires.
  const [viewport, setViewport] = useState(() => ({ width: window.innerWidth, height: window.innerHeight }))
  useLayoutEffect(() => {
    function recompute() {
      setViewport({ width: window.innerWidth, height: window.innerHeight })
    }
    window.addEventListener("resize", recompute)
    return () => window.removeEventListener("resize", recompute)
  }, [])

  const frameOverhead = (FRAME_PADDING + BORDER_WIDTH) * 2
  // Space on *each* side of `anchorRect` — not just right/below, now that the grid can open
  // toward whichever side actually has room (see `opensLeft`/`opensUp` below).
  const spaceRight = viewport.width - anchorRect.left - frameOverhead
  const spaceLeft = anchorRect.right - frameOverhead
  const spaceBelow = viewport.height - anchorRect.top - frameOverhead
  const spaceAbove = anchorRect.bottom - frameOverhead

  // Viewport space wins over the 3×3 cap, and both lose to how many pages there actually are —
  // never show more empty tracks than content, and never claim more space than the screen has,
  // regardless of how many pages are bookmarked. `tracksThatFit`'s own `Math.max(1, ...)` is what
  // guarantees at least one column survives even on a narrow (e.g. mobile) viewport where 3
  // columns of `cellWidth`-sized covers genuinely wouldn't fit.
  //
  // Column/row *count* is picked from whichever side has more room (`Math.max`) — the grid still
  // opens toward the *other* side if that's where more space actually is (`opensLeft`/`opensUp`
  // below use the same `spaceRight`-vs-`spaceLeft` comparison), so this only has to answer "how
  // many tracks fit at all", not "which direction". Using the max of both sides (rather than
  // whichever side was picked to open toward) means a cell count that's consistent regardless of
  // which corner ends up anchored — the grid's own size doesn't visibly jump depending on which
  // side of the viewport the card happens to be on.
  const columns = Math.min(MAX_COLUMNS, tracksThatFit(Math.max(spaceLeft, spaceRight), cellWidth), livePages.length || 1)
  const rows = Math.min(MAX_ROWS, tracksThatFit(Math.max(spaceAbove, spaceBelow), rowHeight), Math.ceil(livePages.length / columns))
  // Whether `overflowY: "auto"` below will actually show a scrollbar — true whenever the real row
  // count `livePages` needs exceeds what `rows` displays at once. Gates the `SCROLLBAR_WIDTH_
  // ESTIMATE` allowance in `gridWidth` just below (only reserve the space when a scrollbar can
  // actually appear — an unconditional reservation was tried first and rejected: it left a
  // visible, unexplained gap of blank space past the last column on every short list that never
  // scrolls at all, worse than the layout shift it was trying to prevent) and the scroll-to-bottom
  // `useLayoutEffect` further down (only needed when there's actually something to scroll).
  const willScroll = Math.ceil(livePages.length / columns) > rows
  // What `gridTemplateColumns`' own tracks add up to, plus `SCROLLBAR_WIDTH_ESTIMATE` — but only
  // when `willScroll` is true. The grid element's CSS `width` below reserves that extra allowance
  // via `scrollbarGutter: "stable"`, gated the same way (only `scrollbarGutter: "stable"` when
  // `willScroll`) — real, live measurement showed *why* the allowance is needed at all when
  // scrolling: Chrome's actual `scrollbar-gutter: stable` behavior shrinks the usable content area
  // *within* a declared `width` by the scrollbar's width the moment a vertical scrollbar appears,
  // while `gridTemplateColumns`'s explicit `columns` tracks still each demand their full
  // `cellWidth`px regardless — without inflating `width` to compensate, the last column (and
  // anything positioned relative to it, like each cell's own delete button at `right: 2`) has
  // nowhere to fit once the scrollbar carves its width out, and gets clipped by this element's own
  // `overflowX: "hidden"` below (confirmed live: the third column's delete button was visibly cut
  // off exactly when enough bookmarks triggered vertical scrolling). Gating on `willScroll` does
  // reintroduce a one-time sideways shift if `livePages.length` later crosses the scrolling
  // threshold (e.g. via the delete button) — accepted deliberately: keeping short, non-scrolling
  // lists visually tight (no unexplained blank space past the last column) matters more here than
  // avoiding that shift.
  const gridWidth = columns * cellWidth + (columns - 1) * GRID_GAP + (willScroll ? SCROLLBAR_WIDTH_ESTIMATE : 0)
  const gridHeight = rows * rowHeight + (rows - 1) * GRID_GAP

  // Which corner of the grid lands on `anchorRect` — always the corner closest to whichever side
  // actually has *more* room, full stop (no "prefer the default corner unless the preferred side
  // is generously insufficient" hysteresis — tried first and rejected: on a small/short mobile
  // viewport where both above and below are genuinely too small for the grid, that hysteresis kept
  // picking "opens down" regardless, running the grid's bottom edge off the bottom of the screen
  // even though the top had more headroom; confirmed live on an actual phone). Ties (equal space
  // on both sides) keep the original down-right default.
  const opensLeft = spaceLeft > spaceRight
  const opensUp = spaceAbove > spaceBelow

  // Rearranges `livePages` so the anchored page (`livePages[0]`) lands in whichever grid corner
  // `opensLeft`/`opensUp` put it in, without ever needing an explicit per-cell `gridColumn`/
  // `gridRow` — plain row-major grid auto-flow handles the rest once the array itself is in the
  // right order. Splits into `columns`-wide chunks (each one grid row), reverses each chunk's own
  // order when opening left (so the anchor's chunk renders with the anchor cell last, i.e.
  // rightmost), and reverses the chunk order itself when opening up (so the anchor's chunk renders
  // last, i.e. bottommost). The anchor's own chunk (`livePages[0..columns-1]`) is guaranteed to be
  // completely full (never a short trailing row) because `columns` is derived as `Math.min(...,
  // livePages.length || 1)` — it can never exceed `livePages.length` — so only a *later*, already
  // non-anchored chunk can ever be partial, and reversal there is a purely cosmetic no-op concern
  // rather than something that could break the anchor's own alignment.
  //
  // `opensUp` moving the anchor's row to array-*end* would normally get hidden by `willScroll`'s
  // own `overflowY: auto` + `maxHeight` (which always shows the rendered array's first `rows` rows)
  // — handled below by a `useLayoutEffect` that scrolls the grid to its own bottom on open whenever
  // `opensUp && willScroll`, so the last (anchor) row is the one actually visible rather than the
  // scroll-clipped first one.
  function reorderForCorner(pagesToOrder: number[]): number[] {
    const rows: number[][] = []
    for (let i = 0; i < pagesToOrder.length; i += columns) {
      rows.push(pagesToOrder.slice(i, i + columns))
    }
    if (opensLeft) rows.forEach((row) => row.reverse())
    if (opensUp) rows.reverse()
    return rows.flat()
  }
  const orderedPages = reorderForCorner(livePages)

  // See `reorderForCorner`'s own docs for why this is needed at all: `opensUp` puts the anchor's
  // row last in the array, but a scrolling grid only shows its *first* `rows` rows by default —
  // without this, the anchor's own row (and thus its alignment with `anchorRect`) would be
  // scrolled out of view the instant the grid opens. Runs on every relevant change (not just
  // mount) so deleting a bookmark — which can shrink `orderedPages.length` and thus
  // `scrollHeight` — keeps the grid pinned to its new bottom rather than leaving a gap.
  useLayoutEffect(() => {
    if (!opensUp || !willScroll) return
    const el = scrollElRef.current
    if (!el) return
    el.scrollTop = el.scrollHeight - el.clientHeight
  }, [opensUp, willScroll, orderedPages.length])

  function openPage(page: number) {
    navigate(`${routes.reader(archiveId)}?p=${page}`)
  }

  // When (and only when) the grid can't itself scroll vertically, or the wheel gesture is
  // horizontal to begin with, this always forwards immediately — `willScroll === false` means
  // there's no `overflowY` content to compete for the gesture at all, and a horizontal wheel
  // (e.g. a mouse with its own side-scroll wheel/thumb wheel, like Logitech's MX Master series)
  // has nothing to scroll here regardless — `overflowX` is unconditionally `"hidden"` (see that
  // rule's own docs), so this grid never has a horizontal scrollbar to claim the gesture for.
  //
  // The remaining case — `willScroll === true` and the gesture is vertical — needs the actual
  // "hit the edge, pause, only then hand off" debounce: scrolling the grid's own content normally
  // until `scrollTop` reaches either boundary, then absorbing (not forwarding) same-direction
  // wheel input for `EDGE_HANDOFF_DELAY_MS` so a single continuous scroll gesture that arrives at
  // the bottom doesn't bleed straight into scrolling the carousel underneath with no perceptible
  // boundary — only a *new* same-direction wheel tick arriving after that pause counts as the
  // user having deliberately kept scrolling past an already-fully-scrolled grid.
  //
  // Registered as a real, imperative `addEventListener("wheel", ..., { passive: false })` below
  // rather than a plain React `onWheel` prop — React 17+ attaches its own synthetic `onWheel`
  // (and `onTouchStart`/`onTouchMove`) listener at the root as *passive* for scroll-performance
  // reasons, which silently makes every `e.preventDefault()` call in this function a no-op (the
  // browser only warns in the console, doesn't throw), so the "absorb" and "forward to carousel"
  // paths below both failed to stop the browser's own default scroll from also reaching past this
  // grid to the page's own `body` scrollbar underneath (confirmed live: page behind the tooltip
  // visibly scrolled in lockstep with the tooltip). An imperative listener isn't subject to
  // React's passive default and can actually opt out via the options object.
  // `forwarding: false` while still inside the initial `EDGE_HANDOFF_DELAY_MS` pause (ticks get
  // absorbed); flips to `true` the moment that pause elapses and stays `true` for every
  // subsequent same-direction tick, rather than resetting after each one. That "stays true" part
  // matters: an earlier version cleared this state after every single forwarded tick, which
  // silently turned "keep scrolling past the edge" into a repeating cycle of forward-one-tick,
  // then-wait-500ms-again — Lenis only builds up its usual scroll velocity/inertia from a
  // continuous run of closely-spaced real wheel events (see `handleBookmarkWheelPassthrough`'s own
  // docs for why events are *replayed*, not manually computed), so feeding it isolated ticks
  // roughly 500ms apart produced a scroll that was both correct in direction and far smaller/
  // choppier than scrolling the carousel directly (confirmed live: passthrough scroll past an
  // already-bottomed-out tooltip felt noticeably weaker than the no-scrollbar-tooltip case, which
  // forwards every tick unthrottled). Reset back to `null` whenever the gesture leaves the edge
  // (scrolls back away from the boundary) or reverses direction — either means the user is no
  // longer in the "kept scrolling past an edge" gesture this state exists to track.
  const edgeState = useRef<{ direction: "up" | "down"; firstHitTime: number; forwarding: boolean } | null>(null)
  useEffect(() => {
    const el = scrollElRef.current
    if (!el || !onWheelPassthrough) return
    function handleWheel(e: WheelEvent) {
      if (!onWheelPassthrough) return
      if (!willScroll || e.deltaX !== 0) {
        e.preventDefault()
        onWheelPassthrough(e.deltaX, e.deltaY)
        return
      }
      const el = e.currentTarget as HTMLDivElement
      const direction = e.deltaY > 0 ? "down" : "up"
      const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 1
      const atTop = el.scrollTop <= 0
      const atEdgeForDirection = (direction === "down" && atBottom) || (direction === "up" && atTop)
      if (!atEdgeForDirection) {
        edgeState.current = null
        return // Let the browser scroll the grid's own content normally.
      }
      const state = edgeState.current
      if (state && state.direction === direction && state.forwarding) {
        // Already past the pause window and forwarding — keep forwarding every tick, unthrottled,
        // so Lenis sees the same continuous run of events it would from scrolling it directly.
        e.preventDefault()
        onWheelPassthrough(0, e.deltaY)
        return
      }
      if (!state || state.direction !== direction) {
        // The first tick to hit this edge in this direction (or a direction reversal) — starts
        // the pause window rather than forwarding immediately, so a single scroll gesture that
        // merely *reaches* the edge doesn't also immediately bleed into the carousel.
        edgeState.current = { direction, firstHitTime: performance.now(), forwarding: false }
        e.preventDefault()
        return
      }
      if (performance.now() - state.firstHitTime < EDGE_HANDOFF_DELAY_MS) {
        // Still inside the pause window from the first tick that hit this edge — absorb this one
        // too (neither scrolls the grid further, since it's already at the boundary, nor forwards
        // it to the carousel yet).
        e.preventDefault()
        return
      }
      // The pause window has elapsed and the user kept scrolling in the same direction — genuinely
      // hands off now, and every further same-direction tick until the next edge/direction change.
      edgeState.current = { ...state, forwarding: true }
      e.preventDefault()
      onWheelPassthrough(0, e.deltaY)
    }
    el.addEventListener("wheel", handleWheel, { passive: false })
    return () => el.removeEventListener("wheel", handleWheel)
  }, [onWheelPassthrough, willScroll])

  // Guards `onMouseLeave` below — the confirm dialog (`pendingDelete !== null`) renders through
  // its own nested portal straight onto `document.body`, genuinely outside `rootRef`'s DOM
  // subtree (it's a `position: fixed`, screen-centered dialog, not something anchored inside this
  // grid). `mouseleave` doesn't follow React's portal-aware event tree — it's a plain native event
  // keyed on real DOM geometry — so moving the pointer off the grid and onto that dialog to click
  // its own Confirm/Cancel button genuinely leaves `rootRef`'s bounds and would otherwise fire
  // `onClose()` before the dialog click ever lands, closing the whole preview (and, with it, the
  // dialog) out from under the user.
  function handleMouseLeave() {
    if (pendingDelete !== null) return
    onClose()
  }

  return createPortal(
    <>
      {/* Touch-only backdrop — replaces an earlier `document`-level `mousedown` listener that
          tried to detect "tap landed outside the grid" by walking `rootRef`'s own DOM subtree.
          That approach broke for exactly the delete-confirm dialog below: `Confirm` renders
          through its *own* separate `createPortal` straight onto `document.body`, never a
          descendant of `rootRef` at all, so tapping its own "删除" button read as "outside" and
          fired `onClose()` on `mousedown` — unmounting this whole grid (dialog and pending delete
          mutation included) *before* the button's own `click` (and the network request it
          triggers) ever got a chance to fire. A backdrop sidesteps that whole class of bug: it's
          a real, visible, clickable element occupying its own spot in the stacking order, so
          anything rendered *above* it (the grid's own content, and the dialog's still-higher
          `Z_OVERLAY_TOOLTIP + 1` layer) is never mistaken for "outside" no matter which portal it
          came from — only an actual tap on the backdrop itself closes anything. Skipped on a
          hover-capable device — `onMouseLeave` below already covers closing there, and a visible
          gray overlay for a hover-triggered preview would be an unrequested, purely decorative
          change to established desktop behavior. */}
      {!supportsHover && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }}
          onClick={() => {
            if (pendingDelete !== null) return
            onClose()
          }}
        />
      )}
      <div
        ref={rootRef}
        onMouseLeave={handleMouseLeave}
        style={{
        // `"absolute"` (document-relative), not `"fixed"` (viewport-relative) — same fix
        // `ArchiveContextMenu.tsx` already applies to the right-click menu (see that component's
        // own docs): a `"fixed"` popup stays pinned to the same screen position while the page
        // scrolls underneath it, drifting away from the cover it's supposed to be anchored to.
        // `anchorRect` itself stays viewport-relative (`getBoundingClientRect()`, used as-is for
        // the `availableWidth`/`availableHeight` viewport-space math below, which legitimately
        // cares about the *screen*, not the document) — only the final `top`/`left` add
        // `window.scrollY`/`scrollX` to convert into document coordinates for this element's own
        // `position: absolute` to use.
        position: "absolute",
        // Offsets by the frame's *own* box-model footprint (padding + its 1px border) so the
        // anchored cell's image still lands exactly on `anchorRect` — the border itself also
        // takes up space in standard box-sizing, not just the padding, and was left out of an
        // earlier version of this offset (confirmed live: without `+ BORDER_WIDTH`, the first
        // cell landed 1px off from the real cover underneath it).
        //
        // Which corner of the grid the anchored cell sits in flips with `opensLeft`/`opensUp`, so
        // which of `anchorRect`'s edges the frame is positioned *from* flips too:
        // - Default (opens down-right): anchored cell is the grid's top-left, so `anchorRect`'s
        //   own top-left corner is what the frame offsets from (same as the original formula).
        // - `opensLeft`: anchored cell is the grid's top-*right* — its right edge, not left, sits
        //   at `anchorRect.right`, so `left` is derived by subtracting the frame's full width
        //   (`gridWidth + 2 * frameOverhead-per-side`) back from `anchorRect.right` instead of
        //   adding the padding/border offset to `anchorRect.left`.
        // - `opensUp`: same idea vertically — the anchored cell is the grid's *bottom* row, so
        //   `top` is derived by walking back up from `anchorRect.top` by every row above it
        //   (`gridHeight - rowHeight`, i.e. all rows except the anchored one, plus their gaps).
        top: opensUp
          ? anchorRect.top - (gridHeight - rowHeight) - FRAME_PADDING - BORDER_WIDTH + window.scrollY
          : anchorRect.top - FRAME_PADDING - BORDER_WIDTH + window.scrollY,
        left: opensLeft
          ? anchorRect.right - gridWidth - FRAME_PADDING - BORDER_WIDTH + window.scrollX
          : anchorRect.left - FRAME_PADDING - BORDER_WIDTH + window.scrollX,
        zIndex: Z_OVERLAY_TOOLTIP,
      }}
    >
      <div
        className="swal2-popup"
        style={{
          display: "block",
          padding: FRAME_PADDING,
          borderWidth: BORDER_WIDTH,
          borderRadius: 4,
          boxShadow: "0 2px 8px rgba(0,0,0,0.3)",
        }}
      >
        {/* Scrolls internally past 9 cells — nested one level inside the padded/rounded
            `.swal2-popup` box above (same reasoning as `Tooltip.tsx`'s own nested scroll div: a
            scrollbar directly on the rounded/bordered element clips against its own corners). */}
        <div
          ref={scrollElRef}
          className="thin-scrollbar"
          style={{
            display: "grid",
            gridTemplateColumns: `repeat(${columns}, ${cellWidth}px)`,
            gap: GRID_GAP,
            width: gridWidth,
            maxHeight: gridHeight,
            overflowY: "auto",
            // Explicit `"hidden"`, not the default `"visible"` — `columns` is already derived from
            // `tracksThatFit` to stay within the viewport's real available width, so this never
            // needs to actually clip anything in practice, but it's cheap insurance against a
            // horizontal scrollbar ever appearing at all (an explicit product requirement — this
            // grid must never scroll sideways) rather than relying on the width math always being
            // exactly right on every device/zoom level.
            overflowX: "hidden",
            // Reserves `SCROLLBAR_WIDTH_ESTIMATE` worth of extra space (folded into `gridWidth`
            // above, same `willScroll` gate) so a vertical scrollbar's own appearance doesn't
            // squeeze the explicit `columns`-track grid — see `SCROLLBAR_WIDTH_ESTIMATE`'s own
            // docs for the real bug this fixes (the last column's delete button getting clipped by
            // `overflowX: "hidden"` above once scrolling kicked in). Gated on `willScroll` (not
            // unconditional) so a short list that never scrolls at all doesn't carry an
            // unexplained gap of blank space past its last column — `gridWidth`'s own docs cover
            // why that tradeoff (accepting a possible one-time sideways shift on crossing the
            // scrolling threshold) was chosen deliberately.
            ...(willScroll ? { scrollbarGutter: "stable" } : {}),
          }}
        >
          {orderedPages.map((page) => {
            const filename = bookmarkedPages.data?.find((b) => b.page === page)?.filename ?? null
            const stampCount = bookmarkedPages.data?.find((b) => b.page === page)?.stamp_count ?? 0
            return (
              <a
                key={page}
                href={`${routes.reader(archiveId)}?p=${page}`}
                onClick={(e) => {
                  e.preventDefault()
                  openPage(page)
                }}
                style={{ display: "block", position: "relative", color: "inherit", textDecoration: "none" }}
              >
                <img
                  src={`/api/archives/${archiveId}/thumbnail?page=${page}`}
                  alt=""
                  style={{
                    width: cellWidth,
                    height: cellHeight,
                    // Matches `lrr.css`'s own `div.id3:not(.nocrop) img` default cover cropping,
                    // so the first cell doesn't visually jump when it swaps in over the real cover
                    // thumbnail.
                    objectFit: "cover",
                    display: "block",
                  }}
                />
                {/* issue #97: a stamp-count badge, top-left — mirrors the delete button's own
                    top-right `position: absolute` placement below. `FaStamp` (react-icons/fa6's
                    real ink-stamp SVG, matching the Font Awesome `fa-stamp` glyph the reader
                    toolbar's own stamp-placement button uses) reads as "the stamp feature itself"
                    — confirmed against a user's own screenshot as the expected icon here, same
                    correction applied to that toolbar button (2026-08-27); `dialog.tsx`'s own
                    `fa-thumbtack` default is a *per-stamp* fallback icon (used when an individual
                    stamp has no custom icon set), a narrower, different role than this badge's
                    "does this page have any stamps at all." */}
                {stampCount > 0 && (
                  <div style={{ position: "absolute", top: 2, left: 2, display: "flex" }}>
                    <span
                      title={t("bookmarks.pageHasStamps", { count: stampCount }) ?? undefined}
                      style={{
                        display: "flex",
                        alignItems: "center",
                        gap: 3,
                        background: "rgba(0,0,0,0.55)",
                        color: "#fff",
                        borderRadius: 10,
                        fontSize: FONT_SIZE_SM,
                        padding: "1px 6px",
                      }}
                    >
                      <FaStamp size={9} aria-hidden="true" />
                      {stampCount}
                    </span>
                  </div>
                )}
                {/* Wrapped in its own `onClick` (bubble phase) — this button is nested inside the
                    cell's `<a>` (needed so the delete affordance sits visually inside each
                    thumbnail, top-right), so a click here bubbles up through the `<a>` unless
                    stopped first. `IconButton` itself only exposes a plain `() => void` `onClick`
                    (no event object to call `stopPropagation` on directly), so the stop happens on
                    this wrapping `div` instead — it catches the bubble before it ever reaches the
                    `<a>`'s own `onClick`, which would otherwise navigate to the reader in the same
                    gesture as opening the confirm dialog. */}
                <div
                  onClick={(e) => {
                    e.preventDefault()
                    e.stopPropagation()
                  }}
                  // `display: "flex"` — without it, this plain block div gives its `<button>`
                  // child (an inline-level element by default) the usual inline-layout descender
                  // gap below the line box, adding a few px of unwanted height that pushed the
                  // button visibly downward inside this `top: 2`-positioned wrapper (confirmed
                  // live: the wrapper measured 24px tall against the button's own 20px, and the
                  // button's real top landed 4px below where `top: 2` alone would place it).
                  style={{ position: "absolute", top: 2, right: 2, display: "flex" }}
                >
                  <IconButton
                    // `react-icons/fa6`'s `FaTrashCan` — a real SVG rendering of the same Font
                    // Awesome 6 "trash can" glyph the rest of the app uses via the `fa`/`fas` CSS
                    // classes (this project's own `@fortawesome/fontawesome-free` is v6.2.1, so
                    // this is the matching icon set, keeping the visual language consistent), not
                    // the CSS-class `<i>` version. Two separate problems ruled that CSS-class
                    // version out here: (1) `fa-trash` at this button's 20px size renders at a
                    // ~10.7px font size where its fine detail (lid, body grating lines) blurs
                    // together under anti-aliasing into a solid colored block rather than a
                    // recognizable shape (confirmed live: an isolated 40px test render of the same
                    // glyph looked correct, so this was a too-small-for-the-icon's-detail problem,
                    // not a broken font); (2) even the simpler `fa-times` glyph still rendered
                    // visibly off-center toward the top-left despite `getBoundingClientRect`
                    // reporting the `<i>` element's own box as centered — Font Awesome (like most
                    // icon fonts) positions glyphs relative to the font's em-box/baseline, not
                    // relative to the glyph's own visible ink, so a geometrically-centered box can
                    // still look visually uncentered. `react-icons`' SVG has neither problem: its
                    // `viewBox` is defined directly against the visible shape (so centering the
                    // element centers what's actually drawn), and being a real vector shape rather
                    // than a font glyph, its fine detail stays crisp at small sizes instead of
                    // anti-aliasing into mush.
                    // Bigger on touch (no `:hover` to reveal it gradually there, and a real
                    // finger needs a bigger target than a mouse cursor does) — direct feedback:
                    // 20px was too small to reliably tap on mobile Firefox.
                    icon={<FaTrashCan size={supportsHover ? 11 : 16} />}
                    size={supportsHover ? 20 : 30}
                    title={t("common.delete") ?? undefined}
                    onClick={() => setPendingDelete(page)}
                    style={{
                      borderRadius: "50%",
                      background: "rgba(0,0,0,0.55)",
                      color: "#e74c3c",
                      border: "none",
                      padding: "0.5px 0 0 1px",
                    }}
                  />
                </div>
                <div
                  style={{
                    fontSize: FONT_SIZE_SM,
                    marginTop: 2,
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                  }}
                  title={filename ?? undefined}
                >
                  {t("bookmarks.pageLabel", { page })}
                  {filename ? ` · ${filename}` : ""}
                </div>
              </a>
            )
          })}
        </div>
      </div>
      {pendingDelete !== null &&
        createPortal(
          // `Confirm` itself hardcodes `Z_OVERLAY_BACKDROP`/`Z_OVERLAY_CONTENT` (1000/1001) — well
          // below this grid's own `Z_OVERLAY_TOOLTIP` (1100), since `Confirm` is a general-purpose
          // component with no idea it might ever need to out-rank a tooltip-layer popup it's
          // opened from. `zIndex` only wins a stacking fight within the same stacking context, so
          // a plain wrapper div `position: relative` + a `zIndex` above `Z_OVERLAY_TOOLTIP`
          // establishes a new one that contains `Confirm`'s own fixed-positioned children,
          // lifting the whole dialog (backdrop included) above the grid without needing to change
          // `Confirm` itself (which stays correct for its other, non-tooltip callers).
          <div style={{ position: "relative", zIndex: Z_OVERLAY_TOOLTIP + 1 }}>
            <Confirm
              danger
              message={t("bookmarks.confirmRemoveBookmark", { page: pendingDelete, title: archiveTitle })}
              confirmLabel={t("common.delete") ?? undefined}
              onCancel={() => setPendingDelete(null)}
              onConfirm={() => {
                // `onClose()` fires from `mutate`'s own `onSuccess`, not right here — closing the
                // grid unmounts this whole component tree (including `removeBookmark` itself), so
                // firing that unmount in the same tick as `mutate()` risks racing the actual
                // network request on a slow enough device. Waiting for `onSuccess` guarantees the
                // request actually completed first. (This was a real hypothesis for a reported
                // mobile-Firefox failure, not a confirmed root cause — the server access log
                // showed zero DELETE requests ever arriving from that device at all, which this
                // fix alone doesn't explain; see the investigation for what's still open.)
                removeBookmark.mutate(
                  { archiveId, page: pendingDelete },
                  {
                    onSuccess: () => {
                      toast({
                        heading: t("bookmarks.bookmarkRemoved", { page: pendingDelete, title: archiveTitle }) ?? undefined,
                        icon: "success",
                      })
                      // Closes the whole preview, not just the confirm dialog — deleting a
                      // bookmark invalidates the `["bookmarks"]` query, which on `/bookmarks`
                      // itself re-sorts the archive list, very possibly moving this very card to a
                      // different position. `anchorRect` is a one-time snapshot of where the cover
                      // *was* when this grid first opened, so it doesn't track the card's new
                      // position — leaving the grid open would keep it floating disconnected from
                      // anything once the list re-renders around it.
                      onClose()
                    },
                    // `text: String(err)` — same "surface the real server-reported reason, not
                    // just a generic failure label" pattern `AiSmartTankoubonModal.tsx`'s own
                    // `catch` blocks use. `ApiError`'s own `message` is the response body's error
                    // text (`readErrorBody`, `api/client.ts`), so `String(err)` on it reads as
                    // that real reason rather than `"[object Object]"` or similar.
                    onError: (err) => {
                      toast({
                        heading: t("bookmarks.errorRemovingBookmark") ?? undefined,
                        text: String(err),
                        icon: "error",
                      })
                    },
                  },
                )
                setPendingDelete(null)
              }}
            />
          </div>,
          document.body,
        )}
      </div>
    </>,
    document.body,
  )
}
