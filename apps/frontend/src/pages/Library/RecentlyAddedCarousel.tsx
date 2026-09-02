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
  const carouselRef = useRef<HTMLDivElement>(null);
  const lenisRef = useRef<Lenis | null>(null);
  const stepSlide = useCallback(() => {
    const firstChild = carouselRef.current
      ?.firstElementChild as HTMLElement | null;
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
  const bookmarksQuery = useInfiniteBookmarks("bookmarked_at");
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

  // Lenis drives real `scrollLeft` on `el` (not a `transform`), so native `scroll` events still
  // fire — `BookmarkedArchiveHoverCard`'s `[data-scroll-container]` listener relies on this.
  useEffect(() => {
    const el = carouselRef.current;
    if (!el || lenisRef.current) return;
    const lenis = new Lenis({
      wrapper: el,
      content: el,
      orientation: "horizontal",
      gestureOrientation: "both",
      wheelMultiplier: 4.5,
      lerp: 0.1,
      autoRaf: true,
    });
    lenisRef.current = lenis;
    return () => {
      lenis.destroy();
      lenisRef.current = null;
    };
  }, [items]);

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
              <div style={{ padding: "8px 0" }}>
                <SortableList
                  items={selectedIds}
                  getId={(id) => id}
                  direction="horizontal"
                  onReorder={onReorderSelection}
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
                  ref={carouselRef}
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
                    if (lenis) lenis.scrollTo(lenis.targetScroll - stepSlide());
                  }}
                />
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
                    if (lenis) lenis.scrollTo(lenis.targetScroll + stepSlide());
                  }}
                />
              </div>
            )}
          </div>
        )}
      </li>
    </ul>
  );
}
