import { useQuery } from "@tanstack/react-query";
import Lenis from "lenis";
import type { MouseEvent } from "react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { IconType } from "react-icons";
import {
  FaArrowsRotate,
  FaBook,
  FaBookmark,
  FaCaretDown,
  FaCheckDouble,
  FaChevronLeft,
  FaChevronRight,
  FaCompress,
  FaDice,
  FaEject,
  FaEllipsis,
  FaHammer,
  FaInbox,
  FaSquareCheck,
  FaTag,
} from "react-icons/fa6";

import { fetchJson } from "@/api/client";
import { useInfiniteBookmarks } from "@/api/hooks";
import type { ArchiveMetadata, SearchResponse } from "@/api/types";
import { Menu, MenuItem, SortableList } from "@/components/common-ui/Display";
import { IconButton } from "@/components/common-ui/Form";
import { NEW_ONLY, UNTAGGED_ONLY } from "@/lib/constants";
import { CAROUSEL_OPEN_KEY, CAROUSEL_TYPE_KEY } from "@/lib/storageKeys";
import { BookmarkedArchiveHoverCard } from "@/pages/Bookmarks/BookmarkedArchiveHoverCard";

import { CarouselCard } from "./CarouselCard";
import { SelectedArchiveSlideContent } from "./SelectedArchiveSlideContent";
import { type CarouselMode } from "./types";

/** A `Lenis` instance mirrored into both a ref (for event handlers — `onClick`'s own `scrollTo`
 * calls need a value that's always current without re-subscribing) and state (a plain ref write
 * doesn't trigger a re-render, but `useCarouselOverflow` needs to know exactly when this component
 * gets a new instance, or loses one, so it can (re)subscribe its own resize/scroll listeners).
 * Returns `[ref, instance, setter]` — pass `setter` to whatever creates/destroys the real `Lenis`
 * (a callback ref, or `SortableList`'s own `onScroller`). */
function useLenisMirror(): [React.RefObject<Lenis | null>, Lenis | null, (lenis: Lenis | null) => void] {
  const ref = useRef<Lenis | null>(null);
  const [instance, setInstance] = useState<Lenis | null>(null);
  const set = useCallback((lenis: Lenis | null) => {
    ref.current = lenis;
    setInstance(lenis);
  }, []);
  return [ref, instance, set];
}

/** Whether a horizontally-scrolling container can currently scroll left/right — re-measured on
 * every resize of either the container itself or its content (`ResizeObserver`, not a fixed
 * item-count threshold), so a narrow phone viewport shows the arrows past 2 cards while a wide
 * desktop one only shows them past 5+, and rotating/resizing the window re-evaluates live rather
 * than needing a reload. Also tracks the live scroll position so, e.g., the left arrow disappears
 * once already scrolled all the way there instead of staying visible but inert.
 *
 * Takes the `Lenis` instance rather than a raw element ref: both carousels already expose theirs
 * (`lenisRef`/`scrollerRef`) for the arrow buttons' own `scrollTo` calls, and `lenis.options.wrapper`
 * is guaranteed to be *whatever node Lenis is currently actually bound to* — reusing it here means
 * this hook can never independently drift out of sync with the scroll behavior it's describing. */
const NO_OVERFLOW = { canScrollLeft: false, canScrollRight: false };

/** `itemsSignal` is only ever read for its identity, never its contents — it exists purely to
 * force this effect to re-run (and thus re-`observe` the container's *current* children) when the
 * item set changes without `el` itself changing, e.g. switching carousel modes in place. Without
 * it, children added after first mount are never individually observed, so a resize caused purely
 * by items being added/removed (as opposed to an existing child's own box changing) can be missed. */
