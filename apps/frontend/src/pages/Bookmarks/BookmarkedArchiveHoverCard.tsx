import type { MouseEvent } from "react"
import { useRef, useState } from "react"

import type { ArchiveMetadata, BookmarkedArchiveResponse } from "@/api/types"
import { useBookmarkHoverStore } from "@/bookmarkHoverStore"
import { useSupportsHover } from "@/hooks/useSupportsHover"
import { CarouselCard } from "@/pages/Library/CarouselCard"

import { BookmarkHoverGrid } from "./BookmarkHoverGrid"

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
 * store only ever tracks one at a time). On a hover-capable device none of this global-store logic
 * runs at all — `hoverRect`/`onMouseEnter`/`onMouseLeave` are unchanged from before this store
 * existed. */
export function BookmarkedArchiveHoverCard({
  entry,
  cropThumbs,
  onContextMenu,
  onOpen,
  onSearchTag,
  onWheelPassthrough,
}: {
  entry: BookmarkedArchiveResponse
  cropThumbs: boolean
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  onSearchTag?: (namespacedTag: string) => void
  /** Passed straight through to `BookmarkHoverGrid`'s own prop of the same name — see its docs
   * there. Only `RecentlyAddedCarousel`'s bookmark-mode call site provides this. */
  onWheelPassthrough?: (deltaX: number, deltaY: number) => void
}) {
  const wrapperRef = useRef<HTMLDivElement>(null)
  const [hoverRect, setHoverRect] = useState<DOMRect | null>(null)
  const supportsHover = useSupportsHover()
  const openArchiveId = useBookmarkHoverStore((s) => s.openArchiveId)
  const openInStore = useBookmarkHoverStore((s) => s.open)
  const closeInStore = useBookmarkHoverStore((s) => s.close)

  function handleMouseEnter() {
    if (!supportsHover) return
    const cover = wrapperRef.current?.querySelector<HTMLElement>(".id3")
    if (cover) setHoverRect(cover.getBoundingClientRect())
  }

  function handleClickCapture(e: MouseEvent) {
    if (supportsHover) return
    if (!(e.target as HTMLElement).closest(".id3")) return
    e.preventDefault()
    e.stopPropagation()
    const cover = wrapperRef.current?.querySelector<HTMLElement>(".id3")
    if (openArchiveId === entry.archive.arcid) {
      closeInStore()
    } else if (cover) {
      setHoverRect(cover.getBoundingClientRect())
      openInStore(entry.archive.arcid)
    }
  }

  // On a hover-capable device, `hoverRect` alone decides visibility (original behavior,
  // untouched). On touch, the global store's `openArchiveId` is the actual source of truth — a
  // tap elsewhere closing this card's preview (via `closeInStore()`, e.g. from
  // `BookmarkHoverGrid`'s own outside-click handler) needs to hide it even though this
  // component's own `hoverRect` state hasn't been cleared.
  const visible = hoverRect !== null && (supportsHover || openArchiveId === entry.archive.arcid)

  return (
    <div
      ref={wrapperRef}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={() => {
        if (supportsHover) setHoverRect(null)
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
            setHoverRect(null)
            if (!supportsHover) closeInStore()
          }}
          onWheelPassthrough={onWheelPassthrough}
        />
      )}
    </div>
  )
}
