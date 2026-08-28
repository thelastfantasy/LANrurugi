import type { MouseEvent } from "react";
import { useEffect, useRef, useState } from "react";

import type { ArchiveMetadata, BookmarkedArchiveResponse } from "@/api/types";
import { useBookmarkHoverStore } from "@/bookmarkHoverStore";
import { useSupportsHover } from "@/hooks/useSupportsHover";
import { CarouselCard } from "@/pages/Library/CarouselCard";

import { BookmarkHoverGrid } from "./BookmarkHoverGrid";

/** Wraps `CarouselCard` with the bookmark hover-grid preview — kept out of `CarouselCard`/
 * `ArchiveCard` themselves (both reused everywhere across the app: Library grid, compact table,
 * every other carousel mode) so those general-purpose components never need to know whether an
 * archive happens to have bookmarks. Only the two call sites that actually render bookmarked
 * archives (the `/bookmarks` page and the carousel's own "bookmark" mode) wrap with this instead.
 *
 * Anchors the hover grid via `wrapperRef.current.querySelector(".id3")` rather than
 * `document.getElementById(archive.arcid)` — `ArchiveCard`'s root carries `id={id}`, a *global*
 * DOM id, which would collide if the same archive is ever rendered twice on the same page (e.g.
 * the bookmark carousel and the Library grid below it both showing the same book). Scoping the
 * lookup to this instance's own DOM subtree avoids that entirely.
 *
 * On a touch-only device (`useSupportsHover()` false — phone, tablet with no attached mouse)
 * `mouseenter`/`mouseleave` either never fire or fire in a way that doesn't match a genuine
 * "hovering to preview" intent, so tapping the cover there instead toggles the same preview via
 * `onClickCapture` (capture phase, so it runs *before* `CarouselCard`'s own inner `<a
 * onClick={handleOpen}>` on the cover would otherwise navigate to the reader) —
 * `preventDefault`/`stopPropagation` on a tap inside `.id3` swaps "open the reader" for "show the
 * bookmark grid" there. The *title* link (`.id2`, not `.id3`) is deliberately left alone — it's
 * the only way a touch user reaches the reader directly from this card, since the cover itself no
 * longer does.
 *
 * On touch, "which preview is open" is tracked in `useBookmarkHoverStore` (a global singleton,
 * not local state) rather than this component's own `useState` — there's no `mouseleave`
 * equivalent on touch to close a previously-opened preview, so tapping a second card's cover
 * without a shared mutex would leave the first card's `BookmarkHoverGrid` open forever, stacking
 * multiple overlays. `open`ing this archive implicitly closes whichever one was open before (the
 * store only ever tracks one at a time).
 *
 * On a hover-capable device, `hoverRect`/`onMouseEnter`/`onMouseLeave` decide visibility the same
 * as before, plus a scroll-driven re-anchor/close: `RecentlyAddedCarousel`'s horizontal strip can
 * scroll (Lenis) while the cursor stays put, which real `mouseleave` doesn't fire for (the DOM
 * element under a stationary cursor doesn't change just because it moved via `scrollLeft`), so a
 * `hoverRect` captured once at `mouseenter` used to keep the preview floating in its original
 * screen position long after the actual thumbnail had scrolled out from underneath it — reported
 * live, 2026-08-28. Fixed by listening for the nearest `[data-scroll-container]` ancestor's own
 * native `scroll` event (Lenis drives real `scrollLeft` on that element, so it fires normally) and,
 * on each tick, re-measuring the anchor (`.id3`) against the last-known cursor position
 * (`lastMouseRef`, updated on every `mouseenter`/`mousemove` over this card, deliberately a plain
 * ref rather than state so mouse movement alone never triggers a render): still under the cursor →
 * re-anchor the preview to the thumbnail's new position; no longer under it → close. This
 * intentionally does *not* close unconditionally on every scroll tick — only once the cursor is
 * actually no longer over the thumbnail, matching what a real `mouseleave` would do if the browser
 * fired one for a scroll-driven move (the feature's own stated requirement). */