function useCarouselOverflow(lenis: Lenis | null, itemsSignal: unknown) {
  const [state, setState] = useState(NO_OVERFLOW);
  const el = lenis?.options.wrapper instanceof HTMLElement ? lenis.options.wrapper : null;

  useEffect(() => {
    if (!el) return;
    const EPSILON = 1; // sub-pixel rounding shouldn't flicker the arrows in and out
    const measure = () => {
      setState({
        canScrollLeft: el.scrollLeft > EPSILON,
        canScrollRight: el.scrollLeft + el.clientWidth < el.scrollWidth - EPSILON,
      });
    };
    measure();
    const resizeObserver = new ResizeObserver(measure);
    resizeObserver.observe(el);
    for (const child of el.children) resizeObserver.observe(child);
    el.addEventListener("scroll", measure, { passive: true });
    return () => {
      resizeObserver.disconnect();
      el.removeEventListener("scroll", measure);
    };
  }, [el, itemsSignal]);

  return el ? state : NO_OVERFLOW;
}

/** One icon component per `CarouselMode` — moved out of `lib/constants.ts` (a plain `.ts` file,
 * can't hold JSX) since this is the only consumer. Was previously a `Record<CarouselMode,
 * string>` of literal emoji characters fed into `` `fa ${...}` `` — a real, previously-unnoticed
 * bug: `"fa 📚"` isn't a valid Font Awesome class, so neither the `fa` base class nor the emoji
 * ever rendered anything (confirmed live via `getComputedStyle`: empty `<i>`, no `::before`
 * content). */
const CAROUSEL_MODE_ICON: Record<CarouselMode, IconType> = {
  ondeck: FaBook,
  random: FaDice,
  inbox: FaInbox,
  untagged: FaTag,
  bookmark: FaBookmark,
}

