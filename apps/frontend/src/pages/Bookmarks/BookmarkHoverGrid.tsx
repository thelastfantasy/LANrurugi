import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { FaPen, FaStamp, FaTrashCan } from "react-icons/fa6";
import { useNavigate } from "react-router-dom";

import { useBookmarksForArchive, useBookmarksForTankoubon, useRemoveBookmark, useSetBookmarkName } from "@/api/hooks";
import { Tooltip } from "@/components/common-ui/Display";
import { IconButton } from "@/components/common-ui/Form";
import { Confirm } from "@/components/Display";
import { promptDialog } from "@/dialog";
import { useSupportsHover } from "@/hooks/useSupportsHover";
import { routes } from "@/lib/routes";
import { highlightText } from "@/lib/utils/highlightText";
import { isTankoubonId } from "@/lib/utils/isTankoubonId";
import { matchesKeywords, splitKeywords } from "@/lib/utils/matchesKeywords";
import { FONT_SIZE_SM, Z_OVERLAY_BACKDROP, Z_OVERLAY_TOOLTIP } from "@/theme";
import { toast } from "@/toast";

import {
  sortPagesByOrder,
  useHoverGridPageOrder,
} from "./useHoverGridPageOrder";
import { useOnlyMatchingBookmarksPreference } from "./useOnlyMatchingBookmarks";

function cellSize(anchorRect: DOMRect) {
  return { width: anchorRect.width, height: anchorRect.height };
}

const BORDER_WIDTH = 1;
const FRAME_PADDING = 6;
const GRID_GAP = 6;
const MAX_COLUMNS = 3;
const MAX_ROWS = 3;
const EDGE_HANDOFF_DELAY_MS = 500;
/** Must be at least the real scrollbar width — under-reserving clips the last column. */
const SCROLLBAR_WIDTH_ESTIMATE = 12;
const CAPTION_HEIGHT = 20;
const TANK_CAPTION_HEIGHT = CAPTION_HEIGHT * 2;

/** Client-side pre-truncate only; the server's grapheme-accurate check is the real limit. */
const MAX_BOOKMARK_NAME_LEN = 200;

/** Never returns less than 1 — even a viewport too narrow for one cell still shows one column. */
function tracksThatFit(availablePx: number, trackPx: number): number {
  return Math.max(
    1,
    Math.floor((availablePx + GRID_GAP) / (trackPx + GRID_GAP)),
  );
}

/** Narrows below `maxColumns` only when a smaller count strictly reduces empty last-row cells.
 * Search floor is 2 — remainder counts alone would otherwise collapse everything to one column. */
function columnsWithFewestEmptyCells(
  count: number,
  maxColumns: number,
): number {
  if (count <= 1) return maxColumns;
  const remainderAt = (columns: number) =>
    columns * Math.ceil(count / columns) - count;
  let best = maxColumns;
  let bestRemainder = remainderAt(maxColumns);
  for (let columns = maxColumns - 1; columns >= 2; columns--) {
    const remainder = remainderAt(columns);
    if (remainder < bestRemainder) {
      best = columns;
      bestRemainder = remainder;
    }
  }
  return best;
}

/** Normalized render shape for both archive and Tankoubon sources; Tankoubon-only fields
 * (`local_page`/`archive_index`/`localArchiveId`) are always set together. */
interface Entry {
  page: number;
  filename: string | null;
  bookmarked_at: number;
  stamp_count: number;
  name: string | null;
  local_page?: number;
  local_pagecount?: number;
  archive_index?: number;
  localArchiveId?: string;
}

/** Hover preview expanding into a grid of bookmarked-page thumbnails (max 3×3, scrolling past
 * that), with one cell placed to exactly cover the card's own cover thumbnail (`anchorRect`). */