export function BookmarkedArchiveHoverCard({
  entry,
  cropThumbs,
  onContextMenu,
  onOpen,
  onSearchTag,
  onWheelPassthrough,
}: {
  entry: BookmarkedArchiveResponse;
  cropThumbs: boolean;
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void;
  onOpen: (id: string) => void;
  onSearchTag?: (namespacedTag: string) => void;
  /** Passed straight through to `BookmarkHoverGrid`'s own prop of the same name — see its docs
   * there. Only `RecentlyAddedCarousel`'s bookmark-mode call site provides this. */
  onWheelPassthrough?: (deltaX: number, deltaY: number) => void;
}) {
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [hoverRect, setHoverRect] = useState<DOMRect | null>(null);
  const supportsHover = useSupportsHover();
  const openArchiveId = useBookmarkHoverStore((s) => s.openArchiveId);
  const openInStore = useBookmarkHoverStore((s) => s.open);
  const closeInStore = useBookmarkHoverStore((s) => s.close);
  // Last-known cursor position while hovering this card, refreshed on every `mouseenter`/
  // `mousemove` — read (not subscribed to) by the scroll-driven re-anchor/close check below, so
  // moving the mouse itself never triggers a re-render on its own (a plain ref, not state).
  const lastMouseRef = useRef<{ x: number; y: number } | null>(null);

  function handleMouseEnter(e: MouseEvent) {
    if (!supportsHover) return;
    lastMouseRef.current = { x: e.clientX, y: e.clientY };
    const cover = wrapperRef.current?.querySelector<HTMLElement>(".id3");
    if (cover) setHoverRect(cover.getBoundingClientRect());
  }

  function handleClickCapture(e: MouseEvent) {
    if (supportsHover) return;
    if (!(e.target as HTMLElement).closest(".id3")) return;
    e.preventDefault();
    e.stopPropagation();
    const cover = wrapperRef.current?.querySelector<HTMLElement>(".id3");
    if (openArchiveId === entry.archive.arcid) {
      closeInStore();
    } else if (cover) {
      setHoverRect(cover.getBoundingClientRect());
      openInStore(entry.archive.arcid);
    }
  }

  // Re-anchors (still under the cursor) or closes (cursor no longer over the thumbnail) whenever
  // the nearest scrollable ancestor actually scrolls — see this component's own top-level docs for
  // why `mouseleave` alone can't catch a scroll-driven move. Only wired up on a hover-capable
  // device with a currently-open preview; a `{ passive: true }` listener on the scroll container
  // itself, not `window` — cheaper (only fires for scrolls that could plausibly affect this card)
  // and correct even if this card is nested inside more than one scrollable ancestor (`closest`
  // finds the nearest one, which is what's actually moving the thumbnail).
  useEffect(() => {
    if (!supportsHover || hoverRect === null) return;
    const scrollParent = wrapperRef.current?.closest<HTMLElement>(
      "[data-scroll-container]",
    );
    const cover = wrapperRef.current?.querySelector<HTMLElement>(".id3");
    if (!scrollParent || !cover) return;
    function handleScroll() {
      const mouse = lastMouseRef.current;
      if (!mouse || !cover) return;
      const rect = cover.getBoundingClientRect();
      const stillOver =
        mouse.x >= rect.left &&
        mouse.x <= rect.right &&
        mouse.y >= rect.top &&
        mouse.y <= rect.bottom;
      if (stillOver) {
        setHoverRect(rect);
      } else {
        setHoverRect(null);
      }
    }
    scrollParent.addEventListener("scroll", handleScroll, { passive: true });
    return () => scrollParent.removeEventListener("scroll", handleScroll);
  }, [supportsHover, hoverRect]);

  const visible =
    hoverRect !== null &&
    (supportsHover || openArchiveId === entry.archive.arcid);

  return (
    <div
      ref={wrapperRef}
      onMouseEnter={handleMouseEnter}
      onMouseMove={(e) => {
        if (supportsHover)
          lastMouseRef.current = { x: e.clientX, y: e.clientY };
      }}
      onMouseLeave={() => {
        if (supportsHover) setHoverRect(null);
      }}
      onClickCapture={handleClickCapture}
    >
      <CarouselCard
        archive={entry.archive}
        cropThumbs={cropThumbs}
        onContextMenu={onContextMenu}
        onOpen={onOpen}
        onSearchTag={onSearchTag}
      />
      {visible && hoverRect && (
        <BookmarkHoverGrid
          anchorRect={hoverRect}
          archiveId={entry.archive.arcid}
          archiveTitle={entry.archive.title}
          pages={entry.pages}
          onClose={() => {
            setHoverRect(null);
            if (!supportsHover) closeInStore();
          }}
          onWheelPassthrough={onWheelPassthrough}
        />
      )}
    </div>
  );
}
