import type { MouseEvent } from "react";
import { useEffect, useRef, useState } from "react";

import type { ArchiveMetadata, BookmarkedArchiveResponse } from "@/api/types";
import { useBookmarkHoverStore } from "@/bookmarkHoverStore";
import { useSupportsHover } from "@/hooks/useSupportsHover";
import { CarouselCard } from "@/pages/Library/CarouselCard";

import { BookmarkHoverGrid } from "./BookmarkHoverGrid";

/** Wraps `CarouselCard` with the bookmark hover-grid preview. On touch, tap toggles the preview
 * via `onClickCapture`; on hover-capable devices, scroll-driven re-anchor/close handles scrolling. */
export function BookmarkedArchiveHoverCard({
  entry,
  cropThumbs,
  onContextMenu,
  onOpen,
  onSearchTag,
  onWheelPassthrough,
  highlightQuery,
}: {
  entry: BookmarkedArchiveResponse;
  cropThumbs: boolean;
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void;
  onOpen: (id: string) => void;
  onSearchTag?: (namespacedTag: string) => void;
  /** Passed straight through to `BookmarkHoverGrid`'s own prop of the same name. */
  onWheelPassthrough?: (deltaX: number, deltaY: number) => void;
  highlightQuery?: string;
}) {
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [hoverRect, setHoverRect] = useState<DOMRect | null>(null);
  const supportsHover = useSupportsHover();
  const openArchiveId = useBookmarkHoverStore((s) => s.openArchiveId);
  const openInStore = useBookmarkHoverStore((s) => s.open);
  const closeInStore = useBookmarkHoverStore((s) => s.close);
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
        highlightQuery={highlightQuery}
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
          highlightQuery={highlightQuery}
        />
      )}
    </div>
  );
}