export function BookmarkHoverGrid({
  anchorRect,
  archiveId,
  archiveTitle,
  pages,
  onClose,
  onWheelPassthrough,
  highlightQuery,
}: {
  anchorRect: DOMRect;
  archiveId: string;
  archiveTitle: string;
  /** Ascending; `pages[0]` is the anchored page. */
  pages: number[];
  onClose: () => void;
  /** Forwards a wheel gesture the grid doesn't consume (both deltas, for replay onto Lenis). */
  onWheelPassthrough?: (deltaX: number, deltaY: number) => void;
  /** Space-separated keywords to `<mark>` inside each bookmark's own name. */
  highlightQuery?: string;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const supportsHover = useSupportsHover();
  const rootRef = useRef<HTMLDivElement>(null);
  const scrollElRef = useRef<HTMLDivElement>(null);

  const [expanded, setExpanded] = useState(false);
  useEffect(() => {
    const raf = requestAnimationFrame(() => setExpanded(true));
    return () => cancelAnimationFrame(raf);
  }, []);

  useEffect(() => {
    if (supportsHover) return;
    const prevOverflow = document.body.style.overflow;
    const prevOverscroll = document.body.style.overscrollBehavior;
    document.body.style.overflow = "hidden";
    document.body.style.overscrollBehavior = "none";
    return () => {
      document.body.style.overflow = prevOverflow;
      document.body.style.overscrollBehavior = prevOverscroll;
    };
  }, [supportsHover]);
  const isTank = isTankoubonId(archiveId);
  const singleBookmarkedPages = useBookmarksForArchive(isTank ? null : archiveId);
  const tankBookmarkedPages = useBookmarksForTankoubon(isTank ? archiveId : null);
  const bookmarkedPages: { data: Entry[] | undefined } = isTank
    ? {
        data: tankBookmarkedPages.data?.map((b) => ({
          page: b.page,
          filename: b.filename,
          bookmarked_at: b.bookmarked_at,
          stamp_count: b.stamp_count,
          name: b.name,
          local_page: b.local_page,
          local_pagecount: b.local_pagecount,
          archive_index: b.archive_index,
          localArchiveId: b.archive_id,
        })),
      }
    : singleBookmarkedPages;
  const removeBookmark = useRemoveBookmark();
  const setName = useSetBookmarkName();
  const [pendingDelete, setPendingDelete] = useState<number | null>(null);
  const [pageOrder] = useHoverGridPageOrder();
  const [onlyMatching] = useOnlyMatchingBookmarksPreference();
  const sortedPages = sortPagesByOrder(
    bookmarkedPages.data ??
      (isTank
        ? []
        : pages.map((page) => ({
            page,
            filename: null,
            bookmarked_at: 0,
            stamp_count: 0,
            name: null,
          }))),
    pageOrder,
  );
  // The archive/Tankoubon this grid belongs to was returned by `/bookmarks`'s own `q` filter
  // because *some* field matched — either its title/basename (want every bookmark shown, the
  // historical behavior) or only one of its individual bookmarks' own name/page number (want just
  // those shown). `/bookmarks`'s response only carries a card-level "matched" bool, not which
  // field — so this re-derives the same per-field check client-side, mirroring
  // `card_matches_query`'s own AND-across-keywords/OR-across-fields rule exactly.
  const keywords = splitKeywords(highlightQuery);
  const titleMatched = matchesKeywords([archiveTitle], keywords);
  const visiblePages =
    onlyMatching && keywords.length > 0 && !titleMatched
      ? sortedPages.filter((b) => matchesKeywords([b.name, String(b.page)], keywords))
      : sortedPages;
  const livePages = visiblePages.map((b) => b.page);
  const { width: cellWidth, height: cellHeight } = cellSize(anchorRect);
  const rowHeight = cellHeight + (isTank ? TANK_CAPTION_HEIGHT : CAPTION_HEIGHT);

  const [viewport, setViewport] = useState(() => ({
    width: window.innerWidth,
    height: window.innerHeight,
  }));
  useLayoutEffect(() => {
    function recompute() {
      setViewport({ width: window.innerWidth, height: window.innerHeight });
    }
    window.addEventListener("resize", recompute);
    return () => window.removeEventListener("resize", recompute);
  }, []);

  const frameOverhead = (FRAME_PADDING + BORDER_WIDTH) * 2;
  const spaceRight = viewport.width - anchorRect.left - frameOverhead;
  const spaceLeft = anchorRect.right - frameOverhead;
  const spaceBelow = viewport.height - anchorRect.top - frameOverhead;
  const spaceAbove = anchorRect.bottom - frameOverhead;

  const maxColumnsThatFit = Math.min(
    MAX_COLUMNS,
    tracksThatFit(Math.max(spaceLeft, spaceRight), cellWidth),
    livePages.length || 1,
  );
  const columns = columnsWithFewestEmptyCells(
    livePages.length,
    maxColumnsThatFit,
  );
  const rows = Math.min(
    MAX_ROWS,
    tracksThatFit(Math.max(spaceAbove, spaceBelow), rowHeight),
    Math.ceil(livePages.length / columns),
  );
  const willScroll = Math.ceil(livePages.length / columns) > rows;
  const gridWidth =
    columns * cellWidth +
    (columns - 1) * GRID_GAP +
    (willScroll ? SCROLLBAR_WIDTH_ESTIMATE : 0);
  const gridHeight = rows * rowHeight + (rows - 1) * GRID_GAP;
  const frameWidth = gridWidth + 2 * (FRAME_PADDING + BORDER_WIDTH);
  const frameHeight = gridHeight + 2 * (FRAME_PADDING + BORDER_WIDTH);

  const opensLeft = spaceLeft > spaceRight;
  const opensUp = spaceAbove > spaceBelow;

  function reorderForCorner(pagesToOrder: number[]): number[] {
    const rows: number[][] = [];
    for (let i = 0; i < pagesToOrder.length; i += columns) {
      rows.push(pagesToOrder.slice(i, i + columns));
    }
    if (opensLeft) rows.forEach((row) => row.reverse());
    if (opensUp) rows.reverse();
    return rows.flat();
  }
  const orderedPages = reorderForCorner(livePages);

  useLayoutEffect(() => {
    if (!opensUp || !willScroll) return;
    const el = scrollElRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight - el.clientHeight;
  }, [opensUp, willScroll, orderedPages.length]);

  function openPage(page: number) {
    navigate(`${routes.reader(archiveId)}?p=${page}`);
  }

  // Imperative non-passive listener — React's synthetic `onWheel` is passive, so `preventDefault`
  // there is a silent no-op. Edge-hit gestures pause `EDGE_HANDOFF_DELAY_MS` before handing off.
  const edgeState = useRef<{
    direction: "up" | "down";
    firstHitTime: number;
    forwarding: boolean;
  } | null>(null);
  useEffect(() => {
    const el = scrollElRef.current;
    if (!el || !onWheelPassthrough) return;
    function handleWheel(e: WheelEvent) {
      if (!onWheelPassthrough) return;
      if (!willScroll || e.deltaX !== 0) {
        e.preventDefault();
        onWheelPassthrough(e.deltaX, e.deltaY);
        return;
      }
      const el = e.currentTarget as HTMLDivElement;
      const direction = e.deltaY > 0 ? "down" : "up";
      const atBottom = el.scrollTop + el.clientHeight >= el.scrollHeight - 1;
      const atTop = el.scrollTop <= 0;
      const atEdgeForDirection =
        (direction === "down" && atBottom) || (direction === "up" && atTop);
      if (!atEdgeForDirection) {
        edgeState.current = null;
        return;
      }
      const state = edgeState.current;
      if (state && state.direction === direction && state.forwarding) {
        e.preventDefault();
        onWheelPassthrough(0, e.deltaY);
        return;
      }
      if (!state || state.direction !== direction) {
        edgeState.current = {
          direction,
          firstHitTime: performance.now(),
          forwarding: false,
        };
        e.preventDefault();
        return;
      }
      if (performance.now() - state.firstHitTime < EDGE_HANDOFF_DELAY_MS) {
        e.preventDefault();
        return;
      }
      edgeState.current = { ...state, forwarding: true };
      e.preventDefault();
      onWheelPassthrough(0, e.deltaY);
    }
    el.addEventListener("wheel", handleWheel, { passive: false });
    return () => el.removeEventListener("wheel", handleWheel);
  }, [onWheelPassthrough, willScroll]);

  function handleMouseLeave() {
    if (pendingDelete !== null) return;
    onClose();
  }

  return createPortal(
    <>
      {!supportsHover && (
        <div
          style={{
            position: "fixed",
            inset: 0,
            zIndex: Z_OVERLAY_BACKDROP,
            background: "rgba(0,0,0,0.4)",
          }}
          onClick={() => {
            if (pendingDelete !== null) return;
            onClose();
          }}
        />
      )}
      <div
        ref={rootRef}
        onMouseLeave={handleMouseLeave}
        style={{
          position: "absolute",
          top: opensUp
            ? anchorRect.top -
              (gridHeight - rowHeight) -
              FRAME_PADDING -
              BORDER_WIDTH +
              window.scrollY
            : anchorRect.top - FRAME_PADDING - BORDER_WIDTH + window.scrollY,
          left: opensLeft
            ? anchorRect.right -
              gridWidth -
              FRAME_PADDING -
              BORDER_WIDTH +
              window.scrollX
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
            transformOrigin: `${opensUp ? "bottom" : "top"} ${opensLeft ? "right" : "left"}`,
            transform: expanded
              ? "scale(1)"
              : `scale(${cellWidth / frameWidth}, ${cellHeight / frameHeight})`,
            opacity: expanded ? 1 : 0,
            transition: "transform 180ms ease-out, opacity 120ms ease-out",
          }}
        >
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
              overflowX: "hidden",
              ...(willScroll ? { scrollbarGutter: "stable" } : {}),
            }}
          >
            {orderedPages.map((page) => {
              const entry = bookmarkedPages.data?.find((b) => b.page === page);
              const filename = entry?.filename ?? null;
              const stampCount = entry?.stamp_count ?? 0;
              const thumbArchiveId = entry?.localArchiveId ?? archiveId;
              const thumbPage = entry?.local_page ?? page;
              return (
                <a
                  key={page}
                  href={`${routes.reader(archiveId)}?p=${page}`}
                  onClick={(e) => {
                    e.preventDefault();
                    openPage(page);
                  }}
                  style={{
                    display: "block",
                    position: "relative",
                    color: "inherit",
                    textDecoration: "none",
                  }}
                >
                  <div style={{ position: "relative" }}>
                    <img
                      src={`/api/archives/${thumbArchiveId}/thumbnail?page=${thumbPage}`}
                      alt=""
                      style={{
                        width: cellWidth,
                        height: cellHeight,
                        objectFit: "cover",
                        display: "block",
                      }}
                    />
                    {entry?.name && (
                      // Overlaid on the thumbnail (own `position: relative` wrapper above, not the
                      // outer `<a>`) instead of a caption line — a fixed-height row across the grid.
                      <Tooltip
                        label={highlightText(entry.name, highlightQuery)}
                        anchor="cursor"
                        wrapperStyle={{ position: "absolute", bottom: 0, left: 0, right: 0, display: "block" }}
                      >
                        <div
                          style={{
                            background: "rgba(0,0,0,0.55)",
                            color: "#fff",
                            fontSize: FONT_SIZE_SM,
                            fontWeight: 600,
                            padding: "2px 6px",
                            whiteSpace: "nowrap",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                          }}
                        >
                          {highlightText(entry.name, highlightQuery)}
                        </div>
                      </Tooltip>
                    )}
                  </div>
                  {stampCount > 0 && (
                    <div
                      style={{
                        position: "absolute",
                        top: 2,
                        left: 2,
                        display: "flex",
                      }}
                    >
                      <span
                        title={
                          t("bookmarks.pageHasStamps", { count: stampCount }) ??
                          undefined
                        }
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
                  <div
                    onClick={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                    }}
                    style={{
                      position: "absolute",
                      top: 2,
                      right: 2,
                      display: "flex",
                      gap: 2,
                    }}
                  >
                    <IconButton
                      icon={<FaPen size={supportsHover ? 10 : 14} />}
                      size={supportsHover ? 20 : 30}
                      title={t("bookmarks.editName") ?? undefined}
                      onClick={() => {
                        void promptDialog(
                          t("bookmarks.namePlaceholder", { max: MAX_BOOKMARK_NAME_LEN }) ?? "",
                          entry?.name ?? "",
                        ).then((result) => {
                          if (result === null) return;
                          const trimmed = result.trim().slice(0, MAX_BOOKMARK_NAME_LEN);
                          setName.mutate(
                            { archiveId, page, name: trimmed.length > 0 ? trimmed : null },
                            {
                              onError: (err) => {
                                toast({
                                  heading: t("bookmarks.errorRenamingBookmark") ?? undefined,
                                  text: String(err),
                                  icon: "error",
                                });
                              },
                            },
                          );
                        });
                      }}
                      style={{
                        borderRadius: "50%",
                        background: "rgba(0,0,0,0.55)",
                        color: "#fff",
                        border: "none",
                        padding: "0.5px 0 0 1px",
                      }}
                    />
                    <IconButton
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
                    style={{ fontSize: FONT_SIZE_SM, marginTop: 2 }}
                    title={filename ?? undefined}
                  >
                    {entry?.archive_index !== undefined && entry.local_page !== undefined ? (
                      <>
                        <div style={{ whiteSpace: "nowrap" }}>
                          {t("bookmarks.tankPageLabel", {
                            chapter: entry.archive_index + 1,
                            localPage: entry.local_page,
                            localPagecount: entry.local_pagecount,
                            globalPage: page,
                          })}
                        </div>
                        {filename && (
                          <div
                            style={{
                              whiteSpace: "nowrap",
                              overflow: "hidden",
                              textOverflow: "ellipsis",
                            }}
                          >
                            {filename}
                          </div>
                        )}
                      </>
                    ) : (
                      <div
                        style={{
                          whiteSpace: "nowrap",
                          overflow: "hidden",
                          textOverflow: "ellipsis",
                        }}
                      >
                        {t("bookmarks.pageLabel", { page })}
                        {filename ? ` · ${filename}` : ""}
                      </div>
                    )}
                  </div>
                </a>
              );
            })}
          </div>
        </div>
        {pendingDelete !== null &&
          createPortal(
            <div
              style={{ position: "relative", zIndex: Z_OVERLAY_TOOLTIP + 1 }}
            >
              <Confirm
                danger
                message={t("bookmarks.confirmRemoveBookmark", {
                  page: pendingDelete,
                  title: archiveTitle,
                })}
                confirmLabel={t("common.delete") ?? undefined}
                onCancel={() => setPendingDelete(null)}
                onConfirm={() => {
                  removeBookmark.mutate(
                    { archiveId, page: pendingDelete },
                    {
                      onSuccess: () => {
                        toast({
                          heading:
                            t("bookmarks.bookmarkRemoved", {
                              page: pendingDelete,
                              title: archiveTitle,
                            }) ?? undefined,
                          icon: "success",
                        });
                        onClose();
                      },
                      onError: (err) => {
                        toast({
                          heading:
                            t("bookmarks.errorRemovingBookmark") ?? undefined,
                          text: String(err),
                          icon: "error",
                        });
                      },
                    },
                  );
                  setPendingDelete(null);
                }}
              />
            </div>,
            document.body,
          )}
      </div>
    </>,
    document.body,
  );
}
