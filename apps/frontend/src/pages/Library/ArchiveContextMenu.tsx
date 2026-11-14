import { useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";

import { startDownload } from "@/api/download";
import type { ArchiveMetadata } from "@/api/types";
import {
  PopupMenu,
  PopupMenuItem,
  PopupMenuSeparator,
} from "@/components/common-ui/Display";
import { RatingWidget } from "@/components/common-ui/Form";
import { useMenuPalette } from "@/hooks/useMenuPalette";
import { copyWithToast } from "@/lib/clipboard";
import { routes } from "@/lib/routes";
import { splitTagsByNamespace } from "@/lib/tagFormat";
import { isTankoubonId } from "@/lib/utils/isTankoubonId";
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme";

import { type ContextMenuState } from "./types";

/** Ports legacy's right-click menu — same action set and login-gating (Edit/Delete/Rating/
 * Category only shown when logged in). Closes on any outside click or right-click. */
export function ArchiveContextMenu({
  state,
  categories,
  loggedIn,
  liveArchives,
  onClose,
  onToggleCategory,
  onDelete,
  onOpen,
  onRatingChange,
  onToggleSelection,
  isSelected,
  onSetProgress,
  onShare,
}: {
  state: ContextMenuState;
  categories:
    | { id: string; name: string; search: string | null; archives: string[] }[]
    | undefined;
  loggedIn: boolean;
  /** The live, refetch-synced search results — `state.archive` is a stale right-click-time
   * snapshot; looked up by id so the rating row reflects the just-saved value. */
  liveArchives: ArchiveMetadata[];
  onClose: () => void;
  onToggleCategory: (
    categoryId: string,
    archiveId: string,
    currentlyIn: boolean,
  ) => void;
  onDelete: (archiveId: string, isTank: boolean) => void;
  onOpen: (id: string) => void;
  onRatingChange: (
    archiveId: string,
    isTank: boolean,
    rating: string | null,
  ) => void;
  onToggleSelection: (id: string) => void;
  isSelected: boolean;
  /** Mutation lives in the parent, not here — `onClose()` unmounts this menu immediately, which
   * would tear down a locally-owned mutation before its callback ever fires. */
  onSetProgress: (archiveId: string, page: number) => void;
  /** 017: opens the share dialog for this item (the parent widens it to the whole selection). */
  onShare: (archiveId: string) => void;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { x, y } = state;
  const archive =
    liveArchives.find((a) => a.arcid === state.archive.arcid) ?? state.archive;
  const isTank = isTankoubonId(archive.arcid);
  const staticCategories = (categories ?? []).filter((c) => !c.search);
  const [categoryMenuOpen, setCategoryMenuOpen] = useState(false);
  const palette = useMenuPalette();
  const menuRef = useRef<HTMLUListElement | null>(null);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  // Geometry decided from the row's rect when the submenu opens: align with the row, then bound
  // the panel to the room actually left on that side. No post-mount measurement, so the panel
  // never renders at an overflowing position first.
  const [submenuPlacement, setSubmenuPlacement] = useState<{
    side: "left" | "right";
    up: boolean;
    maxHeight: number;
    maxWidth: number;
  } | null>(null);

  // `state.x/y` are document coordinates (clientX/Y + scroll), so keep `position: absolute` and
  // clamp against the *visual* viewport: on mobile `window.innerHeight` still counts the strip a
  // collapsed browser toolbar will cover, which is why clamping to it alone still let the menu run
  // off the bottom. `visualViewport` is the area actually visible right now.
  useLayoutEffect(() => {
    function place() {
      const menu = menuRef.current;
      if (!menu) return;
      const rect = menu.getBoundingClientRect();
      const margin = 8;
      const viewport = window.visualViewport;
      const scrollX = window.scrollX;
      const scrollY = window.scrollY;
      const visibleLeft = scrollX + (viewport?.offsetLeft ?? 0);
      const visibleTop = scrollY + (viewport?.offsetTop ?? 0);
      const visibleRight = visibleLeft + (viewport?.width ?? window.innerWidth);
      const visibleBottom = visibleTop + (viewport?.height ?? window.innerHeight);
      let left = x;
      let top = y;
      if (left + rect.width + margin > visibleRight) left = x - rect.width;
      if (top + rect.height + margin > visibleBottom) {
        top = visibleBottom - rect.height - margin;
      }
      left = Math.min(
        Math.max(visibleLeft + margin, left),
        Math.max(visibleLeft + margin, visibleRight - rect.width - margin),
      );
      top = Math.min(
        Math.max(visibleTop + margin, top),
        Math.max(visibleTop + margin, visibleBottom - rect.height - margin),
      );
      setPosition({ left, top });
    }
    // Placed once, in document coordinates: the menu then scrolls with the page and keeps its
    // position relative to the archive card. Only a width change (rotation, desktop window resize)
    // re-measures — a height-only resize is the mobile toolbar showing/hiding, and re-clamping for
    // that would visibly drag the menu away from its card.
    place();
    let lastWidth = window.innerWidth;
    function onResize() {
      if (window.innerWidth === lastWidth) return;
      lastWidth = window.innerWidth;
      place();
    }
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [x, y]);

  // Submenu opens on hover; a short close delay absorbs the mouse crossing the gap into it.
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  /** Place the panel beside the parent menu so the two never overlap, then fit it to the room. */
  function placeSubmenu(trigger: HTMLElement) {
    const anchor = menuRef.current?.getBoundingClientRect();
    const row = trigger.getBoundingClientRect();
    const viewport = window.visualViewport;
    const width = viewport?.width ?? window.innerWidth;
    const height = viewport?.height ?? window.innerHeight;
    const margin = 8;
    const side_ = anchor ?? row;
    const roomRight = width - margin - side_.right;
    const roomLeft = side_.left - margin;
    // Prefer the side with room beside the menu; never slide the panel over the menu itself.
    const side = roomRight >= 180 || roomRight >= roomLeft ? "right" : "left";
    const rows = Math.max(1, staticCategories.length);
    const needed = Math.min(220, rows * 30 + 14);
    const below = height - margin - row.bottom;
    const above = row.top - margin;
    // Vertical alignment is not required: open downwards when the list fits there, upwards when
    // only that side does. The panel scrolls instead of running off the screen.
    const up = below < needed && above > below;
    setSubmenuPlacement({
      side,
      up,
      maxWidth: Math.max(120, side === "right" ? roomRight : roomLeft),
      maxHeight: Math.min(220, Math.max(72, up ? above : below)),
    });
  }
  function openSubmenu(which: "category", trigger?: HTMLElement | null) {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current);
    if (trigger) placeSubmenu(trigger);
    setCategoryMenuOpen(which === "category");
  }
  function scheduleCloseSubmenus() {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current);
    closeTimerRef.current = setTimeout(() => {
      setCategoryMenuOpen(false);
    }, 200);
  }

  function copyLink() {
    const url = `${window.location.origin}${routes.reader(archive.arcid)}`;
    // `copyWithToast` falls back to a selection-based copy: the async clipboard API needs a secure
    // context, and a home instance is usually reached over plain HTTP, where this used to throw
    // before even the failure toast could appear.
    void copyWithToast(
      url,
      t("library.linkCopiedToClipboard") ?? "",
      t("library.failedToCopyLink") ?? "",
    );
  }

  return (
    <>
      <div
        style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={onClose}
        onContextMenu={(e) => {
          e.preventDefault();
          onClose();
        }}
      />
      <PopupMenu
        ref={menuRef}
        style={{
          position: "absolute",
          top: position?.top ?? y,
          left: position?.left ?? x,
          // Hidden for the one layout pass before the menu has been measured, so it never flashes
          // at the overflowing position first.
          visibility: position ? "visible" : "hidden",
          // Dynamic viewport height so a browser toolbar showing/hiding never re-creates overflow.
          // While the category submenu is open both are dropped: an `overflow` other than visible
          // clips the absolutely-positioned submenu, which is what made it look like the entry
          // simply refused to expand (and the scrollbar the user saw was this same box).
          maxHeight: categoryMenuOpen ? undefined : "calc(100dvh - 24px)",
          overflowY: categoryMenuOpen ? "visible" : "auto",
          zIndex: Z_OVERLAY_CONTENT,
        }}
      >
        {loggedIn && (
          <>
            <li
              className="flex items-center justify-center gap-1 px-2 pt-1"
              style={{
                paddingBottom: ".45em",
                borderBottom: `1px solid ${palette.separator}`,
                marginBottom: ".35em",
              }}
            >
              <RatingWidget
                archiveId={archive.arcid}
                tags={archive.tags}
                size={16}
                onChange={(nextTags) => {
                  const tagsByNamespace = splitTagsByNamespace(nextTags);
                  const rating = tagsByNamespace.rating?.[0] ?? null;
                  onRatingChange(archive.arcid, isTank, rating);
                }}
              />
            </li>
          </>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose();
            onOpen(archive.arcid);
          }}
        >
          <i className="fa fa-book-open" style={{ width: 18 }}></i>{" "}
          {t("common.read")}
        </PopupMenuItem>
        {loggedIn && !isTank && archive.pagecount > 0 && (
          <PopupMenuItem
            onClick={() => {
              onClose();
              onSetProgress(
                archive.arcid,
                archive.progress > 0 ? 0 : archive.pagecount,
              );
            }}
          >
            <i
              className={`fa ${archive.progress > 0 ? "fa-eye-slash" : "fa-eye"}`}
              style={{ width: 18 }}
            ></i>{" "}
            {archive.progress > 0
              ? t("library.markAsUnread")
              : t("library.markAsRead")}
          </PopupMenuItem>
        )}
        {loggedIn && (
          <PopupMenuItem
            onClick={() => {
              onClose();
              void startDownload(
                isTank
                  ? `/api/tankoubons/${archive.arcid}`
                  : `/api/archives/${archive.arcid}`,
              );
            }}
          >
            <i className="fa fa-download" style={{ width: 18 }}></i>{" "}
            {t("library.download")}
          </PopupMenuItem>
        )}
        {loggedIn && (
          <PopupMenuItem
            onClick={() => {
              onClose();
              onShare(archive.arcid);
            }}
          >
            <i className="fa fa-share-nodes" style={{ width: 18 }}></i>{" "}
            {t("share.action.share")}
          </PopupMenuItem>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose();
            copyLink();
          }}
        >
          <i className="fa fa-link" style={{ width: 18 }}></i>{" "}
          {t("library.copyLink")}
        </PopupMenuItem>
        {loggedIn && (
          <PopupMenuItem
            onClick={() => {
              onClose();
              onToggleSelection(archive.arcid);
            }}
          >
            <i className="fa fa-check-square" style={{ width: 18 }}></i>{" "}
            {isSelected
              ? t("library.removeFromSelection")
              : t("library.addToSelection")}
          </PopupMenuItem>
        )}
        {loggedIn && (
          <>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose();
                navigate(
                  isTank
                    ? routes.tankoubonEdit(archive.arcid)
                    : routes.edit(archive.arcid),
                );
              }}
            >
              <i className="fa fa-pen" style={{ width: 18 }}></i>{" "}
              {isTank ? t("common.editTankoubon") : t("library.editMetadata")}
            </PopupMenuItem>
            <PopupMenuItem
              style={{ position: "relative" }}
              onMouseEnter={(e) => openSubmenu("category", e.currentTarget)}
              onMouseLeave={scheduleCloseSubmenus}
              // Hover is the only way this opened before, so on a touch screen (no hover) the
              // submenu could never appear at all. A tap toggles it instead.
              onClick={(e) => {
                if (closeTimerRef.current) clearTimeout(closeTimerRef.current);
                if (categoryMenuOpen) {
                  setCategoryMenuOpen(false);
                } else {
                  openSubmenu("category", e.currentTarget);
                }
              }}
            >
              <i className="fa fa-search-plus" style={{ width: 18 }}></i>{" "}
              {t("library.addToCategory")}
              {categoryMenuOpen && (
                <PopupMenu
                  portal={false}
                  style={{
                    position: "absolute",
                    ...(submenuPlacement?.side === "left"
                      ? { right: "100%" }
                      : { left: "100%" }),
                    ...(submenuPlacement?.up ? { bottom: 0 } : { top: 0 }),
                    maxHeight: submenuPlacement?.maxHeight ?? 220,
                    maxWidth: submenuPlacement?.maxWidth ?? "calc(100vw - 16px)",
                    overflowY: "auto",
                    overflowX: "hidden",
                  }}
                  onMouseEnter={() => openSubmenu("category")}
                  onMouseLeave={scheduleCloseSubmenus}
                >
                  {staticCategories.length === 0 && (
                    <PopupMenuItem disabled>
                      {t("library.noCategoriesFound")}
                    </PopupMenuItem>
                  )}
                  {staticCategories.map((c) => {
                    const currentlyIn = c.archives.includes(archive.arcid);
                    return (
                      <PopupMenuItem
                        key={c.id}
                        onClick={() =>
                          onToggleCategory(c.id, archive.arcid, currentlyIn)
                        }
                      >
                        <input
                          type="checkbox"
                          readOnly
                          checked={currentlyIn}
                          style={{ verticalAlign: "middle" }}
                        />{" "}
                        {c.name}
                      </PopupMenuItem>
                    );
                  })}
                </PopupMenu>
              )}
            </PopupMenuItem>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose();
                onDelete(archive.arcid, isTank);
              }}
            >
              <i className="fa fa-trash" style={{ width: 18 }}></i>{" "}
              {t("common.delete")}
            </PopupMenuItem>
          </>
        )}
      </PopupMenu>
    </>
  );
}