export function RecentlyAddedCarousel({
  filter,
  category,
  hideCompleted,
  groupbyTanks,
  cropThumbs,
  loggedIn,
  onContextMenu,
  onOpen,
  multiSelect,
  selectedIds,
  onToggleSelected,
  onReorderSelection,
  onSelectPage,
  onClearSelection,
  onRunBatch,
  onMerge,
  canMerge,
  onSearchTag,
}: {
  filter: string;
  category: string;
  hideCompleted: boolean;
  groupbyTanks: boolean;
  cropThumbs: boolean;
  /** Gates the "Bookmarked" carousel mode — an unauthenticated guest has no personal bookmarks. */
  loggedIn: boolean;
  onContextMenu: (
    e: MouseEvent,
    archive: ArchiveMetadata,
    source: "carousel",
  ) => void;
  onOpen: (id: string) => void;
  multiSelect: boolean;
  selectedIds: string[];
  onToggleSelected: (id: string) => void;
  /** Drag-to-reorder in the selection list — the new order becomes the merged Tankoubon's volume order. */
  onReorderSelection: (newOrder: string[]) => void;
  onSelectPage: () => void;
  onClearSelection: () => void;
  onRunBatch: () => void;
  onMerge: () => void;
  canMerge: boolean;
  onSearchTag: (namespacedTag: string) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(
    () => localStorage.getItem(CAROUSEL_OPEN_KEY) === "1",
  );
  const [storedMode, setMode] = useState<CarouselMode>(
    () =>
      (localStorage.getItem(CAROUSEL_TYPE_KEY) as CarouselMode | null) ??
      "ondeck",
  );
  const mode = !loggedIn && storedMode === "bookmark" ? "ondeck" : storedMode;
  const carouselRef = useRef<HTMLDivElement | null>(null);
  const [lenisRef, lenisInstance, setLenisInstance] = useLenisMirror();
  // A callback ref (not a plain `useRef` + `useEffect([items])`) because this div's own mount
  // state doesn't track `items` at all — switching into/out of selection mode (`multiSelect`)
  // unmounts and remounts this exact div without `items` ever changing, which previously left
  // `lenisRef.current` bound to a detached, since-orphaned node while the real one went
  // unbound — every arrow click was silently clamped to a stale `limit: 0`. A callback ref fires
  // exactly on this node's own mount/unmount, whatever the reason, so it can never miss one.
  const carouselCallbackRef = useCallback((el: HTMLDivElement | null) => {
    lenisRef.current?.destroy();
    carouselRef.current = el;
    if (!el) {
      setLenisInstance(null);
      return;
    }
    setLenisInstance(
      new Lenis({
        wrapper: el,
        content: el,
        orientation: "horizontal",
        gestureOrientation: "both",
        wheelMultiplier: 4.5,
        lerp: 0.1,
        autoRaf: true,
      }),
    );
  }, [lenisRef, setLenisInstance]);
  const stepSlide = useCallback(() => {
    const firstChild = carouselRef.current
      ?.firstElementChild as HTMLElement | null;
    return firstChild ? firstChild.getBoundingClientRect().width + 8 : 236;
  }, []);
  // Selection mode renders its own scroll container inside `SortableList` rather than the
  // `carouselRef` div above, so prev/next arrows there need their own ref pair.
  const selectionListRef = useRef<HTMLDivElement>(null);
  const [selectionLenisRef, selectionLenisInstance, setSelectionLenisInstance] = useLenisMirror();
  const selectionOverflow = useCarouselOverflow(selectionLenisInstance, selectedIds);
  const stepSelectionSlide = useCallback(() => {
    // `selectionListRef` > `SortableList`'s own scroll container > one `SortableRow` wrapper
    // div > the `.carousel-slide` this component's own `renderItem` returns.
    const firstChild = selectionListRef.current?.querySelector(
      ":scope > div > div > .carousel-slide",
    ) as HTMLElement | null;
    return firstChild ? firstChild.getBoundingClientRect().width + 8 : 236;
  }, []);
  const handleBookmarkWheelPassthrough = useCallback(
    (deltaX: number, deltaY: number) => {
      const el = carouselRef.current;
      if (!el) return;
      el.dispatchEvent(
        new WheelEvent("wheel", {
          deltaX,
          deltaY,
          deltaMode: 0,
          bubbles: true,
          cancelable: true,
        }),
      );
    },
    [],
  );

  useEffect(() => {
    localStorage.setItem(CAROUSEL_OPEN_KEY, open ? "1" : "0");
  }, [open]);

  useEffect(() => {
    localStorage.setItem(CAROUSEL_TYPE_KEY, storedMode);
  }, [storedMode]);

  const isOpen = open || multiSelect;

  const params = new URLSearchParams();
  if (filter) params.set("filter", filter);
  const isBuiltinSelector = category === NEW_ONLY || category === UNTAGGED_ONLY;
  if (category && !isBuiltinSelector) params.set("category", category);
  if (!groupbyTanks) params.set("groupby_tanks", "false");
  if (hideCompleted) params.set("hidecompleted", "true");
  if (category === NEW_ONLY) params.set("newonly", "true");
  if (category === UNTAGGED_ONLY) params.set("untaggedonly", "true");

  const isRandom = mode === "random";
  const isBookmarkMode = mode === "bookmark";
  const modeParams = new URLSearchParams(params);
  let path: string;
  switch (mode) {
    case "random":
      modeParams.set("count", "15");
      path = `/search/random?${modeParams.toString()}`;
      break;
    case "inbox":
      modeParams.set("newonly", "true");
      modeParams.set("sortby", "date_added");
      modeParams.set("order", "desc");
      modeParams.set("start", "-1");
      path = `/search?${modeParams.toString()}`;
      break;
    case "untagged":
      modeParams.set("untaggedonly", "true");
      modeParams.set("sortby", "date_added");
      modeParams.set("order", "desc");
      modeParams.set("start", "-1");
      path = `/search?${modeParams.toString()}`;
      break;
    case "bookmark":
      path = "";
      break;
    default:
      modeParams.set("sortby", "lastread");
      modeParams.set("hidecompleted", "true");
      path = `/search?${modeParams.toString()}`;
      break;
  }

  const carouselQuery = useQuery({
    queryKey: isRandom
      ? ["search", "random", modeParams.toString()]
      : ["search", { filter, category, mode, hideCompleted, groupbyTanks }],
    queryFn: () => fetchJson<SearchResponse>(path),
    enabled: isOpen && !multiSelect && !isBookmarkMode,
  });
  const bookmarksQuery = useInfiniteBookmarks("bookmarked_at", undefined, loggedIn);
  const bookmarkEntries = bookmarksQuery.data?.pages[0]?.entries ?? [];
  const items: ArchiveMetadata[] = useMemo(
    () =>
      isBookmarkMode
        ? (bookmarksQuery.data?.pages[0]?.entries ?? []).map(
            (entry) => entry.archive,
          )
        : (carouselQuery.data?.data ?? []),
    [isBookmarkMode, bookmarksQuery.data, carouselQuery.data],
  );
  const loading = isBookmarkMode
    ? bookmarksQuery.isLoading
    : carouselQuery.isLoading;
  const carouselOverflow = useCarouselOverflow(lenisInstance, items);

  // Lenis drives real `scrollLeft` on `el` (not a `transform`), so native `scroll` events still
  // fire — `BookmarkedArchiveHoverCard`'s `[data-scroll-container]` listener relies on this. Its
  // actual creation/teardown lives in `carouselCallbackRef` above, not a `useEffect` here — see
  // that ref's own comment for why.

  // Lenis's own `ResizeObserver` watches `wrapper`/`content`'s *own* border-box — here they're
  // the same flex element, whose own size never changes as children come and go (`overflow:
  // auto` keeps its box fixed size; only its *scrollWidth* grows). Switching carousel modes
  // (e.g. "新档案", 5 items → "展板", 45 items) without this div ever unmounting therefore left
  // Lenis's cached `dimensions.scrollWidth`/`limit` stuck at the old, smaller mode's value —
  // confirmed live: `lenis.limit` stayed `21` (the 5-item mode's real max-scroll) after switching
  // back to a 45-item mode whose real `scrollWidth - clientWidth` was `9461`. Lenis does expose a
  // manual `resize()` for exactly this gap; call it whenever the actual item set changes.
  useEffect(() => {
    lenisRef.current?.resize();
  }, [items, lenisRef]);

  const modeLabel: Record<CarouselMode, string> = {
    ondeck: t("library.onDeck"),
    random: t("library.random"),
    inbox: t("library.newArchives"),
    untagged: t("library.untaggedArchives"),
    bookmark: t("library.bookmarked"),
  };

  const ModeIcon = CAROUSEL_MODE_ICON[mode];

  return (
    <ul className="collapsible index-carousel with-right-caret">
      <li
        className="option-flyout"
        style={{
          display: "flex",
          flexWrap: "wrap",
          justifyContent: "space-between",
        }}
      >
        <div
          className="collapsible-title"
          onClick={() => setOpen((o) => !o)}
          style={{
            display: "flex",
            alignItems: "center",
            overflow: "hidden",
          }}
        >
          {multiSelect ? (
            <FaSquareCheck size={16} aria-hidden="true" />
          ) : (
            <ModeIcon size={16} aria-hidden="true" />
          )}
          <div style={{ marginLeft: 8 }}>
            {multiSelect ? t("app.selection") : modeLabel[mode]}
          </div>
          <FaCaretDown
            size={24}
            style={{
              marginLeft: 6,
              transform: isOpen ? "translateY(2px) rotate(180deg)" : "translateY(-1px)",
              transition: "transform 0.2s ease",
            }}
          />
        </div>
        {isOpen && multiSelect && (
          <div
            className="collapsible-right"
            onClick={(e) => e.stopPropagation()}
          >
            {selectedIds.length > 0 && (
              <span>{t("library.selected", { n: selectedIds.length })}</span>
            )}
            {selectedIds.length > 0 && (
              <IconButton
                variant="ghost-btn"
                icon={<FaHammer size={18} />}
                size={28}
                style={{ marginLeft: 12 }}
                title={t("library.runBatchOperationsOnSelection") ?? undefined}
                onClick={onRunBatch}
              />
            )}
            {canMerge && (
              <IconButton
                variant="ghost-btn"
                icon={<FaCompress size={18} />}
                size={28}
                style={{ marginLeft: 12 }}
                title={t("library.mergeArchivesIntoTankoubon") ?? undefined}
                onClick={onMerge}
              />
            )}
            {selectedIds.length > 0 && (
              <IconButton
                variant="ghost-btn"
                icon={<FaEject size={18} />}
                size={28}
                style={{ marginLeft: 12 }}
                title={t("library.clearSelection") ?? undefined}
                onClick={onClearSelection}
              />
            )}
            <IconButton
              variant="ghost-btn"
              icon={<FaCheckDouble size={18} />}
              size={28}
              style={{ marginLeft: 12 }}
              title={t("library.selectAllInPage") ?? undefined}
              onClick={onSelectPage}
            />
          </div>
        )}
        {isOpen && !multiSelect && (
          <div
            className="collapsible-right"
            onClick={(e) => e.stopPropagation()}
          >
            <IconButton
              variant="ghost-btn"
              icon={<FaArrowsRotate size={18} className={loading ? "fa-spin" : undefined} />}
              size={28}
              title={t("library.refresh") ?? undefined}
              onClick={() => {
                if (isBookmarkMode) void bookmarksQuery.refetch();
                else void carouselQuery.refetch();
              }}
            />
            <Menu
              trigger={
                <IconButton
                  variant="ghost-btn"
                  icon={<FaEllipsis size={18} />}
                  size={28}
                  style={{ marginLeft: 4 }}
                  title={t("library.carouselMode") ?? undefined}
                />
              }
            >
              {(loggedIn
                ? (["ondeck", "random", "inbox", "untagged", "bookmark"] as CarouselMode[])
                : (["ondeck", "random", "inbox", "untagged"] as CarouselMode[])
              ).map((m) => {
                const ModeIcon = CAROUSEL_MODE_ICON[m]
                return (
                  <MenuItem key={m} onClick={() => setMode(m)}>
                    <span style={{ display: "inline-flex", alignItems: "center", gap: 6, fontWeight: m === mode ? "bold" : undefined }}>
                      <ModeIcon size={14} aria-hidden="true" />
                      {modeLabel[m]}
                    </span>
                  </MenuItem>
                )
              })}
            </Menu>
          </div>
        )}
        {isOpen && multiSelect && (
          <div
            className="collapsible-body"
            style={{ width: "100%", boxSizing: "border-box" }}
          >
            {selectedIds.length === 0 ? (
              <div style={{ padding: "8px 0" }}>
                <div
                  className="id1"
                  style={{ width: "100%", boxSizing: "border-box" }}
                >
                  <div className="id2"></div>
                  <div
                    className="id3"
                    style={{
                      display: "flex",
                      flexDirection: "column",
                      alignItems: "center",
                      justifyContent: "center",
                    }}
                  >
                    <i className="fa fa-glasses fa-4x" aria-hidden="true"></i>
                    <span style={{ marginTop: 12 }}>
                      {t("library.clickArchivesToAddThem")}
                    </span>
                  </div>
                  <div className="id4"></div>
                </div>
              </div>
            ) : (
              <div style={{ padding: "8px 0", position: "relative" }} ref={selectionListRef}>
                <SortableList
                  items={selectedIds}
                  getId={(id) => id}
                  direction="horizontal"
                  onReorder={onReorderSelection}
                  onScroller={setSelectionLenisInstance}
                  renderItem={(id, dragHandleProps) => (
                    <div
                      {...dragHandleProps.attributes}
                      {...dragHandleProps.listeners}
                      className="carousel-slide"
                      style={{
                        marginRight: 8,
                        cursor: dragHandleProps.isDragging
                          ? "grabbing"
                          : "grab",
                      }}
                    >
                      <SelectedArchiveSlideContent
                        id={id}
                        cropThumbs={cropThumbs}
                        onContextMenu={(e, archive) =>
                          onContextMenu(e, archive, "carousel")
                        }
                        onRemove={onToggleSelected}
                      />
                    </div>
                  )}
                />
                {selectionOverflow.canScrollLeft && (
                  <IconButton
                    className="carousel-prev"
                    icon={<FaChevronLeft size={24} />}
                    size={32}
                    style={{
                      position: "absolute",
                      left: 0,
                      top: "50%",
                      transform: "translateY(-50%)",
                      zIndex: 20,
                    }}
                    onClick={() => {
                      const lenis = selectionLenisRef.current;
                      if (!lenis) return;
                      // Re-measure right before scrolling — Lenis's own `ResizeObserver`-driven
                      // remeasure can still be pending (see `SortableList`'s own `resize()` call
                      // for the full story) in the narrow window right after this arrow first
                      // appears, so a `scrollTo` immediately after selecting items could otherwise
                      // clamp against a stale, too-small `limit`.
                      lenis.resize();
                      lenis.scrollTo(lenis.targetScroll - stepSelectionSlide());
                    }}
                  />
                )}
                {selectionOverflow.canScrollRight && (
                  <IconButton
                    className="carousel-next"
                    icon={<FaChevronRight size={24} />}
                    size={32}
                    style={{
                      position: "absolute",
                      right: 0,
                      top: "50%",
                      transform: "translateY(-50%)",
                      zIndex: 20,
                    }}
                    onClick={() => {
                      const lenis = selectionLenisRef.current;
                      if (!lenis) return;
                      lenis.resize();
                      lenis.scrollTo(lenis.targetScroll + stepSelectionSlide());
                    }}
                  />
                )}
              </div>
            )}
          </div>
        )}
        {isOpen && !multiSelect && (
          <div
            className="collapsible-body"
            style={{ width: "100%", boxSizing: "border-box" }}
          >
            {loading && items.length === 0 ? (
              <div
                style={{
                  height: 344,
                  display: "flex",
                  justifyContent: "center",
                  alignItems: "center",
                }}
              >
                <i
                  className="fa fa-stroopwafel fa-spin fa-4x"
                  aria-hidden="true"
                ></i>
              </div>
            ) : items.length === 0 ? (
              <div
                style={{
                  height: 344,
                  display: "flex",
                  justifyContent: "center",
                  alignItems: "center",
                  flexDirection: "column",
                }}
              >
                <i className="fa fa-glasses fa-4x" aria-hidden="true"></i>
                <span style={{ marginTop: 12 }}>
                  {t("library.noResultsHere")}
                </span>
              </div>
            ) : (
              <div style={{ position: "relative" }}>
                <div
                  ref={carouselCallbackRef}
                  className="hide-scrollbar"
                  data-scroll-container
                  style={{
                    display: "flex",
                    gap: 8,
                    overflowX: "auto",
                    overflowY: "hidden",
                    padding: "8px 0",
                  }}
                >
                  {isBookmarkMode
                    ? bookmarkEntries.map((entry) => (
                        <div
                          key={entry.archive.arcid}
                          className="carousel-slide"
                        >
                          <BookmarkedArchiveHoverCard
                            entry={entry}
                            cropThumbs={cropThumbs}
                            onContextMenu={(e, archive) =>
                              onContextMenu(e, archive, "carousel")
                            }
                            onOpen={onOpen}
                            onSearchTag={onSearchTag}
                            onWheelPassthrough={handleBookmarkWheelPassthrough}
                          />
                        </div>
                      ))
                    : items.map((a) => (
                        <div key={a.arcid} className="carousel-slide">
                          <CarouselCard
                            archive={a}
                            cropThumbs={cropThumbs}
                            onContextMenu={(e, archive) =>
                              onContextMenu(e, archive, "carousel")
                            }
                            onOpen={onOpen}
                            onSearchTag={onSearchTag}
                          />
                        </div>
                      ))}
                </div>
                {carouselOverflow.canScrollLeft && (
                  <IconButton
                    className="carousel-prev"
                    icon={<FaChevronLeft size={24} />}
                    size={32}
                    style={{
                      position: "absolute",
                      left: 0,
                      top: "50%",
                      transform: "translateY(-50%)",
                      zIndex: 20,
                    }}
                    onClick={() => {
                      const lenis = lenisRef.current;
                      if (!lenis) return;
                      // Re-measure right before scrolling — see the selection-mode arrows' own
                      // comment (identical race, same fix) for why this can't be skipped.
                      lenis.resize();
                      lenis.scrollTo(lenis.targetScroll - stepSlide());
                    }}
                  />
                )}
                {carouselOverflow.canScrollRight && (
                  <IconButton
                    className="carousel-next"
                    icon={<FaChevronRight size={24} />}
                    size={32}
                    style={{
                      position: "absolute",
                      right: 0,
                      top: "50%",
                      transform: "translateY(-50%)",
                      zIndex: 20,
                    }}
                    onClick={() => {
                      const lenis = lenisRef.current;
                      if (!lenis) return;
                      lenis.resize();
                      lenis.scrollTo(lenis.targetScroll + stepSlide());
                    }}
                  />
                )}
              </div>
            )}
          </div>
        )}
      </li>
    </ul>
  );
}
